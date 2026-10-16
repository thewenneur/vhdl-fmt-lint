//! Loading a whole project: reading and parsing every source in parallel,
//! then inferring libraries.

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::SystemTime;

use rayon::prelude::*;
use rustc_hash::FxHashMap;
use vfl_config::{Config, VhdlLsMode};
use vfl_syntax::{Parse, Standard};

use crate::explicit::{LibraryGlobs, VHDL_LS_FILES, path_to_slash, vhdl_ls_libraries};
use crate::infer::{Dirs, Inference, Reason, infer_in, infer_libraries};
use crate::merge::{
    Decision, Kind, LibraryConflict, Libs, Opinions, Pick, apply, decide, default_pick,
};
use crate::summary::{FileSummary, summarize};
use crate::verilog::{VerilogFile, verilog_standard};
use crate::{ProjectRoot, collect_project};

/// How a source file was encoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Encoding {
    Utf8,
    Latin1,
}

/// Decodes a source file: UTF-8 if valid, else Latin-1 (common in VHDL),
/// byte for byte.
pub fn decode(bytes: Vec<u8>) -> (String, Encoding) {
    match String::from_utf8(bytes) {
        Ok(text) => (text, Encoding::Utf8),
        Err(err) => (
            err.into_bytes().iter().map(|&b| char::from(b)).collect(),
            Encoding::Latin1,
        ),
    }
}

/// Encodes text back the way it was decoded.
pub fn encode(text: &str, encoding: Encoding) -> Vec<u8> {
    match encoding {
        Encoding::Utf8 => text.as_bytes().to_vec(),
        Encoding::Latin1 => text
            .chars()
            .map(|c| u8::try_from(c).unwrap_or(b'?'))
            .collect(),
    }
}

/// A parsed source file of a project.
#[derive(Debug, Clone)]
pub struct SourceFile {
    pub path: PathBuf,
    pub text: Arc<str>,
    pub encoding: Encoding,
    pub parse: Parse,
    pub summary: FileSummary,
}

impl SourceFile {
    /// Parses `text` and summarizes it.
    pub fn new(
        path: PathBuf,
        text: impl Into<Arc<str>>,
        encoding: Encoding,
        standard: Standard,
    ) -> Self {
        let text = text.into();
        let parse = vfl_syntax::parse(&text, standard);
        let summary = summarize(&parse.syntax());
        SourceFile {
            path,
            text,
            encoding,
            parse,
            summary,
        }
    }

    /// Reads, decodes and parses a file.
    pub fn read(path: PathBuf, standard: Standard) -> std::io::Result<Self> {
        let (text, encoding) = decode(std::fs::read(&path)?);
        Ok(Self::new(path, text, encoding, standard))
    }
}

enum Loaded {
    Vhdl(SourceFile),
    Verilog(VerilogFile),
}

/// A loaded project: its root, config, files and their libraries.
#[derive(Debug, Clone)]
pub struct Project {
    pub root: ProjectRoot,
    pub config: Config,
    pub files: Vec<SourceFile>,
    /// Libraries of each file, parallel to `files`.
    pub libraries: Vec<Vec<(String, Reason)>>,
    /// The Verilog and SystemVerilog files, whose modules VHDL can
    /// instantiate.
    pub verilog: Vec<VerilogFile>,
    /// Libraries of each Verilog file, parallel to `verilog`: those of the
    /// units that instantiate its modules, as for VHDL files.
    pub verilog_libraries: Vec<Vec<(String, Reason)>>,
    /// Files that could not be read, and why.
    pub unreadable: Vec<(PathBuf, String)>,
    /// Inference ambiguities and other things worth telling the user.
    pub notes: Vec<String>,
    /// Files the library sources disagree about, and what vfl chose.
    pub library_conflicts: Vec<LibraryConflict>,
    /// Files of other tools that may list libraries (VUnit `run.py`,
    /// FuseSoC `.core`, Vivado and Quartus projects), read on each
    /// inference.
    pub tool_files: Vec<PathBuf>,
    /// Where each file is in `files` and `verilog`, by path.
    pub paths: PathIndex,
}

/// The library lists of a project, as [`Project::library_sources`] read
/// them, with when each file they come from was last modified.
pub struct LibrarySources {
    /// The config's list, if any.
    config: Option<LibraryGlobs>,
    /// Those of tool files.
    tools: Vec<LibraryGlobs>,
    /// `vhdl_ls.toml`'s list, unless `[project]` `vhdl-ls` is off.
    vhdl_ls: Option<LibraryGlobs>,
    vhdl_ls_mode: VhdlLsMode,
    /// How the lists and the code are merged, file by file. Decided on
    /// the first inference with these lists and kept, so that later ones
    /// (on edits) run inference once.
    decision: OnceLock<Decision>,
    /// What checking the lists against the sources found.
    notes: Vec<String>,
    stamps: Vec<(PathBuf, Option<SystemTime>)>,
}

impl LibrarySources {
    /// Whether a file they come from was created, changed or deleted since
    /// they were read. The config's lists are not checked: a changed
    /// config means loading the project again.
    pub fn is_stale(&self) -> bool {
        self.stamps
            .iter()
            .any(|(path, time)| modified(path) != *time)
    }
}

fn modified(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

#[derive(Debug, thiserror::Error)]
pub enum LoadError {
    #[error(transparent)]
    Config(#[from] vfl_config::ConfigError),
    #[error("scanning {path}: {source}")]
    Walk {
        path: PathBuf,
        source: ignore::Error,
    },
    #[error("invalid library pattern in {source_file}: {error}")]
    Glob {
        source_file: String,
        error: globset::Error,
    },
}

impl Project {
    /// Loads the project at `root`: collects sources, parses them in
    /// parallel and infers libraries.
    pub fn load(root: ProjectRoot) -> Result<Project, LoadError> {
        Self::load_or(root, &Config::default())
    }

    /// Like [`Project::load`], with `defaults` as the config when no config
    /// file applies: the settings of an editor.
    pub fn load_or(root: ProjectRoot, defaults: &Config) -> Result<Project, LoadError> {
        let config = vfl_config::load_for_dir_or(&root.dir, defaults)?;
        Self::load_with(root, config, &|_, _| {})
    }

    /// Loads the project at `root` with `config`, telling `progress` how
    /// many of how many files are read and parsed so far, from the
    /// threads that parse them.
    pub fn load_with(
        root: ProjectRoot,
        config: Config,
        progress: &(dyn Fn(usize, usize) + Sync),
    ) -> Result<Project, LoadError> {
        let (paths, tool_files) =
            collect_project(&root.dir, &config).map_err(|source| LoadError::Walk {
                path: root.dir.clone(),
                source,
            })?;
        Self::from_parts(root, config, paths, tool_files, progress)
    }

    /// Loads the given files as a project rooted at `root`. Verilog and
    /// SystemVerilog files among them (by the `[verilog]` extensions) are
    /// loaded as such.
    pub fn from_paths(
        root: ProjectRoot,
        config: Config,
        paths: Vec<PathBuf>,
    ) -> Result<Project, LoadError> {
        let tool_files = collect_project(&root.dir, &config)
            .map(|(_, tools)| tools)
            .unwrap_or_default();
        Self::from_parts(root, config, paths, tool_files, &|_, _| {})
    }

    fn from_parts(
        root: ProjectRoot,
        config: Config,
        paths: Vec<PathBuf>,
        tool_files: Vec<PathBuf>,
        progress: &(dyn Fn(usize, usize) + Sync),
    ) -> Result<Project, LoadError> {
        let standard = config.standard();
        let total = paths.len();
        let done = std::sync::atomic::AtomicUsize::new(0);
        // With `[verilog]` `mode` off, Verilog files are left out, not
        // read as VHDL.
        let results: Vec<_> = paths
            .into_par_iter()
            .filter_map(|path| {
                let read = match verilog_standard(&config, &path) {
                    Some(_) if !config.verilog.loaded() => None,
                    Some(std) => Some(
                        VerilogFile::read(path.clone(), &config, &root.dir, std)
                            .map(Loaded::Verilog),
                    ),
                    None => Some(SourceFile::read(path.clone(), standard).map(Loaded::Vhdl)),
                };
                let done = done.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                progress(done, total);
                Some(read?.map_err(|err| (path, err.to_string())))
            })
            .collect();
        let mut files = Vec::new();
        let mut verilog = Vec::new();
        let mut unreadable = Vec::new();
        for result in results {
            match result {
                Ok(Loaded::Vhdl(file)) => files.push(file),
                Ok(Loaded::Verilog(file)) => verilog.push(file),
                Err(err) => unreadable.push(err),
            }
        }
        let mut project = Project {
            root,
            config,
            files,
            libraries: Vec::new(),
            verilog,
            verilog_libraries: Vec::new(),
            unreadable,
            notes: Vec::new(),
            library_conflicts: Vec::new(),
            tool_files,
            paths: PathIndex::default(),
        };
        project.infer()?;
        Ok(project)
    }

    /// (Re)computes every file's libraries.
    pub fn infer(&mut self) -> Result<(), LoadError> {
        let sources = self.library_sources()?;
        self.infer_with(&sources);
        Ok(())
    }

    /// Reads the library lists that assign files to libraries: the
    /// config's, those of tool files and `vhdl_ls.toml`.
    pub fn library_sources(&self) -> Result<LibrarySources, LoadError> {
        let root = self.root.dir.clone();
        let mut config = if self.config.project.libraries.is_empty() {
            None
        } else {
            let source = vfl_config::find_config(&root)
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "vfl.toml".into());
            Some(
                LibraryGlobs::new(source.clone(), &root, &root, &self.config.project.libraries)
                    .map_err(|error| LoadError::Glob {
                        source_file: source,
                        error,
                    })?,
            )
        };
        let mut vhdl_ls = match self.config.project.vhdl_ls {
            VhdlLsMode::Off => None,
            VhdlLsMode::Fallback | VhdlLsMode::Prefer => vhdl_ls_libraries(&root),
        }
        .and_then(|(path, libraries)| {
            let source = path.display().to_string();
            let base = path.parent().unwrap_or(&root).to_owned();
            // A broken vhdl_ls.toml is not ours to reject; skip it.
            LibraryGlobs::new(source, &base, &root, &libraries).ok()
        });
        let mut tools = Vec::new();
        for (path, libraries) in crate::import::import_all(&self.tool_files) {
            let source = path.display().to_string();
            let base = path.parent().unwrap_or(&root).to_owned();
            if let Ok(g) = LibraryGlobs::new_strict(source, &base, &root, &libraries) {
                tools.push(g);
            }
        }
        let stamps = VHDL_LS_FILES
            .iter()
            .map(|name| root.join(name))
            .chain(self.tool_files.iter().cloned())
            .map(|path| {
                let time = modified(&path);
                (path, time)
            })
            .collect();
        // Lists that lag behind the tree: entries matching nothing are
        // read without their leading directories when that makes them
        // match (not those of vfl.toml, which is ours to get right).
        let relative = self.relative_paths();
        let mut notes = Vec::new();
        if let Some(config) = &mut config {
            notes.extend(config.check(&root, &relative, false));
        }
        for list in tools.iter_mut().chain(&mut vhdl_ls) {
            notes.extend(list.check(&root, &relative, true));
        }
        Ok(LibrarySources {
            config,
            tools,
            vhdl_ls,
            vhdl_ls_mode: self.config.project.vhdl_ls,
            decision: OnceLock::new(),
            notes,
            stamps,
        })
    }

    /// The paths of the sources, VHDL then Verilog, relative to the root and
    /// `/`-separated, as library lists match them.
    fn relative_paths(&self) -> Vec<String> {
        let root = &self.root.dir;
        (self.files.iter().map(|f| &f.path))
            .chain(self.verilog.iter().map(|f| &f.path))
            .map(|path| {
                path.strip_prefix(root)
                    .map(path_to_slash)
                    .unwrap_or_else(|_| path_to_slash(path))
            })
            .collect()
    }

    /// Computes every file's libraries from `sources` and what the files
    /// declare and refer to.
    pub fn infer_with(&mut self, sources: &LibrarySources) {
        // Verilog files take part like VHDL files, after them.
        let paths: Vec<PathBuf> = self
            .files
            .iter()
            .map(|f| f.path.clone())
            .chain(self.verilog.iter().map(|f| f.path.clone()))
            .collect();
        let relative = self.relative_paths();
        let summaries: Vec<FileSummary> = self
            .files
            .iter()
            .map(|f| f.summary.clone())
            .chain(self.verilog.iter().map(|f| f.summary.clone()))
            .collect();
        // `vfl.toml` is never questioned, nor `vhdl_ls.toml` when
        // preferred.
        let preferred = match sources.vhdl_ls_mode {
            VhdlLsMode::Prefer => sources.vhdl_ls.as_ref(),
            _ => None,
        };
        let fixed: Vec<Option<Libs>> = relative
            .iter()
            .map(|r| (sources.config.iter().chain(preferred)).find_map(|s| s.assign(r)))
            .collect();
        let merged = match sources.vhdl_ls_mode {
            VhdlLsMode::Fallback => sources.vhdl_ls.as_ref(),
            _ => None,
        };
        if sources.tools.is_empty() && merged.is_none() {
            let none = vec![None; paths.len()];
            let Inference {
                mut libraries,
                mut notes,
            } = infer_libraries(&paths, &summaries, &fixed, &none);
            notes.splice(0..0, sources.notes.iter().cloned());
            self.verilog_libraries = libraries.split_off(self.files.len());
            self.libraries = libraries;
            self.notes = notes;
            self.library_conflicts = Vec::new();
            return;
        }
        let opinions_of = |r: &str| -> Opinions {
            (sources.tools.iter().map(|t| (Kind::Tool, t)))
                .chain(merged.map(|v| (Kind::VhdlLs, v)))
                .filter_map(|(kind, list)| Some((kind, list.assign(r)?)))
                .collect()
        };
        let mut fresh = None;
        let decision = sources.decision.get_or_init(|| {
            let opinions: Vec<Opinions> = relative.iter().map(|r| opinions_of(r)).collect();
            let (mut decision, result) = decide(&paths, &relative, &summaries, &fixed, &opinions);
            decision.notes.splice(0..0, sources.notes.iter().cloned());
            fresh = Some(result);
            decision
        });
        // Just decided: with the libraries that came of it.
        let Inference {
            mut libraries,
            mut notes,
        } = fresh.unwrap_or_else(|| {
            let picks: Vec<Pick> = relative
                .iter()
                .map(|r| match decision.picks.get(r) {
                    Some(pick) => pick.clone(),
                    None => default_pick(&opinions_of(r)),
                })
                .collect();
            let a = apply(&fixed, &picks);
            infer_in(
                &paths,
                &Dirs::new(&paths),
                &summaries,
                &a.explicit,
                &a.fallback,
                &a.seeds,
            )
        });
        notes.splice(0..0, decision.notes.iter().cloned());
        notes.dedup();
        self.library_conflicts = decision.conflicts.clone();
        self.verilog_libraries = libraries.split_off(self.files.len());
        self.libraries = libraries;
        self.notes = notes;
    }

    /// The vendor libraries whose bundled stubs the analysis needs: those a
    /// VHDL file uses (`unisim.vcomponents`, `entity xpm.xpm_cdc_single`)
    /// that no file of the project is in, unless `[project]` `vendor-stubs`
    /// is off. In the order of [`vfl_sema::VENDOR_LIBRARIES`].
    pub fn vendor_stubs(&self) -> Vec<&'static str> {
        if !self.config.project.vendor_stubs {
            return Vec::new();
        }
        let mut used = [false; vfl_sema::VENDOR_LIBRARIES.len()];
        for r in self.files.iter().flat_map(|f| &f.summary.refs) {
            if r.library != "work"
                && let Some(i) = vfl_sema::VENDOR_LIBRARIES
                    .iter()
                    .position(|l| *l == r.library)
            {
                used[i] = true;
            }
        }
        if !used.contains(&true) {
            return Vec::new();
        }
        let held = |library: &str| {
            self.libraries
                .iter()
                .chain(&self.verilog_libraries)
                .flatten()
                .any(|(l, _)| l == library)
        };
        vfl_sema::VENDOR_LIBRARIES
            .iter()
            .zip(used)
            .filter(|&(library, used)| used && !held(library))
            .map(|(library, _)| *library)
            .collect()
    }

    /// The index of the Verilog file at `path`.
    pub fn verilog_index(&self, path: &Path) -> Option<usize> {
        self.paths.verilog.find(&self.verilog, |f| &f.path, path)
    }

    /// The index of the file at `path`.
    pub fn file_index(&self, path: &Path) -> Option<usize> {
        self.paths.vhdl.find(&self.files, |f| &f.path, path)
    }
}

/// Where each file of a [`Project`] is in its lists, for
/// [`Project::file_index`] and [`Project::verilog_index`], which the
/// language server calls on every edit. The lists can change without it
/// knowing, so each index found is checked, and the map is built again
/// when one is wrong or a file is not in it but is in the list.
#[derive(Default)]
pub struct PathIndex {
    vhdl: PathMap,
    verilog: PathMap,
}

impl Clone for PathIndex {
    fn clone(&self) -> Self {
        PathIndex::default()
    }
}

impl std::fmt::Debug for PathIndex {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PathIndex")
    }
}

#[derive(Default)]
struct PathMap(std::sync::Mutex<FxHashMap<PathBuf, usize>>);

impl PathMap {
    fn find<T>(&self, items: &[T], path_of: fn(&T) -> &PathBuf, path: &Path) -> Option<usize> {
        let mut map = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(&i) = map.get(path)
            && items.get(i).is_some_and(|item| path_of(item) == path)
        {
            return Some(i);
        }
        // Not in the map, or elsewhere in the list: searched as before,
        // and the map is built again when that finds it.
        let found = items.iter().position(|item| path_of(item) == path)?;
        map.clear();
        for (i, item) in items.iter().enumerate() {
            map.entry(path_of(item).clone()).or_insert(i);
        }
        Some(found)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RootKind;
    use std::fs;

    /// Files are found by path whatever was done to the list since.
    #[test]
    fn file_index_follows_changes_to_the_list() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["a", "b", "c"] {
            fs::write(dir.path().join(format!("{name}.vhd")), "entity e is end;").unwrap();
        }
        let mut project = Project::load(ProjectRoot {
            dir: dir.path().to_owned(),
            kind: RootKind::Fallback,
        })
        .unwrap();
        let path = |name: &str| dir.path().join(format!("{name}.vhd"));
        let index = |project: &Project, name: &str| {
            let found = project.file_index(&path(name));
            assert_eq!(
                found,
                project.files.iter().position(|f| f.path == path(name)),
                "{name}"
            );
            found
        };
        assert!(index(&project, "b").is_some());
        assert_eq!(index(&project, "d"), None);
        let a = project.files.remove(index(&project, "a").unwrap());
        assert_eq!(index(&project, "a"), None);
        index(&project, "b");
        index(&project, "c");
        let standard = project.config.standard();
        project
            .files
            .push(SourceFile::new(path("d"), "", Encoding::Utf8, standard));
        project.files.insert(0, a);
        for name in ["a", "b", "c", "d"] {
            index(&project, name);
        }
        // Another file in the place of one.
        let i = index(&project, "c").unwrap();
        project.files[i] = SourceFile::new(path("e"), "", Encoding::Utf8, standard);
        assert_eq!(index(&project, "c"), None);
        assert_eq!(index(&project, "e"), Some(i));
        // A copy finds them too.
        let copy = project.clone();
        for name in ["a", "b", "d", "e"] {
            index(&copy, name);
        }
    }

    #[test]
    fn loads_and_infers_with_explicit_entries() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("ext")).unwrap();
        fs::create_dir_all(root.join("rtl")).unwrap();
        fs::write(root.join("ext/pkg.vhd"), "package pkg is end;").unwrap();
        fs::write(
            root.join("rtl/top.vhd"),
            "library ext; use ext.pkg.all; entity top is end;",
        )
        .unwrap();
        fs::write(root.join("rtl/latin1.vhd"), b"-- caf\xe9\nentity l is end;").unwrap();
        fs::write(
            root.join("vfl.toml"),
            "[project.libraries]\nvendor = [\"ext/*.vhd\"]\n",
        )
        .unwrap();
        let project = Project::load(ProjectRoot {
            dir: root.to_owned(),
            kind: RootKind::Config,
        })
        .unwrap();
        let libs = |name: &str| {
            let i = project.file_index(&root.join(name)).unwrap();
            project.libraries[i]
                .iter()
                .map(|(l, _)| l.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(libs("ext/pkg.vhd"), ["vendor"]);
        assert_eq!(libs("rtl/top.vhd"), ["work"]);
        let latin = &project.files[project.file_index(&root.join("rtl/latin1.vhd")).unwrap()];
        assert_eq!(latin.encoding, Encoding::Latin1);
        assert!(latin.text.contains('é'));
    }

    #[test]
    fn verilog_modules_join_the_libraries_that_use_them() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("rtl")).unwrap();
        // `core` is instantiated from `fast`, `leaf` from `core`; `alone` from nothing.
        fs::write(
            root.join("rtl/top.vhd"),
            "library fast; entity top is end;\n\
             architecture a of top is begin u : entity fast.core; end;",
        )
        .unwrap();
        fs::write(
            root.join("rtl/core.sv"),
            "`include \"defs.svh\"\n\
             module core(input logic clk);\n  leaf u (.clk(clk));\nendmodule\n",
        )
        .unwrap();
        fs::write(
            root.join("rtl/leaf.v"),
            "module leaf(input clk); endmodule\n",
        )
        .unwrap();
        fs::write(root.join("rtl/alone.v"), "module alone; endmodule\n").unwrap();
        fs::write(root.join("rtl/defs.svh"), "`define W 8\n").unwrap();
        // Off: the Verilog files are left out, not read as VHDL.
        fs::write(root.join("vfl.toml"), "[verilog]\nmode = \"off\"\n").unwrap();
        let load = || {
            Project::load(ProjectRoot {
                dir: root.to_owned(),
                kind: RootKind::Vcs,
            })
            .unwrap()
        };
        let off = load();
        assert_eq!(off.files.len(), 1);
        assert!(off.verilog.is_empty());
        // Names only, the default.
        fs::remove_file(root.join("vfl.toml")).unwrap();
        let project = Project::load(ProjectRoot {
            dir: root.to_owned(),
            kind: RootKind::Vcs,
        })
        .unwrap();
        assert_eq!(project.files.len(), 1);
        let libs = |name: &str| {
            let i = project.verilog_index(&root.join(name)).unwrap();
            project.verilog_libraries[i]
                .iter()
                .map(|(l, _)| l.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(libs("rtl/core.sv"), ["fast"]);
        assert_eq!(libs("rtl/leaf.v"), ["fast"]);
        assert_eq!(libs("rtl/alone.v"), ["work"]);
        // A header is parsed too, but has no modules.
        assert_eq!(libs("rtl/defs.svh"), ["work"]);
        let core = &project.verilog[project.verilog_index(&root.join("rtl/core.sv")).unwrap()];
        assert!(core.parse.errors().is_empty(), "{:?}", core.parse.errors());
        assert_eq!(core.modules[0].ports[0].name, "clk");
        assert_eq!(core.foreign[0].name, "core");
    }

    #[test]
    fn vhdl_ls_toml_is_imported() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/a.vhd"), "entity a is end;").unwrap();
        fs::write(
            root.join("vhdl_ls.toml"),
            "[libraries]\nmylib.files = ['src/*.vhd']\n",
        )
        .unwrap();
        let project = Project::load(ProjectRoot {
            dir: root.to_owned(),
            kind: RootKind::Vcs,
        })
        .unwrap();
        assert_eq!(project.libraries[0][0].0, "mylib");
    }

    #[test]
    fn inference_ranks_above_vhdl_ls_toml_by_default() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::create_dir_all(root.join("tb")).unwrap();
        fs::write(root.join("src/pkg.vhd"), "package pkg is end;").unwrap();
        fs::write(
            root.join("src/core.vhd"),
            "use work.pkg.all; entity core is end;",
        )
        .unwrap();
        fs::write(root.join("src/top.vhd"), "entity top is end;").unwrap();
        fs::write(
            root.join("src/arch.vhd"),
            "architecture a of top is begin end;",
        )
        .unwrap();
        fs::write(
            root.join("tb/tb.vhd"),
            "library real_lib; use real_lib.pkg.all; entity tb is end;",
        )
        .unwrap();
        // Stale: lists the sources in a library nothing uses, and not
        // src/arch.vhd.
        fs::write(
            root.join("vhdl_ls.toml"),
            "[libraries]\nold_lib.files = ['src/pkg.vhd', 'src/core.vhd', 'src/top.vhd']\n",
        )
        .unwrap();
        let load = || {
            Project::load(ProjectRoot {
                dir: root.to_owned(),
                kind: RootKind::Vcs,
            })
            .unwrap()
        };
        let libs = |project: &Project, name: &str| {
            let i = project.file_index(&root.join(name)).unwrap();
            project.libraries[i]
                .iter()
                .map(|(l, _)| l.clone())
                .collect::<Vec<_>>()
        };
        let project = load();
        // What the code says wins.
        assert_eq!(libs(&project, "src/pkg.vhd"), ["real_lib"]);
        // core.vhd follows the pkg it uses through `work`.
        assert_eq!(libs(&project, "src/core.vhd"), ["real_lib"]);
        // Nothing in the code places top.vhd: the list does.
        assert_eq!(libs(&project, "src/top.vhd"), ["old_lib"]);
        // Placed by the list, top's architecture follows it.
        assert_eq!(libs(&project, "src/arch.vhd"), ["old_lib"]);
        assert_eq!(libs(&project, "tb/tb.vhd"), ["work"]);

        fs::write(root.join("vfl.toml"), "[project]\nvhdl-ls = \"prefer\"\n").unwrap();
        let project = load();
        assert_eq!(libs(&project, "src/pkg.vhd"), ["old_lib"]);

        fs::write(root.join("vfl.toml"), "[project]\nvhdl-ls = \"off\"\n").unwrap();
        let project = load();
        assert_eq!(libs(&project, "src/pkg.vhd"), ["real_lib"]);
        assert_eq!(libs(&project, "src/top.vhd"), ["work"]);
    }

    #[test]
    fn vhdl_ls_toml_wins_when_it_leaves_fewer_conflicts() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        // Each VIP declares its own `methods`; the testbench's `clk.methods`
        // is closest to uart/methods.vhd, which inference wrongly puts in
        // `clk` next to clk/methods.vhd.
        for vip in ["clk", "uart"] {
            fs::create_dir_all(root.join(vip)).unwrap();
            fs::write(
                root.join(vip).join("methods.vhd"),
                "package methods is end;",
            )
            .unwrap();
            fs::write(
                root.join(vip).join("vvc.vhd"),
                format!("library {vip}; use work.methods.all; entity {vip}_vvc is end;"),
            )
            .unwrap();
        }
        fs::write(
            root.join("uart/tb.vhd"),
            "library clk; use clk.methods.all; entity tb is end;",
        )
        .unwrap();
        fs::write(
            root.join("other.vhd"),
            "library uart, clk; use uart.uart_vvc; use clk.clk_vvc; entity t is end;",
        )
        .unwrap();
        let load = || {
            Project::load(ProjectRoot {
                dir: root.to_owned(),
                kind: RootKind::Vcs,
            })
            .unwrap()
        };
        let libs = |project: &Project, name: &str| {
            let i = project.file_index(&root.join(name)).unwrap();
            project.libraries[i]
                .iter()
                .map(|(l, _)| l.clone())
                .collect::<Vec<_>>()
        };
        let project = load();
        assert_eq!(libs(&project, "uart/methods.vhd"), ["clk", "uart"]);
        assert!(project.notes.is_empty(), "{:?}", project.notes);

        fs::write(
            root.join("vhdl_ls.toml"),
            "[libraries]\nclk.files = ['clk/*.vhd']\nuart.files = ['uart/*.vhd']\n",
        )
        .unwrap();
        let project = load();
        assert_eq!(libs(&project, "uart/methods.vhd"), ["uart"]);
        assert_eq!(libs(&project, "clk/methods.vhd"), ["clk"]);
        // The code is set aside for the list: nothing to report.
        assert!(project.library_conflicts.is_empty());

        // Off: inference alone.
        fs::write(root.join("vfl.toml"), "[project]\nvhdl-ls = \"off\"\n").unwrap();
        let project = load();
        assert_eq!(libs(&project, "uart/methods.vhd"), ["clk", "uart"]);
    }

    #[test]
    fn vendor_stubs_for_used_libraries_the_project_lacks() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::write(
            root.join("top.vhd"),
            "library unisim, xpm, altera_mf; use unisim.vcomponents.all;\n\
             entity top is end;\n\
             architecture a of top is begin u : entity xpm.xpm_cdc_single; end;",
        )
        .unwrap();
        let load = || {
            Project::load(ProjectRoot {
                dir: root.to_owned(),
                kind: RootKind::Fallback,
            })
            .unwrap()
        };
        // `altera_mf` is named but not used.
        assert_eq!(load().vendor_stubs(), ["unisim", "xpm"]);
        // A real copy of a library wins.
        fs::write(root.join("vcomponents.vhd"), "package vcomponents is end;").unwrap();
        assert_eq!(load().vendor_stubs(), ["xpm"]);
        fs::write(root.join("vfl.toml"), "[project]\nvendor-stubs = false\n").unwrap();
        assert!(load().vendor_stubs().is_empty());
    }

    #[test]
    fn tool_files_assign_libraries() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("rtl/sub")).unwrap();
        std::fs::create_dir(root.join(".git")).unwrap();
        std::fs::write(root.join("rtl/a.vhd"), "package a is end package;\n").unwrap();
        std::fs::write(root.join("rtl/sub/b.vhd"), "package b is end package;\n").unwrap();
        std::fs::write(
            root.join("build.tcl"),
            "read_vhdl -library mylib [glob rtl/*.vhd]\n",
        )
        .unwrap();
        let project = Project::load(crate::find_root(root)).unwrap();
        let library = |name: &str| {
            let index = project
                .files
                .iter()
                .position(|f| f.path.ends_with(name))
                .unwrap();
            project.libraries[index][0].0.clone()
        };
        assert_eq!(library("a.vhd"), "mylib");
        // `*` stays within `rtl/`, as in Vivado.
        assert_eq!(library("b.vhd"), "work");
    }

    fn write(root: &Path, files: &[(&str, &str)]) {
        for (name, text) in files {
            let path = root.join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, text).unwrap();
        }
    }

    fn libraries_of(project: &Project, root: &Path, name: &str) -> Vec<String> {
        let i = project.file_index(&root.join(name)).unwrap();
        project.libraries[i]
            .iter()
            .map(|(l, _)| l.clone())
            .collect()
    }

    #[test]
    fn the_code_decides_between_disagreeing_tool_files() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir(root.join(".git")).unwrap();
        write(
            root,
            &[
                ("rtl/pkg.vhd", "package pkg is end;"),
                ("rtl/core.vhd", "use work.pkg.all; entity core is end;"),
                (
                    "tb/tb.vhd",
                    "library good; use good.pkg.all; use good.core; entity tb is end;",
                ),
                // An old script, first in path order, names another library.
                ("a_old.tcl", "read_vhdl -library stale [glob rtl/*.vhd]\n"),
                ("build.tcl", "read_vhdl -library good [glob rtl/*.vhd]\n"),
            ],
        );
        let project = Project::load(crate::find_root(root)).unwrap();
        assert_eq!(libraries_of(&project, root, "rtl/pkg.vhd"), ["good"]);
        assert_eq!(libraries_of(&project, root, "rtl/core.vhd"), ["good"]);
        let conflict = project
            .library_conflicts
            .iter()
            .find(|c| c.path.ends_with("rtl/pkg.vhd"))
            .unwrap();
        assert_eq!(conflict.chosen, ["good"]);
        let message = conflict.message();
        assert!(
            message.contains("a_old.tcl puts it in `stale`"),
            "{message}"
        );
        assert!(message.contains("vfl uses `good`"), "{message}");
    }

    #[test]
    fn a_stale_vhdl_ls_toml_is_merged_file_by_file() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir(root.join(".git")).unwrap();
        // Two VIPs declare packages of the same name, which inference
        // alone cannot tell apart; the list does, but names the library
        // of util/ as it was before a rename.
        let mut files = vec![
            (
                "util/types.vhd".to_owned(),
                "package types is end;".to_owned(),
            ),
            (
                "uart/tb.vhd".to_owned(),
                "library clk; use clk.methods.all; entity tb is end;".to_owned(),
            ),
            (
                "other.vhd".to_owned(),
                "library uart, clk; use uart.uart_vvc; use clk.clk_vvc; entity t is end;"
                    .to_owned(),
            ),
            (
                "vhdl_ls.toml".to_owned(),
                "[libraries]\nclk.files = ['clk/*.vhd']\nuart.files = ['uart/*.vhd']\n\
                 util_v1.files = ['util/*.vhd']\n"
                    .to_owned(),
            ),
        ];
        for vip in ["clk", "uart"] {
            files.push((
                format!("{vip}/methods.vhd"),
                "package methods is end;".to_owned(),
            ));
            files.push((
                format!("{vip}/vvc.vhd"),
                format!(
                    "library {vip}, util; use util.types.all; use work.methods.all; \
                     entity {vip}_vvc is end;"
                ),
            ));
        }
        let files: Vec<(&str, &str)> = files
            .iter()
            .map(|(p, t)| (p.as_str(), t.as_str()))
            .collect();
        write(root, &files);
        let project = Project::load(crate::find_root(root)).unwrap();
        // The list where the code cannot tell...
        assert_eq!(libraries_of(&project, root, "uart/methods.vhd"), ["uart"]);
        assert_eq!(libraries_of(&project, root, "clk/methods.vhd"), ["clk"]);
        // ...and the code where it can.
        assert_eq!(libraries_of(&project, root, "util/types.vhd"), ["util"]);
        assert_eq!(project.library_conflicts.len(), 1);
        assert!(
            project.library_conflicts[0]
                .path
                .ends_with("util/types.vhd")
        );
    }

    #[test]
    fn a_list_written_for_another_directory_is_rebased() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir(root.join(".git")).unwrap();
        write(
            root,
            &[
                ("src/a.vhd", "entity a is end;"),
                ("src/b.vhd", "entity b is end;"),
                (
                    "vhdl_ls.toml",
                    "[libraries]\nmylib.files = ['old/src/*.vhd', 'old/src/b.vhd', 'gone.vhd']\n",
                ),
            ],
        );
        let project = Project::load(crate::find_root(root)).unwrap();
        assert_eq!(libraries_of(&project, root, "src/a.vhd"), ["mylib"]);
        assert_eq!(libraries_of(&project, root, "src/b.vhd"), ["mylib"]);
        let notes = project.notes.join("\n");
        assert!(
            notes.contains("2 entries match no source as written"),
            "{notes}"
        );
        assert!(notes.contains("`old/src/*.vhd` as `src/*.vhd`"), "{notes}");
        assert!(
            notes.contains("1 entry matches no source (gone.vhd)"),
            "{notes}"
        );
    }

    #[test]
    fn inferring_again_keeps_the_decision() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir(root.join(".git")).unwrap();
        write(
            root,
            &[
                ("rtl/pkg.vhd", "package pkg is end;"),
                (
                    "tb/tb.vhd",
                    "library good; use good.pkg.all; entity tb is end;",
                ),
                ("a_old.tcl", "read_vhdl -library stale [glob rtl/*.vhd]\n"),
                ("build.tcl", "read_vhdl -library good [glob rtl/*.vhd]\n"),
            ],
        );
        let mut project = Project::load(crate::find_root(root)).unwrap();
        let sources = project.library_sources().unwrap();
        project.infer_with(&sources);
        let first = project.libraries.clone();
        let conflicts = project.library_conflicts.clone();
        // As on an edit: the lists are kept, the decision too.
        project.infer_with(&sources);
        assert_eq!(project.libraries, first);
        assert_eq!(project.library_conflicts, conflicts);
        assert_eq!(libraries_of(&project, root, "rtl/pkg.vhd"), ["good"]);
    }
}

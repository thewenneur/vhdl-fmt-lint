//! Loading a whole project: reading and parsing every source in parallel,
//! then inferring libraries.

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::SystemTime;

use rayon::prelude::*;
use vfl_config::{Config, VhdlLsMode};
use vfl_syntax::{Parse, Standard};

use crate::explicit::{LibraryGlobs, VHDL_LS_FILES, path_to_slash, vhdl_ls_libraries};
use crate::infer::{Inference, Reason, infer_libraries, library_conflicts};
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
    pub fn new(path: PathBuf, text: String, encoding: Encoding, standard: Standard) -> Self {
        let parse = vfl_syntax::parse(&text, standard);
        let summary = summarize(&parse.syntax());
        SourceFile {
            path,
            text: text.into(),
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
    /// Files of other tools that may list libraries (VUnit `run.py`,
    /// FuseSoC `.core`, Vivado and Quartus projects), read on each
    /// inference.
    pub tool_files: Vec<PathBuf>,
}

/// The library lists of a project, as [`Project::library_sources`] read
/// them, with when each file they come from was last modified.
pub struct LibrarySources {
    /// The config's list, if any, then those of tool files.
    globs: Vec<LibraryGlobs>,
    /// How many of `globs` come from the config.
    config_lists: usize,
    /// `vhdl_ls.toml`'s list, unless `[project]` `vhdl-ls` is off.
    vhdl_ls: Option<LibraryGlobs>,
    vhdl_ls_mode: VhdlLsMode,
    /// In fallback mode, when ranking `vhdl_ls.toml` above inference left
    /// fewer library conflicts, the note saying so. Decided on the first
    /// inference with these lists and kept, so that later ones run
    /// inference once.
    vhdl_ls_first: OnceLock<Option<String>>,
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
        let (paths, tool_files) =
            collect_project(&root.dir, &config).map_err(|source| LoadError::Walk {
                path: root.dir.clone(),
                source,
            })?;
        Self::from_parts(root, config, paths, tool_files)
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
        Self::from_parts(root, config, paths, tool_files)
    }

    fn from_parts(
        root: ProjectRoot,
        config: Config,
        paths: Vec<PathBuf>,
        tool_files: Vec<PathBuf>,
    ) -> Result<Project, LoadError> {
        let standard = config.standard();
        // With `[verilog]` `mode` off, Verilog files are left out, not
        // read as VHDL.
        let results: Vec<_> = paths
            .into_par_iter()
            .filter_map(|path| {
                let read = match verilog_standard(&config, &path) {
                    Some(_) if !config.verilog.loaded() => return None,
                    Some(std) => VerilogFile::read(path.clone(), &config, &root.dir, std)
                        .map(Loaded::Verilog),
                    None => SourceFile::read(path.clone(), standard).map(Loaded::Vhdl),
                };
                Some(read.map_err(|err| (path, err.to_string())))
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
            tool_files,
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
        let mut globs = Vec::new();
        if !self.config.project.libraries.is_empty() {
            let source = vfl_config::find_config(&root)
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "vfl.toml".into());
            globs.push(
                LibraryGlobs::new(source.clone(), &root, &root, &self.config.project.libraries)
                    .map_err(|error| LoadError::Glob {
                        source_file: source,
                        error,
                    })?,
            );
        }
        let vhdl_ls = match self.config.project.vhdl_ls {
            VhdlLsMode::Off => None,
            VhdlLsMode::Fallback | VhdlLsMode::Prefer => vhdl_ls_libraries(&root),
        }
        .and_then(|(path, libraries)| {
            let source = path.display().to_string();
            let base = path.parent().unwrap_or(&root).to_owned();
            // A broken vhdl_ls.toml is not ours to reject; skip it.
            LibraryGlobs::new(source, &base, &root, &libraries).ok()
        });
        let config_lists = globs.len();
        for (path, libraries) in crate::import::import_all(&self.tool_files) {
            let source = path.display().to_string();
            let base = path.parent().unwrap_or(&root).to_owned();
            if let Ok(g) = LibraryGlobs::new_strict(source, &base, &root, &libraries) {
                globs.push(g);
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
        Ok(LibrarySources {
            globs,
            config_lists,
            vhdl_ls,
            vhdl_ls_mode: self.config.project.vhdl_ls,
            vhdl_ls_first: OnceLock::new(),
            stamps,
        })
    }

    /// Computes every file's libraries from `sources` and what the files
    /// declare and refer to.
    pub fn infer_with(&mut self, sources: &LibrarySources) {
        let root = &self.root.dir;
        // Verilog files take part like VHDL files, after them.
        let paths: Vec<PathBuf> = self
            .files
            .iter()
            .map(|f| f.path.clone())
            .chain(self.verilog.iter().map(|f| f.path.clone()))
            .collect();
        let relative: Vec<String> = paths
            .iter()
            .map(|path| {
                path.strip_prefix(root)
                    .map(path_to_slash)
                    .unwrap_or_else(|_| path_to_slash(path))
            })
            .collect();
        let assign = |lists: &[&LibraryGlobs]| -> Vec<Option<Vec<(String, Reason)>>> {
            if lists.is_empty() {
                return vec![None; relative.len()];
            }
            relative
                .iter()
                .map(|r| lists.iter().find_map(|s| s.assign(r)))
                .collect()
        };
        // Preferred, `vhdl_ls.toml` ranks below the config and above tool
        // files.
        let (config, tools) = sources.globs.split_at(sources.config_lists);
        let preferred: Vec<&LibraryGlobs> =
            config.iter().chain(&sources.vhdl_ls).chain(tools).collect();
        let others: Vec<&LibraryGlobs> = sources.globs.iter().collect();
        let summaries: Vec<FileSummary> = self
            .files
            .iter()
            .map(|f| f.summary.clone())
            .chain(self.verilog.iter().map(|f| f.summary.clone()))
            .collect();
        let none = vec![None; paths.len()];
        let Inference {
            mut libraries,
            notes,
        } = match (&sources.vhdl_ls, sources.vhdl_ls_mode) {
            (Some(vhdl_ls), VhdlLsMode::Fallback) => {
                let infer =
                    || infer_libraries(&paths, &summaries, &assign(&others), &assign(&[vhdl_ls]));
                let listed = || infer_libraries(&paths, &summaries, &assign(&preferred), &none);
                let note = |listed: usize, inferred: usize| {
                    format!(
                        "{} ranks above library inference: it leaves {listed} library \
                         conflicts (unresolved references, units declared twice in a \
                         library), inference {inferred}. Set [project] vhdl-ls = \"prefer\" \
                         to always rank it so.",
                        vhdl_ls.source()
                    )
                };
                match sources.vhdl_ls_first.get() {
                    Some(None) => infer(),
                    Some(Some(note)) => {
                        let mut listed = listed();
                        listed.notes.push(note.clone());
                        listed
                    }
                    None => {
                        let inferred = infer();
                        let conflicts = library_conflicts(&summaries, &inferred.libraries);
                        // Inference leaves conflicts: rank vhdl_ls.toml
                        // above it instead if that leaves fewer.
                        let better = (conflicts > 0)
                            .then(listed)
                            .map(|mut listed| {
                                let left = library_conflicts(&summaries, &listed.libraries);
                                listed.notes.push(note(left, conflicts));
                                (listed, left)
                            })
                            .filter(|(_, left)| *left < conflicts);
                        let _ = sources.vhdl_ls_first.set(
                            better
                                .as_ref()
                                .and_then(|(listed, _)| listed.notes.last().cloned()),
                        );
                        better.map_or(inferred, |(listed, _)| listed)
                    }
                }
            }
            (Some(_), _) => infer_libraries(&paths, &summaries, &assign(&preferred), &none),
            (None, _) => infer_libraries(&paths, &summaries, &assign(&others), &none),
        };
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
        self.verilog.iter().position(|f| f.path == path)
    }

    /// The index of the file at `path`.
    pub fn file_index(&self, path: &Path) -> Option<usize> {
        self.files.iter().position(|f| f.path == path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RootKind;
    use std::fs;

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
        assert!(project.notes[0].contains("ranks above library inference"));

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
}

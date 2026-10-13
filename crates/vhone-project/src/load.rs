//! Loading a whole project: reading and parsing every source in parallel,
//! then inferring libraries.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use rayon::prelude::*;
use vhone_config::Config;
use vhone_syntax::{Parse, Standard};

use crate::explicit::{LibraryGlobs, VHDL_LS_FILES, path_to_slash, vhdl_ls_libraries};
use crate::infer::{Inference, Reason, infer_libraries};
use crate::summary::{FileSummary, summarize};
use crate::{ProjectRoot, collect};

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
        let parse = vhone_syntax::parse(&text, standard);
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

/// A loaded project: its root, config, files and their libraries.
#[derive(Debug, Clone)]
pub struct Project {
    pub root: ProjectRoot,
    pub config: Config,
    pub files: Vec<SourceFile>,
    /// Libraries of each file, parallel to `files`.
    pub libraries: Vec<Vec<(String, Reason)>>,
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
    globs: Vec<LibraryGlobs>,
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
    Config(#[from] vhone_config::ConfigError),
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
        let config = vhone_config::load_for_dir(&root.dir)?;
        let (paths, tool_files) =
            collect(&root.dir, &config.project).map_err(|source| LoadError::Walk {
                path: root.dir.clone(),
                source,
            })?;
        Self::from_parts(root, config, paths, tool_files)
    }

    /// Loads the given files as a project rooted at `root`.
    pub fn from_paths(
        root: ProjectRoot,
        config: Config,
        paths: Vec<PathBuf>,
    ) -> Result<Project, LoadError> {
        let tool_files = collect(&root.dir, &config.project)
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
        let results: Vec<_> = paths
            .into_par_iter()
            .map(|path| match SourceFile::read(path.clone(), standard) {
                Ok(file) => Ok(file),
                Err(err) => Err((path, err.to_string())),
            })
            .collect();
        let mut files = Vec::new();
        let mut unreadable = Vec::new();
        for result in results {
            match result {
                Ok(file) => files.push(file),
                Err(err) => unreadable.push(err),
            }
        }
        let mut project = Project {
            root,
            config,
            files,
            libraries: Vec::new(),
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
    /// config's, `vhdl_ls.toml` and those of tool files.
    pub fn library_sources(&self) -> Result<LibrarySources, LoadError> {
        let root = self.root.dir.clone();
        let mut globs = Vec::new();
        if !self.config.project.libraries.is_empty() {
            let source = vhone_config::find_config(&root)
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "vhone.toml".into());
            globs.push(
                LibraryGlobs::new(source.clone(), &root, &root, &self.config.project.libraries)
                    .map_err(|error| LoadError::Glob {
                        source_file: source,
                        error,
                    })?,
            );
        }
        if let Some((path, libraries)) = vhdl_ls_libraries(&root) {
            let source = path.display().to_string();
            let base = path.parent().unwrap_or(&root).to_owned();
            // A broken vhdl_ls.toml is not ours to reject; skip it.
            if let Ok(g) = LibraryGlobs::new(source, &base, &root, &libraries) {
                globs.push(g);
            }
        }
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
        Ok(LibrarySources { globs, stamps })
    }

    /// Computes every file's libraries from `sources` and what the files
    /// declare and refer to.
    pub fn infer_with(&mut self, sources: &LibrarySources) {
        let root = &self.root.dir;
        let explicit: Vec<Option<Vec<(String, Reason)>>> = self
            .files
            .iter()
            .map(|file| {
                let relative = file
                    .path
                    .strip_prefix(root)
                    .map(path_to_slash)
                    .unwrap_or_else(|_| path_to_slash(&file.path));
                sources.globs.iter().find_map(|s| s.assign(&relative))
            })
            .collect();
        let paths: Vec<PathBuf> = self.files.iter().map(|f| f.path.clone()).collect();
        let summaries: Vec<FileSummary> = self.files.iter().map(|f| f.summary.clone()).collect();
        let Inference { libraries, notes } = infer_libraries(&paths, &summaries, &explicit);
        self.libraries = libraries;
        self.notes = notes;
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
            root.join("vhone.toml"),
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

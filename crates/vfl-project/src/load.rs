//! Loading a whole project: reading and parsing every source in parallel,
//! then inferring libraries.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rayon::prelude::*;
use vfl_config::Config;
use vfl_syntax::{Parse, Standard};

use crate::explicit::{LibraryGlobs, path_to_slash, vhdl_ls_libraries};
use crate::infer::{Inference, Reason, infer_libraries};
use crate::summary::{FileSummary, summarize};
use crate::{ProjectRoot, collect_sources};

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
        let config = vfl_config::load_for_dir(&root.dir)?;
        let paths =
            collect_sources(&root.dir, &config.project).map_err(|source| LoadError::Walk {
                path: root.dir.clone(),
                source,
            })?;
        Self::from_paths(root, config, paths)
    }

    /// Loads the given files as a project rooted at `root`.
    pub fn from_paths(
        root: ProjectRoot,
        config: Config,
        paths: Vec<PathBuf>,
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
        };
        project.infer()?;
        Ok(project)
    }

    /// (Re)computes every file's libraries.
    pub fn infer(&mut self) -> Result<(), LoadError> {
        let root = self.root.dir.clone();
        let mut sources = Vec::new();
        if !self.config.project.libraries.is_empty() {
            let source = vfl_config::find_config(&root)
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "vfl.toml".into());
            sources.push(
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
            if let Ok(globs) = LibraryGlobs::new(source, &base, &root, &libraries) {
                sources.push(globs);
            }
        }
        let explicit: Vec<Option<Vec<(String, Reason)>>> = self
            .files
            .iter()
            .map(|file| {
                let relative = file
                    .path
                    .strip_prefix(&root)
                    .map(path_to_slash)
                    .unwrap_or_else(|_| path_to_slash(&file.path));
                sources.iter().find_map(|s| s.assign(&relative))
            })
            .collect();
        let paths: Vec<PathBuf> = self.files.iter().map(|f| f.path.clone()).collect();
        let summaries: Vec<FileSummary> = self.files.iter().map(|f| f.summary.clone()).collect();
        let Inference { libraries, notes } = infer_libraries(&paths, &summaries, &explicit);
        self.libraries = libraries;
        self.notes = notes;
        Ok(())
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
}

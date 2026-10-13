//! VHDL project discovery: finding the project root, its sources and the
//! library of every file, without a hand-written library list. See
//! `docs/design.md`, "Project autodiscovery".

mod explicit;
mod import;
mod infer;
mod load;
mod summary;

use std::path::{Path, PathBuf};

pub use import::is_tool_file;
pub use infer::{BUNDLED_LIBRARIES, Inference, Reason, infer_libraries};
pub use load::{Encoding, LoadError, Project, SourceFile, decode, encode};
pub use summary::{
    FileSummary, UnitKind, UnitRef, UnitSummary, name_token, simple_name, summarize,
};

use ignore::WalkBuilder;
use ignore::overrides::OverrideBuilder;
use vfl_config::{ProjectConfig, find_config};

/// Directories whose presence marks a version-control root.
const VCS_MARKERS: [&str; 3] = [".git", ".hg", ".jj"];

/// Why a directory was chosen as the project root.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RootKind {
    /// It holds a `vfl.toml` or `.vfl.toml`.
    Config,
    /// It is a version-control root.
    Vcs,
    /// Nothing marked a root, so the starting directory is used.
    Fallback,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectRoot {
    pub dir: PathBuf,
    pub kind: RootKind,
}

/// Finds the project root for `path` (a file or a directory).
///
/// The nearest directory holding a config file wins; failing that, the
/// nearest version-control root; failing that, the directory itself.
pub fn find_root(path: &Path) -> ProjectRoot {
    let start = if path.is_file() {
        path.parent().unwrap_or(Path::new("."))
    } else {
        path
    };

    if let Some(dir) = start.ancestors().find(|dir| find_config(dir).is_some()) {
        return ProjectRoot {
            dir: dir.to_owned(),
            kind: RootKind::Config,
        };
    }
    if let Some(dir) = start
        .ancestors()
        .find(|dir| VCS_MARKERS.iter().any(|marker| dir.join(marker).exists()))
    {
        return ProjectRoot {
            dir: dir.to_owned(),
            kind: RootKind::Vcs,
        };
    }
    ProjectRoot {
        dir: start.to_owned(),
        kind: RootKind::Fallback,
    }
}

/// Lists the VHDL sources under `root`, sorted, honouring `.gitignore` and
/// the config's `extensions` and `exclude`.
pub fn collect_sources(root: &Path, config: &ProjectConfig) -> Result<Vec<PathBuf>, ignore::Error> {
    Ok(collect(root, config)?.0)
}

/// Lists the VHDL sources under `root` and the files of other tools that
/// may list libraries (`run.py`, `.core`, `.xpr`, `.tcl`, `.qsf`, `.qip`),
/// both sorted.
pub fn collect(
    root: &Path,
    config: &ProjectConfig,
) -> Result<(Vec<PathBuf>, Vec<PathBuf>), ignore::Error> {
    let mut overrides = OverrideBuilder::new(root);
    for pattern in &config.exclude {
        overrides.add(&format!("!{pattern}"))?;
    }

    let mut sources = Vec::new();
    let mut tool_files = Vec::new();
    for entry in WalkBuilder::new(root)
        .overrides(overrides.build()?)
        .require_git(false)
        .build()
    {
        let entry = entry?;
        let path = entry.path();
        let is_source = entry.file_type().is_some_and(|t| t.is_file())
            && path
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|ext| {
                    config
                        .extensions
                        .iter()
                        .any(|wanted| wanted.eq_ignore_ascii_case(ext))
                });
        if is_source {
            sources.push(path.to_owned());
        } else if entry.file_type().is_some_and(|t| t.is_file()) && import::is_tool_file(path) {
            tool_files.push(path.to_owned());
        }
    }
    sources.sort();
    tool_files.sort();
    Ok((sources, tool_files))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn touch(path: &Path) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, "").unwrap();
    }

    #[test]
    fn config_file_marks_the_root() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir(root.join(".git")).unwrap();
        touch(&root.join("hw/vfl.toml"));
        touch(&root.join("hw/rtl/top.vhd"));

        let found = find_root(&root.join("hw/rtl/top.vhd"));
        assert_eq!(found.dir, root.join("hw"));
        assert_eq!(found.kind, RootKind::Config);
    }

    #[test]
    fn vcs_root_is_used_without_a_config() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir(root.join(".git")).unwrap();
        touch(&root.join("rtl/top.vhd"));

        let found = find_root(&root.join("rtl"));
        assert_eq!(found.dir, root);
        assert_eq!(found.kind, RootKind::Vcs);
    }

    #[test]
    fn sources_respect_extensions_gitignore_and_exclude() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        touch(&root.join("rtl/a.vhd"));
        touch(&root.join("rtl/B.VHDL"));
        touch(&root.join("rtl/notes.txt"));
        touch(&root.join("build/gen.vhd"));
        touch(&root.join("sim/generated/tb_gen.vhd"));
        touch(&root.join("sim/tb.vhd"));
        fs::write(root.join(".gitignore"), "build/\n").unwrap();

        let config = ProjectConfig {
            exclude: vec!["sim/generated/**".into()],
            ..ProjectConfig::default()
        };
        let found: Vec<_> = collect_sources(root, &config)
            .unwrap()
            .into_iter()
            .map(|p| p.strip_prefix(root).unwrap().to_owned())
            .collect();
        assert_eq!(
            found,
            vec![
                PathBuf::from("rtl/B.VHDL"),
                PathBuf::from("rtl/a.vhd"),
                PathBuf::from("sim/tb.vhd"),
            ]
        );
    }
}

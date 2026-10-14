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
pub use load::{Encoding, LibrarySources, LoadError, Project, SourceFile, decode, encode};
pub use summary::{
    FileSummary, UnitKind, UnitRef, UnitSummary, name_token, simple_name, summarize,
};

use ignore::WalkBuilder;
use ignore::overrides::OverrideBuilder;
use vfl_config::{ProjectConfig, config_chain, find_config};

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
/// The outermost directory holding a config file wins, so that nested
/// configs, which only override settings for their subtree, do not split
/// the project. The search stops at a config with `root = true` and does not
/// go above the nearest version-control root. Without a config, the nearest
/// version-control root is used; failing that, the directory itself.
pub fn find_root(path: &Path) -> ProjectRoot {
    let start = if path.is_file() {
        path.parent().unwrap_or(Path::new("."))
    } else {
        path
    };
    let vcs = start.ancestors().find(|dir| is_vcs_root(dir));

    let within_vcs = |dir: &Path| vcs.is_none_or(|vcs| dir.starts_with(vcs));
    let config_dir = match config_chain(start) {
        Ok(chain) => chain
            .iter()
            .filter_map(|file| file.parent())
            .find(|dir| within_vcs(dir))
            .map(Path::to_owned),
        // An unreadable config is reported when it is loaded; the nearest
        // one still marks the root.
        Err(_) => start
            .ancestors()
            .find(|dir| find_config(dir).is_some())
            .map(Path::to_owned),
    };
    if let Some(dir) = config_dir {
        return ProjectRoot {
            dir,
            kind: RootKind::Config,
        };
    }
    if let Some(dir) = vcs {
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

/// Whether `dir` is a version-control root. A git submodule is not one: it
/// belongs to the enclosing superproject, unless a config with `root = true`
/// makes it a project of its own.
fn is_vcs_root(dir: &Path) -> bool {
    VCS_MARKERS.iter().any(|marker| {
        let path = dir.join(marker);
        match path.symlink_metadata() {
            Ok(meta) if meta.is_file() => !is_submodule_gitlink(&path),
            Ok(_) => true,
            Err(_) => false,
        }
    })
}

/// Whether the `.git` file at `path` points into a superproject's
/// `.git/modules/`, as a submodule's does (a linked worktree's points into
/// `.git/worktrees/` instead, and is a root of its own).
fn is_submodule_gitlink(path: &Path) -> bool {
    std::fs::read_to_string(path).is_ok_and(|text| {
        text.lines()
            .find_map(|line| line.strip_prefix("gitdir:"))
            .is_some_and(|gitdir| gitdir.trim().replace('\\', "/").contains(".git/modules/"))
    })
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
    fn nested_configs_do_not_split_the_project() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir(root.join(".git")).unwrap();
        touch(&root.join("hw/vfl.toml"));
        touch(&root.join("hw/tb/vfl.toml"));
        touch(&root.join("hw/tb/tb.vhd"));

        let found = find_root(&root.join("hw/tb/tb.vhd"));
        assert_eq!(found.dir, root.join("hw"));
        assert_eq!(found.kind, RootKind::Config);
    }

    #[test]
    fn root_true_and_vcs_roots_bound_the_search() {
        let dir = tempfile::tempdir().unwrap();
        let outer = dir.path();
        // A config above the repository is layered but is not the root.
        touch(&outer.join("vfl.toml"));
        let repo = outer.join("repo");
        fs::create_dir_all(repo.join(".git")).unwrap();
        touch(&repo.join("a/vfl.toml"));
        touch(&repo.join("a/b/c/x.vhd"));
        fs::write(repo.join("a/b/vfl.toml"), "root = true\n").unwrap();
        touch(&repo.join("d/y.vhd"));

        assert_eq!(find_root(&repo.join("a/b/c/x.vhd")).dir, repo.join("a/b"));
        let found = find_root(&repo.join("d/y.vhd"));
        assert_eq!(found.dir, repo);
        assert_eq!(found.kind, RootKind::Vcs);
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

    /// A superproject with an IP as a git submodule, laid out as `git
    /// submodule add` leaves it: the submodule's `.git` is a file.
    fn superproject(root: &Path) -> PathBuf {
        fs::create_dir_all(root.join(".git/modules/ip")).unwrap();
        touch(&root.join("rtl/top.vhd"));
        let ip = root.join("third_party/ip");
        touch(&ip.join("rtl/fifo.vhd"));
        fs::write(ip.join(".git"), "gitdir: ../../.git/modules/ip\n").unwrap();
        ip
    }

    #[test]
    fn a_submodule_belongs_to_its_superproject() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let ip = superproject(root);

        let found = find_root(&ip.join("rtl/fifo.vhd"));
        assert_eq!(found.dir, root);
        assert_eq!(found.kind, RootKind::Vcs);

        touch(&root.join("vfl.toml"));
        let found = find_root(&ip.join("rtl/fifo.vhd"));
        assert_eq!(found.dir, root);
        assert_eq!(found.kind, RootKind::Config);

        let sources = collect_sources(root, &ProjectConfig::default()).unwrap();
        assert!(sources.contains(&ip.join("rtl/fifo.vhd")));
    }

    #[test]
    fn root_true_makes_a_submodule_its_own_project() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let ip = superproject(root);
        touch(&root.join("vfl.toml"));
        fs::write(ip.join("vfl.toml"), "root = true\n").unwrap();

        let found = find_root(&ip.join("rtl/fifo.vhd"));
        assert_eq!(found.dir, ip);
        assert_eq!(found.kind, RootKind::Config);
    }

    #[test]
    fn a_linked_worktree_is_its_own_root() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join(".git/worktrees/wt")).unwrap();
        let wt = root.join("wt");
        touch(&wt.join("rtl/top.vhd"));
        fs::write(wt.join(".git"), "gitdir: /repo/.git/worktrees/wt\n").unwrap();

        assert_eq!(find_root(&wt.join("rtl")).dir, wt);
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

//! VHDL project discovery: finding the project root, its sources and the
//! library of every file, without a hand-written library list. See
//! `docs/design.md`, "Project autodiscovery".

mod explicit;
mod import;
mod infer;
mod load;
mod order;
mod summary;
mod verilog;

use std::path::{Path, PathBuf};

pub use import::is_tool_file;
pub use infer::{BUNDLED_LIBRARIES, Inference, Reason, infer_libraries, library_conflicts};
pub use load::{
    Encoding, LibrarySources, LoadError, PathIndex, Project, SourceFile, decode, encode,
};
pub use order::{CompileOrder, CompileStep, Language, OrderError, compile_order};
pub use summary::{
    FileSummary, UnitKind, UnitRef, UnitSummary, name_token, simple_name, summarize,
};
pub use verilog::{VerilogFile, module_key, parse_verilog, verilog_standard, with_verilog_options};

use ignore::WalkBuilder;
use ignore::overrides::OverrideBuilder;
use vfl_config::{Config, ProjectConfig, VerilogConfig, config_chain, find_config};

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
    walk(root, config, &config.extensions, true)
}

/// Lists the sources of a project under `root`, VHDL and Verilog together
/// (Verilog unless `[verilog]` `mode` is off), and the files of other tools
/// that may list libraries, in one walk, both sorted.
pub fn collect_project(
    root: &Path,
    config: &Config,
) -> Result<(Vec<PathBuf>, Vec<PathBuf>), ignore::Error> {
    walk(root, &config.project, &project_extensions(config), true)
}

/// Lists the VHDL sources and the Verilog and SystemVerilog sources under
/// `root` in one walk, each sorted, honouring `.gitignore`, the project's
/// `exclude`, its `extensions` and those of `[verilog]`. These are the
/// files to check, format or lint, so Verilog ones are listed only when
/// `[verilog]` `mode` is full.
pub fn collect_all_sources(
    root: &Path,
    project: &ProjectConfig,
    verilog: &VerilogConfig,
) -> Result<(Vec<PathBuf>, Vec<PathBuf>), ignore::Error> {
    let extensions: Vec<String> = project
        .extensions
        .iter()
        .chain(verilog_extensions(verilog, verilog.full()))
        .cloned()
        .collect();
    let (all, _) = walk(root, project, &extensions, false)?;
    Ok(all.into_iter().partition(|path| {
        path.extension()
            .and_then(|e| e.to_str())
            .is_some_and(|ext| {
                project
                    .extensions
                    .iter()
                    .any(|e| e.eq_ignore_ascii_case(ext))
            })
    }))
}

/// VHDL sources, Verilog sources and vendor files.
pub type LintSources = (Vec<PathBuf>, Vec<PathBuf>, Vec<PathBuf>);

/// Like [`collect_all_sources`], with the FPGA vendor files `vfl lint`
/// checks (`.qsf`, `.qip`, `.sdc`, `.xdc`, `.tcl`) as a third list, in the
/// same walk.
pub fn collect_lint_sources(
    root: &Path,
    project: &ProjectConfig,
    verilog: &VerilogConfig,
) -> Result<LintSources, ignore::Error> {
    let extensions: Vec<String> = project
        .extensions
        .iter()
        .chain(verilog_extensions(verilog, verilog.full()))
        .cloned()
        .chain(vfl_vendor::EXTENSIONS.iter().map(|e| e.to_string()))
        .collect();
    let (all, _) = walk(root, project, &extensions, false)?;
    let (vendor, sources): (Vec<PathBuf>, Vec<PathBuf>) = all.into_iter().partition(|path| {
        vfl_vendor::is_vendor_file(path) && !has_extension(path, &project.extensions)
    });
    let (vhdl, verilog) = sources
        .into_iter()
        .partition(|path| has_extension(path, &project.extensions));
    Ok((vhdl, verilog, vendor))
}

fn has_extension(path: &Path, extensions: &[String]) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|ext| extensions.iter().any(|e| e.eq_ignore_ascii_case(ext)))
}

/// The Verilog and SystemVerilog extensions, when `wanted`.
fn verilog_extensions(verilog: &VerilogConfig, wanted: bool) -> impl Iterator<Item = &String> {
    verilog
        .extensions
        .iter()
        .chain(&verilog.sv_extensions)
        .filter(move |_| wanted)
}

/// Lists the Verilog and SystemVerilog sources under `root`, sorted,
/// honouring `.gitignore`, the project's `exclude` and the extensions of
/// `[verilog]`.
pub fn collect_verilog_sources(
    root: &Path,
    project: &ProjectConfig,
    verilog: &VerilogConfig,
) -> Result<Vec<PathBuf>, ignore::Error> {
    let extensions: Vec<String> = verilog
        .extensions
        .iter()
        .chain(&verilog.sv_extensions)
        .cloned()
        .collect();
    Ok(walk(root, project, &extensions, false)?.0)
}

/// The sources directly in `dir`, a directory under `root` that
/// [`collect_project`] walks into, that it would list: one directory read
/// instead of a walk of the project, to tell whether new files in a
/// directory belong to it. Sorted.
pub fn collect_project_in(
    root: &Path,
    dir: &Path,
    config: &Config,
) -> Result<Vec<PathBuf>, ignore::Error> {
    let extensions = project_extensions(config);
    Ok(walk_from(root, dir, Some(1), &config.project, &extensions, false)?.0)
}

/// Every file under `root` whose change matters to the project: its
/// sources (as [`collect_project`] lists them), the files of other tools
/// that may list libraries, and config files (`vfl.toml`,
/// `vhdl_ls.toml`, hidden ones too). For watching them when the editor
/// does not. Sorted.
pub fn collect_watched(root: &Path, config: &Config) -> Result<Vec<PathBuf>, ignore::Error> {
    let extensions = project_extensions(config);
    let mut overrides = OverrideBuilder::new(root);
    for pattern in &config.project.exclude {
        overrides.add(&format!("!{pattern}"))?;
    }
    fn is_config(name: &str) -> bool {
        vfl_config::CONFIG_FILE_NAMES.contains(&name) || explicit::VHDL_LS_FILES.contains(&name)
    }
    let mut files = Vec::new();
    // Hidden files and directories are skipped, as the project's walk
    // does, except config files such as `.vfl.toml`.
    for entry in WalkBuilder::new(root)
        .overrides(overrides.build()?)
        .hidden(false)
        .require_git(false)
        .filter_entry(|e| {
            let name = e.file_name().to_str().unwrap_or_default();
            e.depth() == 0 || !name.starts_with('.') || is_config(name)
        })
        .build()
    {
        let entry = entry?;
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let path = entry.path();
        let name = entry.file_name().to_str().unwrap_or_default();
        let is_source = path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|ext| extensions.iter().any(|e| e.eq_ignore_ascii_case(ext)));
        if is_source || is_config(name) || import::is_tool_file(path) {
            files.push(entry.into_path());
        }
    }
    files.sort();
    Ok(files)
}

/// The extensions of the sources of a project, VHDL and Verilog.
fn project_extensions(config: &Config) -> Vec<String> {
    config
        .project
        .extensions
        .iter()
        .chain(verilog_extensions(&config.verilog, config.verilog.loaded()))
        .cloned()
        .collect()
}

fn walk(
    root: &Path,
    config: &ProjectConfig,
    extensions: &[String],
    tools: bool,
) -> Result<(Vec<PathBuf>, Vec<PathBuf>), ignore::Error> {
    walk_from(root, root, None, config, extensions, tools)
}

/// Walks `start`, a directory under `root`, down to `depth` levels.
fn walk_from(
    root: &Path,
    start: &Path,
    depth: Option<usize>,
    config: &ProjectConfig,
    extensions: &[String],
    tools: bool,
) -> Result<(Vec<PathBuf>, Vec<PathBuf>), ignore::Error> {
    let mut overrides = OverrideBuilder::new(root);
    for pattern in &config.exclude {
        overrides.add(&format!("!{pattern}"))?;
    }

    let mut sources = Vec::new();
    let mut tool_files = Vec::new();
    for entry in WalkBuilder::new(start)
        .overrides(overrides.build()?)
        .require_git(false)
        .max_depth(depth)
        .build()
    {
        let entry = entry?;
        let path = entry.path();
        let is_file = entry.file_type().is_some_and(|t| t.is_file());
        let is_source = is_file
            && path
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|ext| {
                    extensions
                        .iter()
                        .any(|wanted| wanted.eq_ignore_ascii_case(ext))
                });
        if is_source {
            sources.push(path.to_owned());
        } else if tools && is_file && import::is_tool_file(path) {
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
    fn verilog_sources_are_collected_apart() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        touch(&root.join("rtl/a.vhd"));
        touch(&root.join("rtl/b.v"));
        touch(&root.join("rtl/c.SV"));
        touch(&root.join("rtl/defs.svh"));
        touch(&root.join("gen/d.sv"));
        let config = ProjectConfig {
            exclude: vec!["gen/**".into()],
            ..ProjectConfig::default()
        };
        let found: Vec<_> = collect_verilog_sources(root, &config, &VerilogConfig::default())
            .unwrap()
            .into_iter()
            .map(|p| p.strip_prefix(root).unwrap().to_owned())
            .collect();
        assert_eq!(
            found,
            vec![
                PathBuf::from("rtl/b.v"),
                PathBuf::from("rtl/c.SV"),
                PathBuf::from("rtl/defs.svh"),
            ]
        );
        let vhdl = collect_sources(root, &config).unwrap();
        assert_eq!(vhdl.len(), 1);
        // The Verilog files to check, format and lint: with
        // `[verilog]` `mode = "full"` only.
        let (vhdl_too, verilog) =
            collect_all_sources(root, &config, &VerilogConfig::default()).unwrap();
        assert_eq!(vhdl_too, vhdl);
        assert!(verilog.is_empty());
        let full = VerilogConfig {
            mode: vfl_config::VerilogMode::Full,
            ..VerilogConfig::default()
        };
        let (vhdl_too, verilog) = collect_all_sources(root, &config, &full).unwrap();
        assert_eq!(vhdl_too, vhdl);
        assert_eq!(verilog.len(), 3);
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

    #[test]
    fn one_directory_lists_what_the_walk_would() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        touch(&root.join("rtl/a.vhd"));
        touch(&root.join("rtl/a_netlist.vhd"));
        touch(&root.join("rtl/.hidden.vhd"));
        touch(&root.join("rtl/sub/deeper.vhd"));
        touch(&root.join("rtl/skip_me.vhd"));
        fs::write(root.join(".gitignore"), "*_netlist.vhd\n").unwrap();
        let config = Config {
            project: ProjectConfig {
                exclude: vec!["rtl/skip_*".into()],
                ..ProjectConfig::default()
            },
            ..Config::default()
        };
        let all = collect_project(root, &config).unwrap().0;
        let found = collect_project_in(root, &root.join("rtl"), &config).unwrap();
        assert_eq!(found, [root.join("rtl/a.vhd")]);
        assert_eq!(
            all,
            [root.join("rtl/a.vhd"), root.join("rtl/sub/deeper.vhd")]
        );
    }

    #[test]
    fn watched_files_include_configs_and_tool_files() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        touch(&root.join("rtl/a.vhd"));
        touch(&root.join("rtl/.vfl.toml"));
        touch(&root.join("vhdl_ls.toml"));
        touch(&root.join("run.py"));
        touch(&root.join("other.toml"));
        touch(&root.join(".git/config.toml"));
        touch(&root.join("ignored/vfl.toml"));
        fs::write(root.join(".gitignore"), "ignored/\n").unwrap();
        let found: Vec<_> = collect_watched(root, &Config::default())
            .unwrap()
            .into_iter()
            .map(|p| p.strip_prefix(root).unwrap().to_owned())
            .collect();
        assert_eq!(
            found,
            [
                PathBuf::from("rtl/.vfl.toml"),
                PathBuf::from("rtl/a.vhd"),
                PathBuf::from("run.py"),
                PathBuf::from("vhdl_ls.toml"),
            ]
        );
    }
}

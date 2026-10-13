//! `vhone lint`.

use std::collections::HashMap;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use vhone_config::{Config, Level};
use vhone_lint::report::{self, Format, Source};
use vhone_lint::{Diagnostic, FileSettings};
use vhone_project::{Project, collect_sources, encode, find_root};

use crate::ConfigCache;

pub struct Options {
    pub paths: Vec<PathBuf>,
    pub fix: bool,
    pub unsafe_fixes: bool,
    pub format: Format,
    pub deny_warnings: bool,
    pub color: Option<bool>,
}

/// The files to report on, grouped by project root.
fn requested(paths: &[PathBuf]) -> Result<Vec<(PathBuf, Vec<PathBuf>)>> {
    let default = [PathBuf::from(".")];
    let paths = if paths.is_empty() {
        &default[..]
    } else {
        paths
    };
    let mut groups: Vec<(PathBuf, Vec<PathBuf>)> = Vec::new();
    for path in paths {
        if !path.exists() {
            bail!("{}: no such file or directory", path.display());
        }
        let path = std::path::absolute(path)?;
        let root = find_root(&path);
        let files = if path.is_dir() {
            let config = vhone_config::load_for_dir(&path)?;
            collect_sources(&path, &config.project)
                .with_context(|| format!("scanning {}", path.display()))?
        } else {
            vec![path.clone()]
        };
        match groups.iter_mut().find(|(r, _)| *r == root.dir) {
            Some((_, list)) => list.extend(files),
            None => groups.push((root.dir, files)),
        }
    }
    for (_, files) in &mut groups {
        files.sort();
        files.dedup();
    }
    Ok(groups)
}

/// Loads the project at `root`, adding `extra` files it does not include.
fn load(root: &Path, extra: &[PathBuf]) -> Result<Project> {
    let root = find_root(root);
    let config = vhone_config::load_for_dir(&root.dir)?;
    let mut paths = collect_sources(&root.dir, &config.project)
        .with_context(|| format!("scanning {}", root.dir.display()))?;
    for file in extra {
        if !paths.contains(file) {
            paths.push(file.clone());
        }
    }
    Ok(Project::from_paths(root, config, paths)?)
}

fn display_path(path: &Path, cwd: &Path) -> PathBuf {
    path.strip_prefix(cwd)
        .map_or_else(|_| path.to_owned(), Path::to_owned)
}

#[derive(Default)]
struct Totals {
    errors: usize,
    warnings: usize,
    infos: usize,
    fixed: usize,
    fixable: usize,
}

pub fn run(options: Options) -> Result<bool> {
    let cwd = std::env::current_dir()?;
    let color = options.color.unwrap_or_else(|| {
        std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none()
    });
    let mut totals = Totals::default();
    let mut output = String::new();
    let mut all: Vec<(Diagnostic, usize)> = Vec::new();
    let mut sources: Vec<(PathBuf, std::sync::Arc<str>, vhone_syntax::LineIndex)> = Vec::new();
    let mut configs = ConfigCache::default();
    for (root, files) in requested(&options.paths)? {
        let mut passes = 0;
        loop {
            let project = load(&root, &files)?;
            for (path, error) in &project.unreadable {
                if files.contains(path) {
                    bail!("reading {}: {error}", display_path(path, &cwd).display());
                }
            }
            let analysis = vhone_lint::analyze(&project);
            let mut file_configs: Vec<(usize, Config)> = Vec::new();
            for (i, file) in project.files.iter().enumerate() {
                if files.contains(&file.path) {
                    file_configs.push((i, configs.for_file(&file.path)?.clone()));
                }
            }
            let settings: Vec<FileSettings> = file_configs
                .iter()
                .filter_map(|(i, config)| {
                    let file = &project.files[*i];
                    Some(FileSettings {
                        file: analysis.file_id(&file.path)?,
                        syntax_errors: file.parse.errors(),
                        config,
                    })
                })
                .collect();
            let result = vhone_lint::lint(&analysis, &settings);
            if passes == 0 {
                for error in &result.config_errors {
                    eprintln!("warning: {error}");
                }
            }
            let fixable = |d: &Diagnostic| {
                d.fix
                    .as_ref()
                    .is_some_and(|f| f.safe || options.unsafe_fixes)
            };
            if options.fix && passes < 4 {
                let mut by_file: HashMap<u32, Vec<&vhone_lint::Fix>> = HashMap::new();
                for d in result.diagnostics.iter().filter(|d| fixable(d)) {
                    by_file
                        .entry(d.file)
                        .or_default()
                        .push(d.fix.as_ref().unwrap());
                }
                let mut applied_any = false;
                for (file, fixes) in by_file {
                    let analyzed = &analysis.files()[file as usize];
                    let (text, applied) = vhone_lint::apply_fixes(&analyzed.text, fixes);
                    if applied == 0 || text == *analyzed.text {
                        continue;
                    }
                    let encoding = project
                        .file_index(&analyzed.path)
                        .map(|i| project.files[i].encoding)
                        .unwrap_or(vhone_project::Encoding::Utf8);
                    std::fs::write(&analyzed.path, encode(&text, encoding))
                        .with_context(|| format!("writing {}", analyzed.path.display()))?;
                    totals.fixed += applied;
                    applied_any = true;
                }
                passes += 1;
                if applied_any {
                    continue;
                }
            }
            for d in result.diagnostics {
                let file = &analysis.files()[d.file as usize];
                if d.fix
                    .as_ref()
                    .is_some_and(|f| f.safe || options.unsafe_fixes)
                {
                    totals.fixable += 1;
                }
                match d.level {
                    Level::Error => totals.errors += 1,
                    Level::Warn => totals.warnings += 1,
                    _ => totals.infos += 1,
                }
                let shown = display_path(&file.path, &cwd);
                let source = match sources.iter().position(|(p, _, _)| *p == shown) {
                    Some(i) => i,
                    None => {
                        sources.push((shown, file.text.clone(), file.line_index.clone()));
                        sources.len() - 1
                    }
                };
                // Messages name other files by absolute path.
                let mut d = d;
                let prefix = format!("{}{}", cwd.display(), std::path::MAIN_SEPARATOR);
                if d.message.contains(&prefix) {
                    d.message = d.message.replace(&prefix, "");
                }
                all.push((d, source));
            }
            break;
        }
    }
    let diagnostics: Vec<Diagnostic> = all.iter().map(|(d, _)| d.clone()).collect();
    output.push_str(&report::render(
        options.format,
        &diagnostics,
        |i| Source {
            path: &sources[all[i].1].0,
            text: &sources[all[i].1].1,
            line_index: &sources[all[i].1].2,
        },
        color,
    ));
    print!("{output}");
    if matches!(options.format, Format::Human | Format::Concise) {
        summary(&totals, options.fix);
    }
    Ok(totals.errors == 0 && !(options.deny_warnings && totals.warnings > 0))
}

fn plural(n: usize, word: &str) -> String {
    if n == 1 {
        format!("{n} {word}")
    } else {
        format!("{n} {word}s")
    }
}

fn summary(totals: &Totals, fix: bool) {
    let mut parts = vec![
        plural(totals.errors, "error"),
        plural(totals.warnings, "warning"),
    ];
    if totals.infos > 0 {
        parts.push(plural(totals.infos, "note"));
    }
    let mut line = parts.join(", ");
    if fix {
        line.push_str(&format!("; fixed {}", plural(totals.fixed, "problem")));
    } else if totals.fixable > 0 {
        line.push_str(&format!(
            "; {} fixable with `vhone lint --fix`",
            totals.fixable
        ));
    }
    eprintln!("{line}");
}

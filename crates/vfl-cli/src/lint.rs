//! `vfl lint`.

use std::collections::HashMap;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use vfl_config::{Config, Level};
use vfl_lint::report::{self, Format, Source};
use vfl_lint::{Diagnostic, FileSettings};
use vfl_project::{Project, collect_lint_sources, collect_project, encode, find_root};

use crate::ConfigCache;
use crate::baseline::{self, Baseline};
use crate::changed::Changed;

pub struct Options {
    pub paths: Vec<PathBuf>,
    pub fix: bool,
    pub unsafe_fixes: bool,
    /// Rules or categories whose fixes alone are applied (all if empty).
    pub fix_only: Vec<String>,
    pub format: Format,
    pub deny_warnings: bool,
    pub color: Option<bool>,
    pub baseline: Option<PathBuf>,
    pub write_baseline: Option<PathBuf>,
    pub changed_since: Option<String>,
    pub changed_lines: bool,
}

/// The files to report on, grouped by project root: HDL files, and
/// vendor files (`.qsf`, `.sdc`, `.xdc`, ...).
type Group = (PathBuf, Vec<PathBuf>, Vec<PathBuf>);

fn requested(paths: &[PathBuf]) -> Result<Vec<Group>> {
    let default = [PathBuf::from(".")];
    let paths = if paths.is_empty() {
        &default[..]
    } else {
        paths
    };
    let mut groups: Vec<Group> = Vec::new();
    for path in paths {
        if !path.exists() {
            bail!("{}: no such file or directory", path.display());
        }
        let path = std::path::absolute(path)?;
        let root = find_root(&path);
        let mut vendor = Vec::new();
        let files = if path.is_dir() {
            let config = vfl_config::load_for_dir(&path)?;
            let (mut vhdl, verilog, tools) =
                collect_lint_sources(&path, &config.project, &config.verilog)
                    .with_context(|| format!("scanning {}", path.display()))?;
            if let Some(first) = verilog.first() {
                // Says that Verilog support is in progress.
                crate::language(&config, first);
            }
            vhdl.extend(verilog);
            vendor = tools;
            vhdl
        } else if vfl_lint::vendor::is_vendor_file(&path) {
            vendor.push(path.clone());
            Vec::new()
        } else {
            let config = vfl_config::load_for_dir(path.parent().unwrap_or(&path))?;
            match crate::language(&config, &path) {
                crate::Language::Skipped => Vec::new(),
                _ => vec![path.clone()],
            }
        };
        match groups.iter_mut().find(|(r, _, _)| *r == root.dir) {
            Some((_, list, tools)) => {
                list.extend(files);
                tools.extend(vendor);
            }
            None => groups.push((root.dir, files, vendor)),
        }
    }
    for (_, files, vendor) in &mut groups {
        files.sort();
        files.dedup();
        vendor.sort();
        vendor.dedup();
    }
    Ok(groups)
}

/// Loads the project at `root`, with its Verilog files, adding `extra`
/// files it does not include.
fn load(root: &Path, extra: &[PathBuf]) -> Result<Project> {
    let root = find_root(root);
    let config = vfl_config::load_for_dir(&root.dir)?;
    let (mut paths, _) = collect_project(&root.dir, &config)
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
    baselined: usize,
}

pub fn run(options: Options) -> Result<bool> {
    let cwd = std::env::current_dir()?;
    let color = options.color.unwrap_or_else(|| {
        std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none()
    });
    for id in &options.fix_only {
        let id = id.trim_end_matches("/*");
        let known = vfl_lint::rules::exists(id)
            || vfl_lint::RULES
                .iter()
                .any(|r| r.id.split('/').next() == Some(id))
            || id == "naming";
        if !known {
            bail!("--fix-only: unknown rule `{id}`; `vfl rules` lists them");
        }
    }
    let mut totals = Totals::default();
    let mut output = String::new();
    let mut all: Vec<(Diagnostic, usize)> = Vec::new();
    let mut sources: Vec<(PathBuf, std::sync::Arc<str>, vfl_syntax::LineIndex)> = Vec::new();
    let mut configs = ConfigCache::default();
    // Baselines by path, read once.
    let mut baselines: Vec<(PathBuf, Baseline)> = Vec::new();
    let mut new_baseline: Vec<baseline::Entry> = Vec::new();
    for (root, files, vendor) in requested(&options.paths)? {
        let changed = match &options.changed_since {
            Some(rev) => Some(Changed::since(&root, rev, options.changed_lines)?),
            None => None,
        };
        let mut passes = 0;
        loop {
            let project = load(&root, &files)?;
            for (path, error) in &project.unreadable {
                if files.contains(path) {
                    bail!("reading {}: {error}", display_path(path, &cwd).display());
                }
            }
            let analysis = vfl_lint::analyze(&project);
            let mut file_configs: Vec<(usize, Config)> = Vec::new();
            for (i, file) in project.files.iter().enumerate() {
                if files.contains(&file.path) {
                    file_configs.push((i, configs.for_file(&file.path)?.clone()));
                }
            }
            // Every Verilog file is linted, for what each declares for the
            // others; only the requested ones are reported.
            // With `[verilog]` `mode = "full"` only: otherwise they are
            // there for the modules VHDL instantiates.
            let mut verilog_configs: Vec<(usize, Config)> = Vec::new();
            for (i, file) in project.verilog.iter().enumerate() {
                let config = configs.for_file(&file.path)?;
                if config.verilog.full() {
                    verilog_configs.push((i, config.clone()));
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
                        verilog: None,
                        root: Some(&project.root.dir),
                    })
                })
                .chain(verilog_configs.iter().filter_map(|(i, config)| {
                    let file = &project.verilog[*i];
                    Some(FileSettings {
                        file: analysis.file_id(&file.path)?,
                        syntax_errors: file.parse.errors(),
                        config,
                        verilog: Some(&file.parse),
                        root: Some(&project.root.dir),
                    })
                }))
                .collect();
            let mut result = vfl_lint::lint(&analysis, &settings);
            // Vendor files are linted on their own, with the design's
            // ports; their diagnostics take file ids past the analysis's.
            let mut vendor_texts: Vec<(PathBuf, std::sync::Arc<str>, vfl_syntax::LineIndex)> =
                Vec::new();
            if !vendor.is_empty() {
                let ports = vfl_lint::vendor::ports(&analysis);
                for path in &vendor {
                    let text = std::fs::read(path).with_context(|| {
                        format!("reading {}", display_path(path, &cwd).display())
                    })?;
                    let text: std::sync::Arc<str> = vfl_project::decode(text).0.into();
                    let config = configs.for_file(path)?;
                    let id = (analysis.files().len() + vendor_texts.len()) as u32;
                    let (diagnostics, errors) = vfl_lint::vendor::lint(
                        path,
                        &text,
                        config,
                        Some(&project.root.dir),
                        Some(&ports),
                        id,
                    );
                    result.diagnostics.extend(diagnostics);
                    for error in errors {
                        if !result.config_errors.contains(&error) {
                            result.config_errors.push(error);
                        }
                    }
                    let index = vfl_syntax::LineIndex::new(&text);
                    vendor_texts.push((path.clone(), text, index));
                }
            }
            let file_of = |id: u32| -> (&Path, &std::sync::Arc<str>, &vfl_syntax::LineIndex) {
                match analysis.files().get(id as usize) {
                    Some(file) => (&file.path, &file.text, &file.line_index),
                    None => {
                        let (path, text, index) =
                            &vendor_texts[id as usize - analysis.files().len()];
                        (path, text, index)
                    }
                }
            };
            result.diagnostics.retain(|d| {
                let (path, text, line_index) = file_of(d.file);
                (files.iter().chain(&vendor).any(|f| f == path))
                    && changed.as_ref().is_none_or(|changed| {
                        let line = line_index.line_col(text, d.range.start()).line;
                        changed.contains(path, line + 1)
                    })
            });
            if passes == 0 {
                for error in &result.config_errors {
                    eprintln!("warning: {error}");
                }
            }
            let fixable = |d: &Diagnostic| {
                d.fix
                    .as_ref()
                    .is_some_and(|f| f.safe || options.unsafe_fixes)
                    && (options.fix_only.is_empty()
                        || vfl_lint::rule_matches(&options.fix_only, d.rule))
            };
            if options.fix && passes < 4 {
                let mut by_file: HashMap<u32, Vec<&vfl_lint::Fix>> = HashMap::new();
                for d in result.diagnostics.iter().filter(|d| fixable(d)) {
                    by_file
                        .entry(d.file)
                        .or_default()
                        .push(d.fix.as_ref().unwrap());
                }
                let mut applied_any = false;
                for (file, fixes) in by_file {
                    let analyzed = &analysis.files()[file as usize];
                    let (text, applied) = vfl_lint::apply_fixes(&analyzed.text, fixes);
                    if applied == 0 || text == *analyzed.text {
                        continue;
                    }
                    let encoding = project
                        .file_index(&analyzed.path)
                        .map(|i| project.files[i].encoding)
                        .or_else(|| {
                            project
                                .verilog
                                .iter()
                                .find(|f| f.path == analyzed.path)
                                .map(|f| f.encoding)
                        })
                        .unwrap_or(vfl_project::Encoding::Utf8);
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
            // The baseline: `--baseline`, else the project's.
            let baseline_path = options.baseline.clone().or_else(|| {
                let configured = project.config.lint.baseline.as_ref()?;
                Some(project.root.dir.join(configured))
            });
            let baseline = match baseline_path.filter(|_| options.write_baseline.is_none()) {
                Some(path) => {
                    let path = std::path::absolute(&path)?;
                    let i = match baselines.iter().position(|(p, _)| *p == path) {
                        Some(i) => i,
                        None => {
                            baselines.push((path.clone(), Baseline::read(&path)?));
                            baselines.len() - 1
                        }
                    };
                    Some(&mut baselines[i].1)
                }
                None => None,
            };
            let mut baseline = baseline;
            for d in result.diagnostics {
                let (file_path, file_text, file_index) = file_of(d.file);
                if baseline.is_some() || options.write_baseline.is_some() {
                    let relative = display_path(file_path, &root);
                    let fingerprint = report::diagnostic_fingerprint(
                        &d,
                        &Source {
                            path: &relative,
                            text: file_text,
                            line_index: file_index,
                        },
                    );
                    if options.write_baseline.is_some() {
                        new_baseline.push(baseline::Entry {
                            file: relative.to_string_lossy().replace('\\', "/"),
                            rule: d.rule.to_owned(),
                            message: d.message.clone(),
                            fingerprint,
                        });
                        continue;
                    }
                    if baseline.as_mut().is_some_and(|b| b.take(&fingerprint)) {
                        totals.baselined += 1;
                        continue;
                    }
                }
                if fixable(&d) {
                    totals.fixable += 1;
                }
                match d.level {
                    Level::Error => totals.errors += 1,
                    Level::Warn => totals.warnings += 1,
                    _ => totals.infos += 1,
                }
                let shown = display_path(file_path, &cwd);
                let source = match sources.iter().position(|(p, _, _)| *p == shown) {
                    Some(i) => i,
                    None => {
                        sources.push((shown, file_text.clone(), file_index.clone()));
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
    if let Some(path) = &options.write_baseline {
        let count = new_baseline.len();
        baseline::write(path, new_baseline)?;
        eprintln!(
            "wrote {} to the baseline {}",
            plural(count, "finding"),
            path.display()
        );
        return Ok(true);
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
    } else if totals.baselined > 0 {
        eprintln!(
            "{} in the baseline not reported",
            plural(totals.baselined, "finding")
        );
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
    if totals.baselined > 0 {
        parts.push(format!("{} in the baseline", totals.baselined));
    }
    let mut line = parts.join(", ");
    if fix {
        line.push_str(&format!("; fixed {}", plural(totals.fixed, "problem")));
    } else if totals.fixable > 0 {
        line.push_str(&format!(
            "; {} fixable with `vfl lint --fix`",
            totals.fixable
        ));
    }
    eprintln!("{line}");
}

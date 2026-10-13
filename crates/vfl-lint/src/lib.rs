//! Configurable VHDL linter.
//!
//! [`lint`] runs every enabled rule over the files of an [`Analysis`]:
//! syntax errors, the semantic checks of `vfl-sema` (`sema/*`), style rules
//! that look at one file (`style/*`) and naming conventions (`naming/*`).
//! Each rule's level comes from the file's `[lint]` config (a preset plus
//! per-rule overrides), and `-- vfl-ignore` comments silence findings.
//! [`lint_cached`] reuses the findings of files nothing changed for. Some
//! findings carry a [`Fix`]; [`apply_fixes`] applies them. [`report`]
//! prints the result for people, tools and CI.

pub mod naming;
pub mod report;
pub mod rules;
mod style;
mod suppress;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use rayon::prelude::*;
use vfl_config::{Config, Level};
use vfl_sema::{Analysis, FileId};
use vfl_syntax::{GreenNode, SyntaxError, TextRange};

pub use rules::{NAMING_KINDS, RULES, Rule};
pub use suppress::Suppressions;
pub use vfl_sema::{Edit, Fix};

/// A problem found by a rule, before its level is applied.
#[derive(Debug, Clone)]
pub struct Finding {
    pub rule: &'static str,
    pub range: TextRange,
    pub message: String,
    pub fix: Option<Fix>,
}

impl Finding {
    pub fn new(rule: &'static str, range: TextRange, message: impl Into<String>) -> Self {
        Finding {
            rule,
            range,
            message: message.into(),
            fix: None,
        }
    }

    pub fn with_fix(mut self, fix: Fix) -> Self {
        self.fix = Some(fix);
        self
    }
}

/// A reported problem.
#[derive(Debug, Clone)]
pub struct Diagnostic {
    pub rule: &'static str,
    pub level: Level,
    pub file: FileId,
    pub range: TextRange,
    pub message: String,
    pub fix: Option<Fix>,
}

/// What [`lint`] needs to know about one file.
pub struct FileSettings<'a> {
    pub file: FileId,
    pub syntax_errors: &'a [SyntaxError],
    pub config: &'a Config,
}

/// The diagnostics of a lint run, sorted by file and position, and
/// problems with the configuration (unknown rules, bad options).
#[derive(Debug, Default)]
pub struct LintResult {
    pub diagnostics: Vec<Diagnostic>,
    pub config_errors: Vec<String>,
}

/// Lints `files` of `analysis`.
pub fn lint(analysis: &Analysis, files: &[FileSettings]) -> LintResult {
    lint_cached(analysis, files, &mut LintCache::default())
}

/// The findings of a previous lint run, per file.
#[derive(Default)]
pub struct LintCache {
    files: HashMap<PathBuf, CachedFile>,
}

struct CachedFile {
    green: GreenNode,
    config: Config,
    sema: Vec<vfl_sema::Diagnostic>,
    diagnostics: Vec<Diagnostic>,
    errors: Vec<String>,
}

/// Like [`lint`], but reuses a file's findings from `cache` when its tree,
/// its config, its semantic diagnostics and its declarations are the same
/// as in the run that filled the cache; then updates the cache.
pub fn lint_cached(
    analysis: &Analysis,
    files: &[FileSettings],
    cache: &mut LintCache,
) -> LintResult {
    let mut sema: HashMap<FileId, Vec<&vfl_sema::Diagnostic>> = HashMap::new();
    for d in analysis.diagnostics() {
        sema.entry(d.file).or_default().push(d);
    }
    // Which files can reuse their findings.
    let reuse: Vec<bool> = files
        .par_iter()
        .map(|settings| {
            let file = &analysis.files()[settings.file as usize];
            let file_sema = sema.get(&settings.file).map(Vec::as_slice).unwrap_or(&[]);
            cache.files.get(&file.path).is_some_and(|cached| {
                std::ptr::eq(&*cached.green, &*file.green)
                    && cached.config == *settings.config
                    && cached.sema.len() == file_sema.len()
                    && cached.sema.iter().zip(file_sema).all(|(a, b)| a == *b)
                    && !analysis.file_reanalyzed(settings.file)
            })
        })
        .collect();
    // The declarations of the files to lint again.
    let relint: HashSet<FileId> = files
        .iter()
        .zip(&reuse)
        .filter(|(_, reuse)| !**reuse)
        .map(|(settings, _)| settings.file)
        .collect();
    let decls: HashMap<FileId, Vec<&vfl_sema::Decl>> = relint
        .iter()
        .map(|&file| {
            let decls = analysis
                .file_units(file)
                .flat_map(|unit| analysis.unit_decls(unit))
                .collect();
            (file, decls)
        })
        .collect();
    let results: Vec<(Vec<Diagnostic>, Vec<String>, Option<CachedFile>)> = files
        .par_iter()
        .zip(&reuse)
        .map(|(settings, &reuse)| {
            let file = &analysis.files()[settings.file as usize];
            let file_sema = sema.get(&settings.file).map(Vec::as_slice).unwrap_or(&[]);
            if reuse {
                let cached = &cache.files[&file.path];
                let mut diagnostics = cached.diagnostics.clone();
                for d in &mut diagnostics {
                    d.file = settings.file;
                }
                return (diagnostics, cached.errors.clone(), None);
            }
            let (diagnostics, errors) = lint_file(
                analysis,
                settings,
                file_sema,
                decls.get(&settings.file).map(Vec::as_slice).unwrap_or(&[]),
            );
            let cached = CachedFile {
                green: file.green.clone(),
                config: settings.config.clone(),
                sema: file_sema.iter().map(|d| (*d).clone()).collect(),
                diagnostics: diagnostics.clone(),
                errors: errors.clone(),
            };
            (diagnostics, errors, Some(cached))
        })
        .collect();
    let mut out = LintResult::default();
    let mut kept: HashMap<PathBuf, CachedFile> = HashMap::new();
    for ((diagnostics, errors, cached), settings) in results.into_iter().zip(files) {
        let path = &analysis.files()[settings.file as usize].path;
        match cached {
            Some(cached) => {
                kept.insert(path.clone(), cached);
            }
            None => {
                if let Some(cached) = cache.files.remove(path) {
                    kept.insert(path.clone(), cached);
                }
            }
        }
        out.diagnostics.extend(diagnostics);
        for e in errors {
            if !out.config_errors.contains(&e) {
                out.config_errors.push(e);
            }
        }
    }
    cache.files = kept;
    out.diagnostics
        .sort_by_key(|d| (d.file, d.range.start(), d.range.end(), d.rule));
    out
}

fn lint_file(
    analysis: &Analysis,
    settings: &FileSettings,
    sema: &[&vfl_sema::Diagnostic],
    decls: &[&vfl_sema::Decl],
) -> (Vec<Diagnostic>, Vec<String>) {
    let file = &analysis.files()[settings.file as usize];
    let lint_config = &settings.config.lint;
    let mut errors: Vec<String> = rules::unknown_rules(lint_config)
        .into_iter()
        .map(|id| format!("unknown lint rule `{id}`"))
        .collect();
    let mut levels: HashMap<&str, Level> = HashMap::new();
    let mut level = |id: &str| -> Level {
        *levels
            .entry(rules::find(id).map_or("naming", |r| r.id))
            .or_insert_with(|| rules::level(lint_config, id))
    };
    let enabled_cache: HashMap<&'static str, bool> = RULES
        .iter()
        .map(|r| (r.id, rules::level(lint_config, r.id) != Level::Off))
        .collect();
    let enabled = |id: &str| enabled_cache.get(id).copied().unwrap_or(false);

    let root = file.syntax();
    let mut findings = Vec::new();
    for error in settings.syntax_errors {
        findings.push(Finding::new(
            "syntax/error",
            error.range,
            error.message.clone(),
        ));
    }
    for d in sema {
        let mut finding = Finding::new(d.code, d.range, d.message.clone());
        finding.fix = d.fix.clone();
        findings.push(finding);
    }
    let max_line_length = rules::options(lint_config, "style/line-length")
        .and_then(|o| o.get("max"))
        .and_then(|v| v.as_integer())
        .map_or(120, |v| v.max(1) as usize);
    style::check(
        &style::StyleContext {
            text: &file.text,
            root: &root,
            file_stem: file
                .path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned()),
            max_line_length,
        },
        enabled,
        &mut findings,
    );
    let (conventions, naming_errors) = naming::conventions(lint_config);
    errors.extend(naming_errors);
    naming::check(decls, &file.text, &root, &conventions, &mut findings);

    let suppressions = Suppressions::new(&root, &file.text, &file.line_index);
    let mut out = Vec::new();
    for finding in findings {
        let level = if finding.rule.starts_with("naming/") {
            rules::level(lint_config, finding.rule)
        } else {
            level(finding.rule)
        };
        if level == Level::Off {
            continue;
        }
        let line = file
            .line_index
            .line_col(&file.text, finding.range.start())
            .line;
        if suppressions.suppressed(finding.rule, line) {
            continue;
        }
        out.push(Diagnostic {
            rule: finding.rule,
            level,
            file: settings.file,
            range: finding.range,
            message: finding.message,
            fix: finding.fix,
        });
    }
    (out, errors)
}

/// Analyzes a loaded project: every readable file with its inferred
/// libraries.
pub fn analyze(project: &vfl_project::Project) -> Analysis {
    Analysis::new(analysis_inputs(project))
}

/// Analyzes a new version of a project, reusing what `previous` found for
/// the units the changes cannot affect.
pub fn reanalyze(project: &vfl_project::Project, previous: &Analysis) -> Analysis {
    previous.update(analysis_inputs(project))
}

fn analysis_inputs(project: &vfl_project::Project) -> Vec<vfl_sema::FileInput> {
    project
        .files
        .iter()
        .zip(&project.libraries)
        .map(|(file, libraries)| vfl_sema::FileInput {
            path: file.path.clone(),
            text: file.text.clone(),
            green: file.parse.green().clone(),
            libraries: libraries.iter().map(|(l, _)| l.clone()).collect(),
        })
        .collect()
}

/// Applies the fixes' edits to `text`, skipping any that overlap an edit
/// already applied. Returns the new text and how many fixes were applied.
pub fn apply_fixes<'a>(text: &str, fixes: impl IntoIterator<Item = &'a Fix>) -> (String, usize) {
    let mut fixes: Vec<&Fix> = fixes.into_iter().filter(|f| !f.edits.is_empty()).collect();
    fixes.sort_by_key(|f| f.edits.iter().map(|e| e.range.start()).min());
    let mut taken: Vec<TextRange> = Vec::new();
    let mut edits: Vec<&Edit> = Vec::new();
    let mut applied = 0;
    for fix in fixes {
        let overlaps = fix.edits.iter().any(|e| {
            taken.iter().any(|t| {
                t.start() < e.range.end() && e.range.start() < t.end()
                    || (e.range.is_empty() && t.contains_inclusive(e.range.start()))
                    || (t.is_empty() && e.range.contains_inclusive(t.start()))
            })
        });
        if overlaps {
            continue;
        }
        taken.extend(fix.edits.iter().map(|e| e.range));
        edits.extend(&fix.edits);
        applied += 1;
    }
    edits.sort_by_key(|e| (e.range.start(), e.range.end()));
    let mut out = String::with_capacity(text.len());
    let mut at = 0usize;
    for edit in edits {
        let start = usize::from(edit.range.start());
        out.push_str(&text[at..start]);
        out.push_str(&edit.text);
        at = usize::from(edit.range.end());
    }
    out.push_str(&text[at..]);
    (out, applied)
}

#[cfg(test)]
mod tests {
    use super::*;
    use vfl_syntax::TextSize;

    fn edit(start: u32, end: u32, text: &str) -> Fix {
        Fix {
            title: String::new(),
            edits: vec![Edit {
                range: TextRange::new(TextSize::from(start), TextSize::from(end)),
                text: text.into(),
            }],
            safe: true,
        }
    }

    #[test]
    fn fixes_apply_without_overlap() {
        let fixes = [edit(0, 1, "X"), edit(0, 2, "YY"), edit(4, 4, "!")];
        let (out, applied) = apply_fixes("abcd", &fixes);
        assert_eq!(out, "Xbcd!");
        assert_eq!(applied, 2);
    }
}

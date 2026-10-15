//! Configurable VHDL and Verilog linter.
//!
//! [`lint`] runs every enabled rule over the files of an [`Analysis`]:
//! syntax errors, the semantic checks of `vfl-sema` (`sema/*`), style rules
//! that look at one file (`style/*`) and naming conventions (`naming/*`).
//! Verilog and SystemVerilog files get the rules of the [`verilog`]
//! module, which share the VHDL rules' ids where they mean the same and
//! add `verilog/*` ones.
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
mod verilog;

use std::path::PathBuf;
use std::sync::Arc;

use rayon::prelude::*;
use rustc_hash::{FxHashMap, FxHashSet};
use vfl_config::{Config, Level};
use vfl_sema::{Analysis, FileId};
use vfl_syntax::green_walk::ChildTokens;
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
    /// For a naming finding: a name for the declaration that follows the
    /// convention, which editors offer to rename it to everywhere.
    pub rename: Option<String>,
}

impl Finding {
    pub fn new(rule: &'static str, range: TextRange, message: impl Into<String>) -> Self {
        Finding {
            rule,
            range,
            message: message.into(),
            fix: None,
            rename: None,
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
    /// For a naming problem: a name that follows the convention, to rename
    /// the declaration to everywhere (not applied by `--fix`).
    pub rename: Option<String>,
}

/// What [`lint`] needs to know about one file.
pub struct FileSettings<'a> {
    pub file: FileId,
    pub syntax_errors: &'a [SyntaxError],
    pub config: &'a Config,
    /// For a Verilog file, its syntax tree. The Verilog rules read what
    /// the other Verilog files given declare (packages, modules), so a
    /// project's Verilog files are best linted together.
    pub verilog: Option<&'a vfl_verilog_syntax::Parse>,
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
    files: FxHashMap<PathBuf, CachedFile>,
    /// What each Verilog file declares for the others, by its tree.
    exports: FxHashMap<PathBuf, (GreenNode, Arc<verilog::Exports>)>,
}

struct CachedFile {
    green: GreenNode,
    /// A Verilog file's tree, and what its findings depend on in the
    /// other files.
    verilog: Option<(GreenNode, u64)>,
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
    let mut sema: FxHashMap<FileId, Vec<&vfl_sema::Diagnostic>> = FxHashMap::default();
    for d in analysis.diagnostics() {
        sema.entry(d.file).or_default().push(d);
    }
    // What the Verilog files declare, reused for unchanged trees. A VHDL
    // project skips all of it.
    let any_verilog = files.iter().any(|s| s.verilog.is_some());
    let exports: Vec<(PathBuf, Arc<verilog::Exports>)> = if any_verilog {
        files
            .par_iter()
            .filter_map(|settings| {
                let parse = settings.verilog?;
                let path = &analysis.files()[settings.file as usize].path;
                let exports = match cache.exports.get(path) {
                    Some((green, exports)) if std::ptr::eq(&**green, &**parse.green()) => {
                        exports.clone()
                    }
                    _ => Arc::new(verilog::Exports::new(&parse.syntax(), parse.includes())),
                };
                Some((path.clone(), exports))
            })
            .collect()
    } else {
        Vec::new()
    };
    if any_verilog || !cache.exports.is_empty() {
        cache.exports = files
            .iter()
            .filter_map(|s| s.verilog)
            .zip(&exports)
            .map(|(parse, (path, e))| (path.clone(), (parse.green().clone(), e.clone())))
            .collect();
    }
    let project = any_verilog.then(|| verilog::Project::new(analysis, exports.clone()));
    // What each Verilog file sees of the others, for reuse.
    let dependencies: FxHashMap<FileId, u64> = match &project {
        Some(project) => files
            .iter()
            .filter(|s| s.verilog.is_some())
            .zip(&exports)
            .map(|(s, (_, e))| (s.file, project.dependencies(e)))
            .collect(),
        None => FxHashMap::default(),
    };
    let verilog_key = |settings: &FileSettings| {
        settings
            .verilog
            .map(|parse| (parse.green().clone(), dependencies[&settings.file]))
    };
    // Which files can reuse their findings.
    let reuse: Vec<bool> = files
        .par_iter()
        .map(|settings| {
            let file = &analysis.files()[settings.file as usize];
            let file_sema = sema.get(&settings.file).map(Vec::as_slice).unwrap_or(&[]);
            let key = verilog_key(settings);
            cache.files.get(&file.path).is_some_and(|cached| {
                std::ptr::eq(&*cached.green, &*file.green)
                    && match (&cached.verilog, &key) {
                        (Some((g, d)), Some((green, deps))) => {
                            std::ptr::eq(&**g, &**green) && d == deps
                        }
                        (None, None) => true,
                        _ => false,
                    }
                    && cached.config == *settings.config
                    && cached.sema.len() == file_sema.len()
                    && cached.sema.iter().zip(file_sema).all(|(a, b)| a == *b)
                    && !analysis.file_reanalyzed(settings.file)
            })
        })
        .collect();
    // The declarations of the files to lint again.
    let relint: FxHashSet<FileId> = files
        .iter()
        .zip(&reuse)
        .filter(|(_, reuse)| !**reuse)
        .map(|(settings, _)| settings.file)
        .collect();
    let decls: FxHashMap<FileId, Vec<&vfl_sema::Decl>> = relint
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
                project.as_ref(),
            );
            let cached = CachedFile {
                green: file.green.clone(),
                verilog: verilog_key(settings),
                config: settings.config.clone(),
                sema: file_sema.iter().map(|d| (*d).clone()).collect(),
                diagnostics: diagnostics.clone(),
                errors: errors.clone(),
            };
            (diagnostics, errors, Some(cached))
        })
        .collect();
    let mut out = LintResult::default();
    let mut kept: FxHashMap<PathBuf, CachedFile> = FxHashMap::default();
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
    project: Option<&verilog::Project>,
) -> (Vec<Diagnostic>, Vec<String>) {
    let file = &analysis.files()[settings.file as usize];
    let lint_config = &settings.config.lint;
    let mut errors: Vec<String> = rules::unknown_rules(lint_config)
        .into_iter()
        .map(|id| format!("unknown lint rule `{id}`"))
        .collect();
    let mut levels: FxHashMap<&str, Level> = FxHashMap::default();
    let mut level = |id: &str| -> Level {
        *levels
            .entry(rules::find(id).map_or("naming", |r| r.id))
            .or_insert_with(|| rules::level(lint_config, id))
    };
    let enabled_cache: FxHashMap<&'static str, bool> = RULES
        .iter()
        .map(|r| (r.id, rules::level(lint_config, r.id) != Level::Off))
        .collect();
    let enabled = |id: &str| enabled_cache.get(id).copied().unwrap_or(false);

    let mut findings = Vec::new();
    for error in settings.syntax_errors {
        findings.push(Finding::new(
            "syntax/error",
            error.range,
            error.message.clone(),
        ));
    }
    // Computed on demand: it costs nothing where the rule is off.
    let branches = if enabled(vfl_sema::codes::MISSING_BRANCH_ASSIGNMENT) {
        let clocked = rules::clocked_branches(lint_config).unwrap_or_else(|error| {
            errors.push(error);
            false
        });
        analysis.missing_branch_assignments(settings.file, clocked)
    } else {
        vfl_sema::MissingBranches::default()
    };
    // Computed on demand too.
    let multiple = if enabled(vfl_sema::codes::MULTIPLE_DRIVERS) {
        Some(
            rules::resolved_drivers(lint_config).unwrap_or_else(|error| {
                errors.push(error);
                vfl_sema::ResolvedDrivers::default()
            }),
        )
    } else {
        None
    };
    let drivers = analysis.driver_checks(
        settings.file,
        vfl_sema::DriverChecks {
            multiple,
            loops: enabled(vfl_sema::codes::COMBINATIONAL_LOOP),
        },
    );
    // A latch that a missing branch assignment explains is reported as the
    // latter only.
    let explained = |d: &vfl_sema::Diagnostic| {
        d.code == vfl_sema::codes::INFERRED_LATCH && branches.latches.contains(&d.range)
    };
    for d in sema
        .iter()
        .copied()
        .filter(|d| !explained(d))
        .chain(&branches.diagnostics)
        .chain(&drivers)
    {
        let code = d.code;
        let mut finding = Finding::new(code, d.range, d.message.clone());
        finding.fix = d.fix.clone();
        findings.push(finding);
    }
    let max_line_length = rules::options(lint_config, "style/line-length")
        .and_then(|o| o.get("max"))
        .and_then(|v| v.as_integer())
        .map_or(120, |v| v.max(1) as usize);
    if let Err(error) = rules::undeclared(lint_config) {
        errors.push(error);
    }
    let (conventions, naming_errors) = naming::conventions(lint_config);
    errors.extend(naming_errors);
    let suppressions = match (settings.verilog, project) {
        (Some(parse), Some(project)) => {
            let root = parse.syntax();
            verilog::check(
                &verilog::Context {
                    text: &file.text,
                    root: &root,
                    path: &file.path,
                    includes: parse.includes(),
                    project,
                    max_line_length,
                    indent: settings.config.format.indent as usize,
                    conventions: &conventions,
                    parses: settings.syntax_errors.is_empty(),
                },
                enabled,
                &mut findings,
            );
            Suppressions::verilog(&root, &file.text, &file.line_index)
        }
        _ => {
            let root = file.syntax();
            style::check(
                &style::StyleContext {
                    text: &file.text,
                    root: &root,
                    file_stem: file
                        .path
                        .file_stem()
                        .map(|s| s.to_string_lossy().into_owned()),
                    max_line_length,
                    indent: settings.config.format.indent as usize,
                    standard: settings.config.standard(),
                },
                enabled,
                &mut findings,
            );
            for finding in &mut findings {
                if finding.rule == "style/positional-association" {
                    finding.fix = name_formals(analysis, settings.file, &root, finding.range);
                }
            }
            naming::check(decls, &file.text, &root, &conventions, &mut findings);
            Suppressions::new(&root, &file.text, &file.line_index)
        }
    };

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
            rename: finding.rename,
        });
    }
    (out, errors)
}

/// Names the formals of the positional associations of the generic or
/// port map at `range`, from the instantiated entity or component.
fn name_formals(
    analysis: &Analysis,
    file: FileId,
    root: &vfl_syntax::SyntaxNode,
    range: TextRange,
) -> Option<Fix> {
    use vfl_sema::{DeclKind, Target};
    use vfl_syntax::SyntaxKind;
    let map = root.covering_element(range).into_node()?;
    let statement = map.parent()?;
    let ports = map.kind() == SyntaxKind::PORT_MAP_ASPECT;
    let reference_at = |range: TextRange| {
        analysis.file_units(file).find_map(|unit| {
            let refs = analysis.unit_references(unit);
            let i = refs.partition_point(|r| r.range.start() < range.start());
            refs[i..].iter().find(|r| r.range == range)
        })
    };
    // The unit's name comes before the maps.
    let unit = statement
        .descendants_with_tokens()
        .filter_map(|e| e.into_token())
        .take_while(|t| t.text_range().end() <= map.text_range().start())
        .filter(|t| matches!(t.kind(), SyntaxKind::IDENT | SyntaxKind::EXT_IDENT))
        .filter_map(|t| reference_at(t.text_range()))
        .flat_map(|r| r.targets.iter())
        .find_map(|t| match t {
            Target::Decl(id) => analysis
                .decl(*id)
                .filter(|d| matches!(d.kind, DeclKind::Entity | DeclKind::Component)),
            _ => None,
        })?;
    let formals: Vec<&str> = unit
        .members
        .as_ref()?
        .iter()
        .filter_map(|m| analysis.decl(m))
        .filter(|d| match d.kind {
            DeclKind::Port { .. } => ports,
            DeclKind::Generic { .. }
            | DeclKind::GenericType
            | DeclKind::GenericSubprogram
            | DeclKind::GenericPackage => !ports,
            _ => false,
        })
        .map(|d| d.spelling.as_str())
        .collect();
    let elements: Vec<vfl_syntax::SyntaxNode> = map
        .descendants()
        .filter(|n| n.kind() == SyntaxKind::ASSOCIATION_ELEMENT)
        .filter(|n| n.parent().and_then(|p| p.parent()).as_ref() == Some(&map))
        .collect();
    let mut edits = Vec::new();
    for (i, element) in elements.iter().enumerate() {
        if element.has_child(|k| k == SyntaxKind::ARROW) {
            continue;
        }
        let formal = formals.get(i)?;
        let start = element.first_token()?;
        let start = if start.kind().is_trivia() {
            element
                .descendants_with_tokens()
                .filter_map(|e| e.into_token())
                .find(|t| !t.kind().is_trivia())?
        } else {
            start
        };
        edits.push(Edit {
            range: TextRange::empty(start.text_range().start()),
            text: format!("{formal} => "),
        });
    }
    (!edits.is_empty()).then(|| Fix {
        title: "Name the formals".into(),
        edits,
        safe: true,
    })
}

/// Analyzes a loaded project: every readable file with its inferred
/// libraries.
pub fn analyze(project: &vfl_project::Project) -> Analysis {
    Analysis::with_foreign(analysis_inputs(project), foreign_inputs(project))
}

/// Analyzes a new version of a project, reusing what `previous` found for
/// the units the changes cannot affect. `previous` is consumed so that
/// what the new analysis does not reuse is freed while it is built.
pub fn reanalyze(project: &vfl_project::Project, previous: Analysis) -> Analysis {
    previous.into_update_with_foreign(analysis_inputs(project), foreign_inputs(project))
}

/// The Verilog files of a project, whose modules VHDL instantiates.
fn foreign_inputs(project: &vfl_project::Project) -> Vec<vfl_sema::ForeignInput> {
    project
        .verilog
        .iter()
        .zip(&project.verilog_libraries)
        .map(|(file, libraries)| vfl_sema::ForeignInput {
            path: file.path.clone(),
            text: file.text.clone(),
            libraries: libraries.iter().map(|(l, _)| l.clone()).collect(),
            modules: file.foreign.clone(),
        })
        .collect()
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

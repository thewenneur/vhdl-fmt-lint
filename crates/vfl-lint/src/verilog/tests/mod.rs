//! Unit tests of the Verilog rules.
//!
//! Each test lints a few files with some rules on. A file marks the
//! findings it must have with a `// expect: rule-a, rule-b` comment on
//! the line reported; [`check`] fails on a finding not marked and on a mark
//! not found, so each test covers both what is reported and what is not.

mod fsm;
mod instances;
mod names;
mod procedures;
mod style;
mod width;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use vfl_config::{Config, LintConfig, VerilogStandard};
use vfl_project::{Encoding, VerilogFile};
use vfl_sema::{Analysis, ForeignInput};

use super::{Context, Exports, Project};
use crate::Finding;

/// A finding: file name, line, rule and message.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Found {
    pub file: String,
    pub line: usize,
    pub rule: String,
    pub message: String,
}

/// Lints `files` (name, text) together, with the rules `rules` on and the
/// naming conventions of `lint` (a `[lint]` table).
pub fn lint_with(files: &[(&str, &str)], rules: &[&str], lint: &str) -> Vec<(Found, Finding)> {
    let root = Path::new("/project");
    let config = Config::default();
    let parsed: Vec<VerilogFile> = files
        .iter()
        .map(|(name, text)| {
            VerilogFile::new(
                root.join(name),
                (*text).to_owned(),
                Encoding::Utf8,
                &config,
                root,
                VerilogStandard::Sv2023,
            )
        })
        .collect();
    let foreign = parsed
        .iter()
        .map(|f| ForeignInput {
            path: f.path.clone(),
            text: f.text.clone(),
            libraries: vec!["work".into()],
            modules: f.foreign.clone(),
        })
        .collect();
    let analysis = Analysis::with_foreign(Vec::new(), foreign);
    let exports: Vec<(PathBuf, Arc<Exports>)> = parsed
        .iter()
        .map(|f| {
            let exports = Exports::new(&f.parse.syntax(), f.parse.includes());
            (f.path.clone(), Arc::new(exports))
        })
        .collect();
    let project = Project::new(&analysis, exports);
    let lint: LintConfig = toml::from_str(lint).unwrap();
    let (conventions, errors) = crate::naming::conventions(&lint);
    assert!(errors.is_empty(), "{errors:?}");
    let mut out = Vec::new();
    for (file, (name, _)) in parsed.iter().zip(files) {
        let root = file.parse.syntax();
        let ctx = Context {
            text: &file.text,
            root: &root,
            path: &file.path,
            includes: file.parse.includes(),
            project: &project,
            max_line_length: 100,
            indent: 4,
            conventions: &conventions,
            parses: file.parse.errors().is_empty(),
        };
        let mut findings = Vec::new();
        super::check(&ctx, |rule| rules.contains(&rule), &mut findings);
        for finding in findings {
            let found = Found {
                file: (*name).to_owned(),
                line: ctx.line(finding.range.start()),
                rule: finding.rule.to_owned(),
                message: finding.message.clone(),
            };
            out.push((found, finding));
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// Lints one file, `test.sv`, with the rules `rules` on.
pub fn lint(text: &str, rules: &[&str]) -> Vec<(Found, Finding)> {
    lint_with(&[("test.sv", text)], rules, "")
}

/// The messages of the findings of `rules` in `text`, in order.
pub fn messages(text: &str, rules: &[&str]) -> Vec<String> {
    lint(text, rules)
        .into_iter()
        .map(|(f, _)| f.message)
        .collect()
}

/// The `// expect:` marks of the files.
fn expected(files: &[(&str, &str)], rules: &[&str]) -> BTreeSet<(String, usize, String)> {
    let mut out = BTreeSet::new();
    for (name, text) in files {
        for (i, line) in text.lines().enumerate() {
            if let Some((_, marks)) = line.split_once("// expect: ") {
                for rule in marks.split(',').map(str::trim) {
                    assert!(rules.contains(&rule), "`{rule}` is marked but not on");
                    out.insert(((*name).to_owned(), i + 1, rule.to_owned()));
                }
            }
        }
    }
    out
}

/// Checks that the files have exactly the findings they mark, with the
/// rules `rules` on.
#[track_caller]
pub fn check_files(files: &[(&str, &str)], rules: &[&str]) {
    let found = lint_with(files, rules, "");
    let expected = expected(files, rules);
    let key = |f: &Found| (f.file.clone(), f.line, f.rule.clone());
    let reported: BTreeSet<(String, usize, String)> = found.iter().map(|(f, _)| key(f)).collect();
    let missing: Vec<_> = expected.difference(&reported).collect();
    let unexpected: Vec<&Found> = found
        .iter()
        .map(|(f, _)| f)
        .filter(|f| !expected.contains(&key(f)))
        .collect();
    assert!(
        missing.is_empty() && unexpected.is_empty(),
        "missing: {missing:#?}\nunexpected: {unexpected:#?}"
    );
}

/// [`check_files`] on one file.
#[track_caller]
pub fn check(text: &str, rules: &[&str]) {
    check_files(&[("test.sv", text)], rules);
}

/// `text` with the fix of its only finding of `rule` applied.
#[track_caller]
pub fn fixed(text: &str, rule: &str) -> String {
    let found = lint(text, &[rule]);
    let [(_, finding)] = found.as_slice() else {
        panic!("expected one finding of {rule}: {found:#?}");
    };
    let fix = finding.fix.as_ref().expect("a fix");
    crate::apply_fixes(text, [fix]).0
}

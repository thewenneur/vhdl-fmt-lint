//! Printing diagnostics: for people, as JSON, as SARIF, as GitHub
//! Actions annotations, as JUnit XML and as GitLab Code Quality reports.

use std::fmt::Write;
use std::path::Path;

use serde_json::{Value, json};
use vfl_config::Level;
use vfl_syntax::{LineIndex, TextRange};

use crate::{Diagnostic, rules};

/// How to print diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// Messages with the offending source line.
    Human,
    /// One line per diagnostic: `path:line:col: level[rule] message`.
    Concise,
    Json,
    Sarif,
    /// `::error file=…` workflow commands.
    Github,
    /// A JUnit XML report: a test suite per file, a failed test case per
    /// diagnostic.
    Junit,
    /// A GitLab Code Quality report (JSON).
    Gitlab,
}

/// A file the diagnostics point into.
pub struct Source<'a> {
    /// The path to print, usually relative to the working directory.
    pub path: &'a Path,
    pub text: &'a str,
    pub line_index: &'a LineIndex,
}

struct Position {
    line: u32,
    col: u32,
    end_line: u32,
    end_col: u32,
}

fn position(source: &Source, range: TextRange) -> Position {
    let start = source.line_index.line_col(source.text, range.start());
    let end = source.line_index.line_col(source.text, range.end());
    Position {
        line: start.line + 1,
        col: start.col + 1,
        end_line: end.line + 1,
        end_col: end.col + 1,
    }
}

fn level_name(level: Level) -> &'static str {
    match level {
        Level::Off => "off",
        Level::Info => "info",
        Level::Warn => "warning",
        Level::Error => "error",
    }
}

fn path_text(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

/// Renders `diagnostics`; `source(i)` is the file of `diagnostics[i]`.
pub fn render<'a>(
    format: Format,
    diagnostics: &[Diagnostic],
    source: impl Fn(usize) -> Source<'a>,
    color: bool,
) -> String {
    match format {
        Format::Human => human(diagnostics, source, color),
        Format::Concise => concise(diagnostics, source),
        Format::Json => {
            serde_json::to_string_pretty(&json_value(diagnostics, source)).expect("JSON serializes")
                + "\n"
        }
        Format::Sarif => {
            serde_json::to_string_pretty(&sarif(diagnostics, source)).expect("JSON serializes")
                + "\n"
        }
        Format::Github => github(diagnostics, source),
        Format::Junit => junit(diagnostics, source),
        Format::Gitlab => {
            serde_json::to_string_pretty(&gitlab(diagnostics, source)).expect("JSON serializes")
                + "\n"
        }
    }
}

/// The text of the line `offset` is on, without its line ending.
fn line_at(text: &str, offset: usize) -> &str {
    let start = text[..offset].rfind('\n').map_or(0, |i| i + 1);
    let end = text[start..].find('\n').map_or(text.len(), |i| start + i);
    text[start..end].trim_end_matches('\r')
}

/// A stable identity of a diagnostic that survives lines being added or
/// removed above it: a hash of its file, its rule, its message with the
/// numbers left out, and the text of its line with the spacing left out.
/// `path` is `/`-separated.
pub fn fingerprint(path: &str, rule: &str, message: &str, line: &str) -> String {
    // FNV-1a: the same on every platform and in every version.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let mut feed = |bytes: &[u8]| {
        for &b in bytes.iter().chain(&[0xff]) {
            hash ^= u64::from(b);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
    };
    feed(path.as_bytes());
    feed(rule.as_bytes());
    let mut message = message.to_owned();
    message.retain(|c| !c.is_ascii_digit());
    feed(message.as_bytes());
    let line: Vec<&str> = line.split_whitespace().collect();
    feed(line.join(" ").as_bytes());
    format!("{hash:016x}")
}

/// The [`fingerprint`] of `d`, in `s`.
pub fn diagnostic_fingerprint(d: &Diagnostic, s: &Source) -> String {
    fingerprint(
        &path_text(s.path),
        d.rule,
        &d.message,
        line_at(s.text, usize::from(d.range.start())),
    )
}

fn xml_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            // Not allowed in XML 1.0.
            c if (c as u32) < 0x20 && !matches!(c, '\t' | '\n' | '\r') => out.push(' '),
            c => out.push(c),
        }
    }
    out
}

fn junit<'a>(diagnostics: &[Diagnostic], source: impl Fn(usize) -> Source<'a>) -> String {
    // Test suites by file, in the order of the diagnostics.
    let mut suites: Vec<(String, Vec<String>)> = Vec::new();
    for (i, d) in diagnostics.iter().enumerate() {
        let s = source(i);
        let p = position(&s, d.range);
        let path = xml_escape(&path_text(s.path));
        let rule = xml_escape(d.rule);
        let message = xml_escape(&d.message);
        let level = level_name(d.level);
        let (line, col) = (p.line, p.col);
        let case = format!(
            "    <testcase classname=\"{path}\" name=\"{path}:{line}:{col}: {rule}\">\n      \
             <failure type=\"{level}\" message=\"{message}\">\
             {path}:{line}:{col}: {level}[{rule}] {message}</failure>\n    </testcase>\n"
        );
        match suites.iter_mut().find(|(p, _)| *p == path) {
            Some((_, cases)) => cases.push(case),
            None => suites.push((path, vec![case])),
        }
    }
    let mut out = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    let _ = writeln!(
        out,
        "<testsuites name=\"vfl\" tests=\"{0}\" failures=\"{0}\" errors=\"0\">",
        diagnostics.len()
    );
    for (path, cases) in &suites {
        let _ = writeln!(
            out,
            "  <testsuite name=\"{path}\" tests=\"{0}\" failures=\"{0}\" errors=\"0\">",
            cases.len()
        );
        for case in cases {
            out.push_str(case);
        }
        out.push_str("  </testsuite>\n");
    }
    out.push_str("</testsuites>\n");
    out
}

fn gitlab<'a>(diagnostics: &[Diagnostic], source: impl Fn(usize) -> Source<'a>) -> Value {
    let mut seen: rustc_hash::FxHashSet<String> = Default::default();
    let items: Vec<Value> = diagnostics
        .iter()
        .enumerate()
        .map(|(i, d)| {
            let s = source(i);
            let p = position(&s, d.range);
            // GitLab keeps one finding per fingerprint: number the repeats.
            let base = diagnostic_fingerprint(d, &s);
            let mut fingerprint = base.clone();
            let mut n = 1;
            while !seen.insert(fingerprint.clone()) {
                n += 1;
                fingerprint = format!("{base}-{n}");
            }
            json!({
                "description": d.message,
                "check_name": d.rule,
                "fingerprint": fingerprint,
                "severity": match d.level {
                    Level::Error => "major",
                    Level::Warn => "minor",
                    _ => "info",
                },
                "location": {
                    "path": path_text(s.path),
                    "lines": { "begin": p.line, "end": p.end_line },
                },
            })
        })
        .collect();
    Value::Array(items)
}

fn paint(color: bool, code: &str, text: &str) -> String {
    if color {
        format!("\x1b[{code}m{text}\x1b[0m")
    } else {
        text.to_owned()
    }
}

fn level_color(level: Level) -> &'static str {
    match level {
        Level::Error => "1;31",
        Level::Warn => "1;33",
        _ => "1;36",
    }
}

fn concise<'a>(diagnostics: &[Diagnostic], source: impl Fn(usize) -> Source<'a>) -> String {
    let mut out = String::new();
    for (i, d) in diagnostics.iter().enumerate() {
        let s = source(i);
        let p = position(&s, d.range);
        let _ = writeln!(
            out,
            "{}:{}:{}: {}[{}] {}",
            path_text(s.path),
            p.line,
            p.col,
            level_name(d.level),
            d.rule,
            d.message
        );
    }
    out
}

fn human<'a>(
    diagnostics: &[Diagnostic],
    source: impl Fn(usize) -> Source<'a>,
    color: bool,
) -> String {
    let mut out = String::new();
    for (i, d) in diagnostics.iter().enumerate() {
        let s = source(i);
        let p = position(&s, d.range);
        let _ = writeln!(
            out,
            "{}[{}]: {}",
            paint(color, level_color(d.level), level_name(d.level)),
            d.rule,
            paint(color, "1", &d.message)
        );
        let number = p.line.to_string();
        let gutter = " ".repeat(number.len());
        let _ = writeln!(
            out,
            "{gutter}{} {}:{}:{}",
            paint(color, "34", "-->"),
            path_text(s.path),
            p.line,
            p.col
        );
        let line_start = s.text[..usize::from(d.range.start())]
            .rfind('\n')
            .map_or(0, |i| i + 1);
        let line_end = s.text[line_start..]
            .find('\n')
            .map_or(s.text.len(), |i| line_start + i);
        let line = s.text[line_start..line_end].trim_end_matches('\r');
        let bar = paint(color, "34", "|");
        let _ = writeln!(out, "{gutter} {bar}");
        let _ = writeln!(
            out,
            "{} {bar} {}",
            paint(color, "34", &number),
            line.replace('\t', " ")
        );
        let start_col = (p.col - 1) as usize;
        let width = if p.end_line == p.line {
            (p.end_col - p.col).max(1) as usize
        } else {
            line.chars().count().saturating_sub(start_col).max(1)
        };
        let _ = writeln!(
            out,
            "{gutter} {bar} {}{}",
            " ".repeat(start_col),
            paint(color, level_color(d.level), &"^".repeat(width))
        );
        if let Some(fix) = &d.fix {
            let note = if fix.safe {
                format!("fix: {}", fix.title)
            } else {
                format!("fix (unsafe): {}", fix.title)
            };
            let _ = writeln!(out, "{gutter} {} {}", paint(color, "34", "="), note);
        }
        out.push('\n');
    }
    out
}

fn json_value<'a>(diagnostics: &[Diagnostic], source: impl Fn(usize) -> Source<'a>) -> Value {
    let items: Vec<Value> = diagnostics
        .iter()
        .enumerate()
        .map(|(i, d)| {
            let s = source(i);
            let p = position(&s, d.range);
            let fix = d.fix.as_ref().map(|f| {
                json!({
                    "title": f.title,
                    "safe": f.safe,
                    "edits": f.edits.iter().map(|e| {
                        let ep = position(&s, e.range);
                        json!({
                            "start": { "line": ep.line, "column": ep.col, "offset": u32::from(e.range.start()) },
                            "end": { "line": ep.end_line, "column": ep.end_col, "offset": u32::from(e.range.end()) },
                            "text": e.text,
                        })
                    }).collect::<Vec<_>>(),
                })
            });
            json!({
                "file": path_text(s.path),
                "rule": d.rule,
                "level": level_name(d.level),
                "message": d.message,
                "start": { "line": p.line, "column": p.col, "offset": u32::from(d.range.start()) },
                "end": { "line": p.end_line, "column": p.end_col, "offset": u32::from(d.range.end()) },
                "fix": fix,
            })
        })
        .collect();
    Value::Array(items)
}

fn sarif<'a>(diagnostics: &[Diagnostic], source: impl Fn(usize) -> Source<'a>) -> Value {
    let mut rule_ids: Vec<&str> = diagnostics.iter().map(|d| d.rule).collect();
    rule_ids.sort_unstable();
    rule_ids.dedup();
    let rules_json: Vec<Value> = rule_ids
        .iter()
        .map(|id| {
            let summary = rules::find(id).map_or_else(
                || {
                    format!(
                        "Naming convention for {}.",
                        id.trim_start_matches("naming/")
                    )
                },
                |r| r.summary.to_owned(),
            );
            json!({ "id": id, "shortDescription": { "text": summary } })
        })
        .collect();
    let results: Vec<Value> = diagnostics
        .iter()
        .enumerate()
        .map(|(i, d)| {
            let s = source(i);
            let p = position(&s, d.range);
            json!({
                "ruleId": d.rule,
                "ruleIndex": rule_ids.iter().position(|r| *r == d.rule),
                "level": match d.level {
                    Level::Error => "error",
                    Level::Warn => "warning",
                    _ => "note",
                },
                "message": { "text": d.message },
                "locations": [{
                    "physicalLocation": {
                        "artifactLocation": { "uri": path_text(s.path) },
                        "region": {
                            "startLine": p.line,
                            "startColumn": p.col,
                            "endLine": p.end_line,
                            "endColumn": p.end_col,
                        }
                    }
                }],
            })
        })
        .collect();
    json!({
        "$schema": "https://json.schemastore.org/sarif-2.1.0.json",
        "version": "2.1.0",
        "runs": [{
            "tool": {
                "driver": {
                    "name": "vfl",
                    "version": env!("CARGO_PKG_VERSION"),
                    "informationUri": "https://github.com/thewenneur/vhdl-fmt-lint",
                    "rules": rules_json,
                }
            },
            "results": results,
        }]
    })
}

/// Escapes a workflow command property or message.
fn escape(text: &str, property: bool) -> String {
    let mut out = text
        .replace('%', "%25")
        .replace('\r', "%0D")
        .replace('\n', "%0A");
    if property {
        out = out.replace(':', "%3A").replace(',', "%2C");
    }
    out
}

fn github<'a>(diagnostics: &[Diagnostic], source: impl Fn(usize) -> Source<'a>) -> String {
    let mut out = String::new();
    for (i, d) in diagnostics.iter().enumerate() {
        let s = source(i);
        let p = position(&s, d.range);
        let command = match d.level {
            Level::Error => "error",
            Level::Warn => "warning",
            _ => "notice",
        };
        let _ = writeln!(
            out,
            "::{command} title={},file={},line={},col={},endLine={},endColumn={}::{}",
            escape(d.rule, true),
            escape(&path_text(s.path), true),
            p.line,
            p.col,
            p.end_line,
            p.end_col,
            escape(&d.message, false)
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use vfl_syntax::TextSize;

    fn diagnostic() -> Diagnostic {
        Diagnostic {
            rule: "sema/unused-signal",
            level: Level::Warn,
            file: 0,
            range: TextRange::new(TextSize::from(9), TextSize::from(10)),
            message: "signal `x` is never used".into(),
            fix: None,
            rename: None,
        }
    }

    fn render_text(format: Format) -> String {
        let text = "line one\n signal x\n";
        let index = LineIndex::new(text);
        render(
            format,
            &[diagnostic()],
            |_| Source {
                path: Path::new("dir/a.vhd"),
                text,
                line_index: &index,
            },
            false,
        )
    }

    #[test]
    fn human_shows_the_line() {
        assert_eq!(
            render_text(Format::Human),
            "warning[sema/unused-signal]: signal `x` is never used\n \
             --> dir/a.vhd:2:1\n  |\n2 |  signal x\n  | ^\n\n"
        );
    }

    #[test]
    fn machine_formats() {
        assert_eq!(
            render_text(Format::Concise),
            "dir/a.vhd:2:1: warning[sema/unused-signal] signal `x` is never used\n"
        );
        assert!(render_text(Format::Github).starts_with(
            "::warning title=sema/unused-signal,file=dir/a.vhd,line=2,col=1,endLine=2,endColumn=2::"
        ));
        let json: Value = serde_json::from_str(&render_text(Format::Json)).unwrap();
        assert_eq!(json[0]["start"]["line"], 2);
        let sarif: Value = serde_json::from_str(&render_text(Format::Sarif)).unwrap();
        assert_eq!(
            sarif["runs"][0]["results"][0]["ruleId"],
            "sema/unused-signal"
        );
        let gitlab: Value = serde_json::from_str(&render_text(Format::Gitlab)).unwrap();
        assert_eq!(gitlab[0]["check_name"], "sema/unused-signal");
        assert_eq!(gitlab[0]["severity"], "minor");
        assert_eq!(gitlab[0]["location"]["path"], "dir/a.vhd");
        assert_eq!(gitlab[0]["location"]["lines"]["begin"], 2);
        assert_eq!(gitlab[0]["fingerprint"].as_str().unwrap().len(), 16);
        assert_eq!(
            render_text(Format::Junit),
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
             <testsuites name=\"vfl\" tests=\"1\" failures=\"1\" errors=\"0\">\n  \
             <testsuite name=\"dir/a.vhd\" tests=\"1\" failures=\"1\" errors=\"0\">\n    \
             <testcase classname=\"dir/a.vhd\" name=\"dir/a.vhd:2:1: sema/unused-signal\">\n      \
             <failure type=\"warning\" message=\"signal `x` is never used\">\
             dir/a.vhd:2:1: warning[sema/unused-signal] signal `x` is never used</failure>\n    \
             </testcase>\n  </testsuite>\n</testsuites>\n"
        );
    }

    #[test]
    fn fingerprints_ignore_line_numbers_and_spacing() {
        let a = fingerprint("a.vhd", "r/x", "at line 12", "  signal  x : bit;");
        assert_eq!(
            a,
            fingerprint("a.vhd", "r/x", "at line 345", "signal x : bit;")
        );
        assert_ne!(
            a,
            fingerprint("b.vhd", "r/x", "at line 12", "signal x : bit;")
        );
        assert_ne!(
            a,
            fingerprint("a.vhd", "r/y", "at line 12", "signal x : bit;")
        );
        assert_ne!(
            a,
            fingerprint("a.vhd", "r/x", "at line 12", "signal y : bit;")
        );
        assert_eq!(xml_escape("a<b>&\"'\u{1}"), "a&lt;b&gt;&amp;&quot;&apos; ");
    }
}

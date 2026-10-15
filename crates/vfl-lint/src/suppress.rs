//! Suppression comments.
//!
//! - `-- vfl-ignore rule-a, rule-b` at the end of a line silences those
//!   rules on that line; on a line of its own, on the next line of code.
//! - `-- vfl-ignore-file rule-a` anywhere silences them in the whole file.
//!
//! Verilog files write them in `//` comments.
//!
//! Without rule ids every rule is silenced. A category (`naming`) silences
//! all its rules.

use vfl_syntax::green_walk::for_each_token;
use vfl_syntax::{LineIndex, SyntaxKind, SyntaxNode, TextRange, TextSize};

#[derive(Debug, Default)]
pub struct Suppressions {
    /// (zero-based line, rules; empty for all).
    lines: Vec<(u32, Vec<String>)>,
    file: Vec<Vec<String>>,
}

impl Suppressions {
    pub fn new(root: &SyntaxNode, text: &str, index: &LineIndex) -> Self {
        let mut comments = Vec::new();
        for_each_token(root, |kind, range, text| {
            if kind == SyntaxKind::LINE_COMMENT {
                comments.extend(suppression(range, text.trim_start_matches('-')));
            }
        });
        Self::from_comments(comments.into_iter(), text, index, "--")
    }

    /// The suppressions of a Verilog file: `// vfl-ignore rule`.
    pub fn verilog(root: &vfl_verilog_syntax::SyntaxNode, text: &str, index: &LineIndex) -> Self {
        let mut comments = Vec::new();
        for_each_token(root, |kind, range, text| {
            if kind == vfl_verilog_syntax::SyntaxKind::LINE_COMMENT {
                comments.extend(suppression(range, text.trim_start_matches('/')));
            }
        });
        Self::from_comments(comments.into_iter(), text, index, "//")
    }

    /// The comments, by range, that start with `vfl-ignore`.
    fn from_comments(
        comments: impl Iterator<Item = (TextRange, String)>,
        text: &str,
        index: &LineIndex,
        comment: &str,
    ) -> Self {
        let mut out = Suppressions::default();
        for (range, body) in comments {
            let body = body.trim();
            let (file_wide, rest) = if let Some(rest) = body.strip_prefix("vfl-ignore-file") {
                (true, rest)
            } else if let Some(rest) = body.strip_prefix("vfl-ignore") {
                (false, rest)
            } else {
                continue;
            };
            if !(rest.is_empty() || rest.starts_with([' ', '\t', ':'])) {
                continue;
            }
            let rules: Vec<String> = rest
                .trim_start_matches(':')
                .split([',', ' ', '\t'])
                .map(str::trim)
                .filter(|r| !r.is_empty())
                .take_while(|r| !r.starts_with(comment))
                .map(str::to_owned)
                .collect();
            if file_wide {
                out.file.push(rules);
                continue;
            }
            let start = range.start();
            let line = index.line(start);
            let line_start = line_start(text, start);
            let alone = text[line_start..usize::from(start)].trim().is_empty();
            let target = if alone {
                next_code_line(text, usize::from(range.end()), index, comment)
            } else {
                Some(line)
            };
            if let Some(target) = target {
                out.lines.push((target, rules));
            }
        }
        out
    }

    /// Whether `rule` is silenced on zero-based `line`.
    pub fn suppressed(&self, rule: &str, line: u32) -> bool {
        self.file.iter().any(|rules| matches(rules, rule))
            || self
                .lines
                .iter()
                .any(|(l, rules)| *l == line && matches(rules, rule))
    }
}

fn matches(rules: &[String], rule: &str) -> bool {
    rules.is_empty()
        || rules.iter().any(|r| {
            r == rule
                || rule
                    .strip_prefix(r.trim_end_matches("/*"))
                    .is_some_and(|rest| rest.starts_with('/'))
        })
}

fn line_start(text: &str, offset: TextSize) -> usize {
    text[..usize::from(offset)].rfind('\n').map_or(0, |i| i + 1)
}

/// A comment's body, when it is a suppression: only those are copied.
fn suppression(range: TextRange, body: &str) -> Option<(TextRange, String)> {
    let body = body.trim();
    body.starts_with("vfl-ignore")
        .then(|| (range, body.to_owned()))
}

/// The first line after the one holding `from` that is not blank or a comment.
fn next_code_line(text: &str, from: usize, index: &LineIndex, comment: &str) -> Option<u32> {
    let mut offset = text[from..].find('\n').map(|i| from + i + 1)?;
    while offset < text.len() {
        let end = text[offset..]
            .find('\n')
            .map_or(text.len(), |i| offset + i + 1);
        let trimmed = text[offset..end].trim();
        if !trimmed.is_empty() && !trimmed.starts_with(comment) {
            return Some(index.line(TextSize::from(offset as u32)));
        }
        offset = end;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use vfl_syntax::{Standard, parse};

    fn suppressions(text: &str) -> Suppressions {
        let parse = parse(text, Standard::Vhdl2008);
        Suppressions::new(&parse.syntax(), text, &LineIndex::new(text))
    }

    #[test]
    fn same_line_next_line_and_file() {
        let text = "entity e is end; -- vfl-ignore style/tabs\n\
                    -- vfl-ignore naming\n\
                    \n\
                    architecture a of e is begin end;\n\
                    -- vfl-ignore-file sema/unused-signal\n";
        let s = suppressions(text);
        assert!(s.suppressed("style/tabs", 0));
        assert!(!s.suppressed("style/line-length", 0));
        assert!(s.suppressed("naming/signal", 3));
        assert!(!s.suppressed("naming/signal", 2));
        assert!(s.suppressed("sema/unused-signal", 0));
    }

    #[test]
    fn bare_ignore_silences_everything() {
        let s = suppressions("signal x : bit; -- vfl-ignore\n");
        assert!(s.suppressed("anything/at-all", 0));
        let s = suppressions("-- vfl-ignored is not a directive\nx");
        assert!(!s.suppressed("anything/at-all", 1));
    }
}

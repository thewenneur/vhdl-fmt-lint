//! Suppression comments.
//!
//! - `-- vhone-ignore rule-a, rule-b` at the end of a line silences those
//!   rules on that line; on a line of its own, on the next line of code.
//! - `-- vhone-ignore-file rule-a` anywhere silences them in the whole file.
//!
//! Without rule ids every rule is silenced. A category (`naming`) silences
//! all its rules.

use vhone_syntax::{LineIndex, SyntaxKind, SyntaxNode, TextSize};

#[derive(Debug, Default)]
pub struct Suppressions {
    /// (zero-based line, rules; empty for all).
    lines: Vec<(u32, Vec<String>)>,
    file: Vec<Vec<String>>,
}

impl Suppressions {
    pub fn new(root: &SyntaxNode, text: &str, index: &LineIndex) -> Self {
        let mut out = Suppressions::default();
        for token in root
            .descendants_with_tokens()
            .filter_map(|e| e.into_token())
            .filter(|t| t.kind() == SyntaxKind::LINE_COMMENT)
        {
            let body = token.text().trim_start_matches('-').trim();
            // `vfl-` is the spelling from before the rename to vhone.
            let Some(body) = body
                .strip_prefix("vhone-")
                .or_else(|| body.strip_prefix("vfl-"))
            else {
                continue;
            };
            let (file_wide, rest) = if let Some(rest) = body.strip_prefix("ignore-file") {
                (true, rest)
            } else if let Some(rest) = body.strip_prefix("ignore") {
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
                .take_while(|r| !r.starts_with("--"))
                .map(str::to_owned)
                .collect();
            if file_wide {
                out.file.push(rules);
                continue;
            }
            let start = token.text_range().start();
            let line = index.line_col(text, start).line;
            let line_start = line_start(text, start);
            let alone = text[line_start..usize::from(start)].trim().is_empty();
            let target = if alone {
                next_code_line(text, usize::from(token.text_range().end()), index)
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

/// The first line after the one holding `from` that is not blank or a comment.
fn next_code_line(text: &str, from: usize, index: &LineIndex) -> Option<u32> {
    let mut offset = text[from..].find('\n').map(|i| from + i + 1)?;
    while offset < text.len() {
        let end = text[offset..]
            .find('\n')
            .map_or(text.len(), |i| offset + i + 1);
        let trimmed = text[offset..end].trim();
        if !trimmed.is_empty() && !trimmed.starts_with("--") {
            return Some(index.line_col(text, TextSize::from(offset as u32)).line);
        }
        offset = end;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use vhone_syntax::{Standard, parse};

    fn suppressions(text: &str) -> Suppressions {
        let parse = parse(text, Standard::Vhdl2008);
        Suppressions::new(&parse.syntax(), text, &LineIndex::new(text))
    }

    #[test]
    fn same_line_next_line_and_file() {
        let text = "entity e is end; -- vhone-ignore style/tabs\n\
                    -- vhone-ignore naming\n\
                    \n\
                    architecture a of e is begin end;\n\
                    -- vhone-ignore-file sema/unused-signal\n";
        let s = suppressions(text);
        assert!(s.suppressed("style/tabs", 0));
        assert!(!s.suppressed("style/line-length", 0));
        assert!(s.suppressed("naming/signal", 3));
        assert!(!s.suppressed("naming/signal", 2));
        assert!(s.suppressed("sema/unused-signal", 0));
    }

    #[test]
    fn bare_ignore_silences_everything() {
        let s = suppressions("signal x : bit; -- vhone-ignore\n");
        assert!(s.suppressed("anything/at-all", 0));
        let s = suppressions("-- vhone-ignored is not a directive\nx");
        assert!(!s.suppressed("anything/at-all", 1));
    }

    #[test]
    fn pre_rename_vfl_spelling_still_works() {
        let s =
            suppressions("signal x : bit; -- vfl-ignore style/tabs\n-- vfl-ignore-file naming\n");
        assert!(s.suppressed("style/tabs", 0));
        assert!(s.suppressed("naming/signal", 1));
    }
}

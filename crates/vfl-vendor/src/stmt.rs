//! A reader of the statement files of Lattice Diamond (`.lpf`) and Gowin
//! (`.cst`): words and `"strings"` up to a `;`, with `#` and `//`
//! comments. It keeps where everything is.

use vfl_syntax::{TextRange, TextSize};

/// A word of a statement: `LOCATE`, `"led[0]"` or `IO_TYPE=LVCMOS33`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Word {
    pub range: TextRange,
    /// The range without the quotes of a string.
    pub inner: TextRange,
}

impl Word {
    pub fn text<'a>(&self, source: &'a str) -> &'a str {
        &source[self.inner]
    }

    /// A `KEY=VALUE` word: the key and the value, without its quotes.
    pub fn pair<'a>(&self, source: &'a str) -> Option<(&'a str, &'a str)> {
        let (key, value) = source[self.range].split_once('=')?;
        Some((key.trim(), value.trim().trim_matches('"')))
    }
}

/// A statement, without its `;`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Statement {
    pub words: Vec<Word>,
    pub range: TextRange,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyntaxError {
    pub range: TextRange,
    pub message: String,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Statements {
    pub statements: Vec<Statement>,
    pub comments: Vec<TextRange>,
    pub errors: Vec<SyntaxError>,
}

fn range(start: usize, end: usize) -> TextRange {
    TextRange::new(TextSize::from(start as u32), TextSize::from(end as u32))
}

/// Reads `source`. `keywords` says which words start a statement, to find a
/// missing `;` when a statement runs into the next line's.
pub fn parse(source: &str, keywords: impl Fn(&str) -> bool) -> Statements {
    let text = source.as_bytes();
    let mut out = Statements::default();
    let mut words: Vec<Word> = Vec::new();
    // Whether the current word is the first of its line.
    let mut line_start = true;
    let mut pos = 0;
    let finish = |words: &mut Vec<Word>, out: &mut Statements| {
        if let (Some(first), Some(last)) = (words.first(), words.last()) {
            let range = first.range.cover(last.range);
            out.statements.push(Statement {
                words: std::mem::take(words),
                range,
            });
        }
    };
    while pos < text.len() {
        match text[pos] {
            b'\n' => {
                line_start = true;
                pos += 1;
            }
            b' ' | b'\t' | b'\r' => pos += 1,
            b';' => {
                finish(&mut words, &mut out);
                pos += 1;
            }
            b'#' => {
                let end = line_end(text, pos);
                out.comments.push(range(pos, end));
                pos = end;
            }
            b'/' if text.get(pos + 1) == Some(&b'/') => {
                let end = line_end(text, pos);
                out.comments.push(range(pos, end));
                pos = end;
            }
            _ => {
                let start = pos;
                let quoted = text[pos] == b'"';
                let mut unclosed = false;
                if quoted {
                    pos += 1;
                    while pos < text.len() && !matches!(text[pos], b'"' | b'\n') {
                        pos += 1;
                    }
                    if text.get(pos) == Some(&b'"') {
                        pos += 1;
                    } else {
                        unclosed = true;
                    }
                } else {
                    while pos < text.len() {
                        match text[pos] {
                            b' ' | b'\t' | b'\r' | b'\n' | b';' => break,
                            b'/' if text.get(pos + 1) == Some(&b'/') => break,
                            b'"' => {
                                // `IO_TYPE="LVCMOS33"`.
                                pos += 1;
                                while pos < text.len() && !matches!(text[pos], b'"' | b'\n') {
                                    pos += 1;
                                }
                                if text.get(pos) == Some(&b'"') {
                                    pos += 1;
                                } else {
                                    unclosed = true;
                                    break;
                                }
                            }
                            _ => pos += 1,
                        }
                    }
                }
                if unclosed {
                    out.errors.push(SyntaxError {
                        range: range(start, pos),
                        message: "missing close-quote".to_owned(),
                    });
                }
                let inner = if quoted && !unclosed {
                    range(start + 1, pos - 1)
                } else if quoted {
                    range(start + 1, pos)
                } else {
                    range(start, pos)
                };
                let word = Word {
                    range: range(start, pos),
                    inner,
                };
                if line_start && !quoted && !words.is_empty() && keywords(&source[inner]) {
                    let last = words.last().unwrap().range.end();
                    out.errors.push(SyntaxError {
                        range: TextRange::empty(last),
                        message: "missing `;` at the end of the statement".to_owned(),
                    });
                    finish(&mut words, &mut out);
                }
                words.push(word);
                line_start = false;
            }
        }
    }
    if let Some(last) = words.last() {
        out.errors.push(SyntaxError {
            range: TextRange::empty(last.range.end()),
            message: "missing `;` at the end of the statement".to_owned(),
        });
        finish(&mut words, &mut out);
    }
    out
}

fn line_end(text: &[u8], from: usize) -> usize {
    text[from..]
        .iter()
        .position(|&b| b == b'\n')
        .map_or(text.len(), |n| from + n)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(source: &str) -> Vec<Vec<&str>> {
        parse(source, |w| w == "LOCATE")
            .statements
            .iter()
            .map(|s| s.words.iter().map(|w| w.text(source)).collect())
            .collect()
    }

    #[test]
    fn statements_strings_and_comments() {
        let source = "# pins\nLOCATE COMP \"led[0]\" SITE \"A1\" ; // first\n\
                      IOBUF PORT \"led[0]\" IO_TYPE=\"LVCMOS33\"\n  PULLMODE=UP;\n";
        assert_eq!(
            words(source),
            [
                vec!["LOCATE", "COMP", "led[0]", "SITE", "A1"],
                vec![
                    "IOBUF",
                    "PORT",
                    "led[0]",
                    "IO_TYPE=\"LVCMOS33\"",
                    "PULLMODE=UP"
                ],
            ]
        );
        let parsed = parse(source, |_| false);
        assert_eq!(parsed.comments.len(), 2);
        assert!(parsed.errors.is_empty());
    }

    #[test]
    fn errors() {
        let source =
            "LOCATE COMP \"a\" SITE \"A1\"\nLOCATE COMP \"b\nLOCATE COMP \"c\" SITE \"C1\";\nX";
        let parsed = parse(source, |w| w == "LOCATE");
        let messages: Vec<_> = parsed.errors.iter().map(|e| e.message.as_str()).collect();
        assert_eq!(
            messages,
            [
                "missing `;` at the end of the statement",
                "missing close-quote",
                "missing `;` at the end of the statement",
                "missing `;` at the end of the statement",
            ]
        );
        assert_eq!(parsed.statements.len(), 4);
    }
}

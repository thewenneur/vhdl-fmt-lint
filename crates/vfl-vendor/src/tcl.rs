//! A Tcl reader that keeps where everything is: a script is a list of
//! commands, each a list of words with their ranges. Nothing is evaluated;
//! braced and quoted words keep their text, and `[...]` substitutions are
//! words of their own that can be read again as a script.
//!
//! Tcl's rules: commands end at a newline or `;` outside braces, quotes
//! and brackets; `#` starts a comment where a command could start; a
//! backslash before a newline joins the lines.

use vfl_syntax::{TextRange, TextSize};

/// How a word is written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WordKind {
    /// `text`, which may hold `$var` and `[cmd]` substitutions.
    Bare,
    /// `{text}`: literal.
    Braced,
    /// `"text"`, which may hold substitutions.
    Quoted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Word {
    pub kind: WordKind,
    /// The whole word, braces or quotes included.
    pub range: TextRange,
    /// The text inside the braces or quotes.
    pub inner: TextRange,
}

impl Word {
    pub fn text<'t>(&self, source: &'t str) -> &'t str {
        &source[self.inner]
    }

    /// Whether the word is the same whatever the variables and commands:
    /// braced, or without `$` and `[`.
    pub fn is_literal(&self, source: &str) -> bool {
        self.kind == WordKind::Braced || !self.text(source).contains(['$', '[', '\\'])
    }

    /// Whether the word is an option, such as `-name`: a literal `-`
    /// followed by a letter.
    pub fn is_option(&self, source: &str) -> bool {
        self.kind == WordKind::Bare && {
            let text = self.text(source);
            text.len() > 1
                && text.starts_with('-')
                && text[1..].starts_with(|c: char| c.is_ascii_alphabetic())
                && !text.contains(['$', '['])
        }
    }

    /// The word as one `[command ...]`, if it is: the range of what is
    /// between the brackets.
    pub fn command(&self, source: &str) -> Option<TextRange> {
        let text = self.text(source);
        if self.kind != WordKind::Bare || !text.starts_with('[') || !text.ends_with(']') {
            return None;
        }
        // The first bracket must close at the end.
        let mut depth = 0usize;
        for (i, c) in text.char_indices() {
            match c {
                '[' => depth += 1,
                ']' => {
                    depth -= 1;
                    if depth == 0 && i + 1 != text.len() {
                        return None;
                    }
                }
                _ => {}
            }
        }
        let start = self.inner.start() + TextSize::from(1);
        let end = self.inner.end() - TextSize::from(1);
        Some(TextRange::new(start, end))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Command {
    pub words: Vec<Word>,
    /// From the first word to the end of the last.
    pub range: TextRange,
}

impl Command {
    pub fn name<'t>(&self, source: &'t str) -> &'t str {
        self.words.first().map_or("", |w| w.text(source))
    }
}

/// A problem with how the script is written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyntaxError {
    pub range: TextRange,
    pub message: String,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Script {
    pub commands: Vec<Command>,
    pub comments: Vec<TextRange>,
    pub errors: Vec<SyntaxError>,
}

/// Reads `source[range]` as a script; ranges are in `source`.
pub fn parse_range(source: &str, range: TextRange) -> Script {
    let mut reader = Reader {
        text: source.as_bytes(),
        pos: usize::from(range.start()),
        end: usize::from(range.end()),
        script: Script::default(),
    };
    reader.script();
    reader.script
}

/// Reads a whole script.
pub fn parse(source: &str) -> Script {
    parse_range(source, TextRange::up_to(TextSize::of(source)))
}

struct Reader<'t> {
    text: &'t [u8],
    pos: usize,
    end: usize,
    script: Script,
}

fn range(start: usize, end: usize) -> TextRange {
    TextRange::new(TextSize::from(start as u32), TextSize::from(end as u32))
}

impl Reader<'_> {
    fn peek(&self) -> Option<u8> {
        (self.pos < self.end).then(|| self.text[self.pos])
    }

    fn error(&mut self, start: usize, end: usize, message: impl Into<String>) {
        self.script.errors.push(SyntaxError {
            range: range(start, end),
            message: message.into(),
        });
    }

    /// Skips spaces, tabs and backslash-newlines; `newlines` skips line
    /// ends and `;` too.
    fn skip_space(&mut self, newlines: bool) {
        while let Some(c) = self.peek() {
            match c {
                b' ' | b'\t' | b'\x0c' | b'\x0b' => self.pos += 1,
                b'\r' if self.text.get(self.pos + 1) != Some(&b'\n') || newlines => self.pos += 1,
                b'\\'
                    if matches!(self.text.get(self.pos + 1), Some(b'\n'))
                        && self.pos + 1 < self.end =>
                {
                    self.pos += 2
                }
                b'\\'
                    if self.text.get(self.pos + 1) == Some(&b'\r')
                        && self.text.get(self.pos + 2) == Some(&b'\n')
                        && self.pos + 2 < self.end =>
                {
                    self.pos += 3
                }
                b'\n' | b';' if newlines => self.pos += 1,
                _ => break,
            }
        }
    }

    fn script(&mut self) {
        loop {
            self.skip_space(true);
            match self.peek() {
                None => return,
                Some(b'#') => self.comment(),
                Some(_) => self.command(),
            }
        }
    }

    fn comment(&mut self) {
        let start = self.pos;
        while let Some(c) = self.peek() {
            match c {
                b'\\' => self.pos += 2,
                b'\n' => break,
                _ => self.pos += 1,
            }
        }
        self.pos = self.pos.min(self.end);
        let mut end = self.pos;
        if end > start && self.text[end - 1] == b'\r' {
            end -= 1;
        }
        self.script.comments.push(range(start, end));
    }

    fn command(&mut self) {
        let mut words = Vec::new();
        loop {
            self.skip_space(false);
            match self.peek() {
                None | Some(b'\n' | b';') => break,
                Some(b'\r') => {
                    self.pos += 1;
                    continue;
                }
                Some(_) => {}
            }
            words.push(self.word());
        }
        if let (Some(first), Some(last)) = (words.first(), words.last()) {
            let range = first.range.cover(last.range);
            self.script.commands.push(Command { words, range });
        }
    }

    fn word(&mut self) -> Word {
        let start = self.pos;
        match self.peek() {
            Some(b'{') => {
                self.pos += 1;
                let closed = self.braces(start);
                // `{*}word`: the elements of `word`, as arguments.
                if closed
                    && &self.text[start..self.pos] == b"{*}"
                    && !matches!(
                        self.peek(),
                        None | Some(b' ' | b'\t' | b'\n' | b'\r' | b';')
                    )
                {
                    self.bare();
                    return Word {
                        kind: WordKind::Bare,
                        range: range(start, self.pos),
                        inner: range(start, self.pos),
                    };
                }
                let inner_end = if closed { self.pos - 1 } else { self.pos };
                self.after_group(start, "close-brace");
                Word {
                    kind: WordKind::Braced,
                    range: range(start, self.pos),
                    inner: range(start + 1, inner_end.min(self.pos)),
                }
            }
            Some(b'"') => {
                self.pos += 1;
                let closed = self.quotes(start);
                let inner_end = if closed { self.pos - 1 } else { self.pos };
                self.after_group(start, "close-quote");
                Word {
                    kind: WordKind::Quoted,
                    range: range(start, self.pos),
                    inner: range(start + 1, inner_end.min(self.pos)),
                }
            }
            _ => {
                self.bare();
                Word {
                    kind: WordKind::Bare,
                    range: range(start, self.pos),
                    inner: range(start, self.pos),
                }
            }
        }
    }

    /// Characters right after a closing brace or quote: an error in Tcl.
    /// They are taken into the word.
    fn after_group(&mut self, start: usize, what: &str) {
        let at = self.pos;
        if matches!(
            self.peek(),
            None | Some(b' ' | b'\t' | b'\n' | b'\r' | b';')
        ) || (self.peek() == Some(b'\\')
            && matches!(self.text.get(self.pos + 1), Some(b'\n' | b'\r')))
        {
            return;
        }
        // Inside brackets, `]` ends the enclosing command.
        if self.peek() == Some(b']') {
            return;
        }
        self.bare();
        let _ = start;
        self.error(at, self.pos, format!("extra characters after {what}"));
    }

    /// From after `{` to after its `}`; whether it closed.
    fn braces(&mut self, open: usize) -> bool {
        let mut depth = 1usize;
        while let Some(c) = self.peek() {
            self.pos += 1;
            match c {
                b'\\' => self.pos = (self.pos + 1).min(self.end),
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        return true;
                    }
                }
                _ => {}
            }
        }
        self.error(open, open + 1, "missing close-brace");
        false
    }

    /// From after `"` to after the closing `"`; whether it closed.
    fn quotes(&mut self, open: usize) -> bool {
        while let Some(c) = self.peek() {
            match c {
                b'\\' => self.pos = (self.pos + 2).min(self.end),
                b'"' => {
                    self.pos += 1;
                    return true;
                }
                b'[' => self.brackets(),
                _ => self.pos += 1,
            }
        }
        self.error(open, open + 1, "missing close-quote");
        false
    }

    /// From `[` to after its `]`.
    fn brackets(&mut self) {
        let open = self.pos;
        self.pos += 1;
        loop {
            match self.peek() {
                None => {
                    self.error(open, open + 1, "missing close-bracket");
                    return;
                }
                Some(b']') => {
                    self.pos += 1;
                    return;
                }
                Some(b'\\') => self.pos = (self.pos + 2).min(self.end),
                Some(b'{') => {
                    let start = self.pos;
                    self.pos += 1;
                    self.braces(start);
                }
                Some(b'"') => {
                    let start = self.pos;
                    self.pos += 1;
                    self.quotes(start);
                }
                Some(b'[') => self.brackets(),
                Some(_) => self.pos += 1,
            }
        }
    }

    /// A bare word: up to a space, a line end or `;`, with `[...]` and
    /// `${...}` kept whole.
    fn bare(&mut self) {
        while let Some(c) = self.peek() {
            match c {
                b' ' | b'\t' | b'\n' | b';' => return,
                b'\r' if self.text.get(self.pos + 1) == Some(&b'\n') => return,
                b'\\' => {
                    if matches!(self.text.get(self.pos + 1), Some(b'\n' | b'\r')) {
                        return;
                    }
                    self.pos = (self.pos + 2).min(self.end);
                }
                b'[' => self.brackets(),
                b'$' if self.text.get(self.pos + 1) == Some(&b'{') => {
                    let start = self.pos + 1;
                    self.pos += 2;
                    self.braces(start);
                }
                _ => self.pos += 1,
            }
        }
    }
}

/// The elements of a Tcl list such as `{a b {c d}}`'s inner text, with
/// their ranges in `source` (braces dropped).
pub fn list_items(source: &str, inner: TextRange) -> Vec<TextRange> {
    let script = parse_range(source, inner);
    script
        .commands
        .iter()
        .flat_map(|c| c.words.iter().map(|w| w.inner))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(source: &str) -> Vec<Vec<String>> {
        parse(source)
            .commands
            .iter()
            .map(|c| c.words.iter().map(|w| w.text(source).to_owned()).collect())
            .collect()
    }

    #[test]
    fn commands_and_words() {
        let source = "set_global_assignment -name VHDL_FILE \"a b.vhd\"\n\
                      # a comment\n\
                      set x {a {b} c}; puts $x\n\
                      create_clock -period 10 [get_ports {clk}]\n";
        assert_eq!(
            words(source),
            vec![
                vec!["set_global_assignment", "-name", "VHDL_FILE", "a b.vhd"],
                vec!["set", "x", "a {b} c"],
                vec!["puts", "$x"],
                vec!["create_clock", "-period", "10", "[get_ports {clk}]"],
            ]
        );
        assert_eq!(parse(source).comments.len(), 1);
        assert!(parse(source).errors.is_empty());
    }

    #[test]
    fn continuations_join_lines() {
        let source = "set_location_assignment \\\n  PIN_A1 -to clk\r\nset a 1";
        assert_eq!(
            words(source),
            vec![
                vec!["set_location_assignment", "PIN_A1", "-to", "clk"],
                vec!["set", "a", "1"],
            ]
        );
    }

    #[test]
    fn unbalanced_groups_are_errors() {
        let errors =
            |s: &str| -> Vec<String> { parse(s).errors.into_iter().map(|e| e.message).collect() };
        assert_eq!(errors("set a {b\nc"), ["missing close-brace"]);
        assert_eq!(errors("set a \"b"), ["missing close-quote"]);
        assert_eq!(errors("puts [get_ports a"), ["missing close-bracket"]);
        assert_eq!(errors("set a {b}c"), ["extra characters after close-brace"]);
        assert!(errors("puts [list {a}]").is_empty());
        assert!(errors("file delete {*}[glob *]; f {*}$args").is_empty());
    }

    #[test]
    fn options_and_commands() {
        let source = "x -name -1 [get_ports a] [a][b]";
        let script = parse(source);
        let w = &script.commands[0].words;
        assert!(w[1].is_option(source));
        assert!(!w[2].is_option(source));
        let inner = w[3].command(source).unwrap();
        assert_eq!(&source[inner], "get_ports a");
        assert!(w[4].command(source).is_none());
    }
}

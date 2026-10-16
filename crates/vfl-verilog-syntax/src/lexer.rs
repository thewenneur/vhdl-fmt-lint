//! The lexer: splits source text into tokens, trivia included, and runs the
//! parts of the preprocessor that decide which tokens the parser sees.
//!
//! Lexing never fails. Malformed input produces tokens that still cover every
//! byte, plus an error for each problem found.
//!
//! Directives become `DIRECTIVE` trivia. The lexer keeps the set of defined
//! macros (from `` `define ``, `` `undef ``, the options and included files)
//! to choose the compiled branch of `` `ifdef `` / `` `ifndef `` /
//! `` `elsif `` / `` `else ``; the other branches become `INACTIVE` trivia.
//! Macro uses are not expanded.

use rowan::{TextRange, TextSize};
use rustc_hash::FxHashMap;

use crate::{IncludeReader, Options, Standard, SyntaxError, SyntaxKind};
use SyntaxKind::*;

/// One token: its kind and its length in bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Token {
    pub kind: SyntaxKind,
    pub len: TextSize,
}

/// What [`lex`] returns.
pub struct Lexed {
    /// Every token of the text, trivia included.
    pub tokens: Vec<Token>,
    pub errors: Vec<SyntaxError>,
    /// The names after `` `include ``, in order.
    pub includes: Vec<String>,
}

/// How deep included files are read, to stop on include cycles.
const MAX_INCLUDE_DEPTH: u32 = 16;

/// How many included files are read in all, so that a file included
/// several times on each level of a cycle does not take exponential time.
const MAX_INCLUDE_READS: u32 = 1024;

/// Splits `text` into tokens. Concatenating the tokens gives back `text`.
pub fn lex(text: &str, options: &Options) -> Lexed {
    let mut macros = Macros::default();
    for define in options.defines {
        let name = define.split('=').next().unwrap_or_default();
        macros.insert(name.to_owned(), false);
    }
    let mut lexer = Lexer::new(text, options, macros, 0);
    lexer.run();
    Lexed {
        tokens: lexer.tokens,
        errors: lexer.errors,
        includes: lexer.includes,
    }
}

/// Defined macros, and whether each takes arguments.
type Macros = FxHashMap<String, bool>;

/// One open `` `ifdef ``.
struct Conditional {
    /// Whether a branch was compiled already.
    taken: bool,
    /// Where the `` `ifdef `` is, for the "missing `` `endif ``" error.
    start: usize,
}

struct Lexer<'a> {
    text: &'a str,
    pos: usize,
    standards: Vec<Standard>,
    tokens: Vec<Token>,
    errors: Vec<SyntaxError>,
    macros: Macros,
    conditionals: Vec<Conditional>,
    includes: Vec<String>,
    include: Option<IncludeReader<'a>>,
    depth: u32,
    /// How many more included files may be read.
    reads_left: u32,
    /// Where the token being lexed starts.
    token_start: usize,
}

fn is_ident_start(c: u8) -> bool {
    c.is_ascii_alphabetic() || c == b'_'
}

fn is_ident_continue(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_' || c == b'$'
}

fn is_space(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\n' | b'\r' | b'\x0b' | b'\x0c')
}

fn is_based_digit(c: u8) -> bool {
    c.is_ascii_hexdigit() || matches!(c, b'_' | b'x' | b'X' | b'z' | b'Z' | b'?')
}

const TIME_UNITS: [&str; 6] = ["s", "ms", "us", "ns", "ps", "fs"];

/// Operators and delimiters, longest first within each first character.
const PUNCTUATION: &[(&str, SyntaxKind)] = &[
    ("<<<=", ASHL_EQ),
    (">>>=", ASHR_EQ),
    ("===", EQ3),
    ("!==", NEQ2),
    ("==?", EQ2_Q),
    ("!=?", NEQ_Q),
    ("<<<", ASHL),
    (">>>", ASHR),
    ("<<=", SHL_EQ),
    (">>=", SHR_EQ),
    ("<->", EQUIV),
    ("->>", DARROW),
    ("|->", PIPE_ARROW),
    ("|=>", PIPE_DARROW),
    ("&&&", AMP3),
    ("#-#", HASH_MINUS_HASH),
    ("#=#", HASH_EQ_HASH),
    ("##", HASH2),
    ("@@", AT2),
    ("::", COLON2),
    (":=", COLON_EQ),
    (":/", COLON_SLASH),
    ("+:", PLUS_COLON),
    ("-:", MINUS_COLON),
    ("++", PLUS2),
    ("--", MINUS2),
    ("**", STAR2),
    ("+=", PLUS_EQ),
    ("-=", MINUS_EQ),
    ("*=", STAR_EQ),
    ("/=", SLASH_EQ),
    ("%=", PERCENT_EQ),
    ("&=", AMP_EQ),
    ("|=", PIPE_EQ),
    ("^=", CARET_EQ),
    ("<=", LTE),
    (">=", GTE),
    ("==", EQ2),
    ("!=", NEQ),
    ("&&", AMP2),
    ("||", PIPE2),
    ("~&", TILDE_AMP),
    ("~|", TILDE_PIPE),
    ("~^", TILDE_CARET),
    ("^~", TILDE_CARET),
    ("->", ARROW),
    ("<<", SHL),
    (">>", SHR),
    (".*", DOT_STAR),
    ("(", LPAREN),
    (")", RPAREN),
    ("[", LBRACKET),
    ("]", RBRACKET),
    ("{", LBRACE),
    ("}", RBRACE),
    (";", SEMI),
    (",", COMMA),
    (".", DOT),
    (":", COLON),
    ("?", QUESTION),
    ("#", HASH),
    ("@", AT),
    ("=", EQ),
    ("+", PLUS),
    ("-", MINUS),
    ("*", STAR),
    ("/", SLASH),
    ("%", PERCENT),
    ("&", AMP),
    ("|", PIPE),
    ("^", CARET),
    ("~", TILDE),
    ("!", BANG),
    ("<", LT),
    (">", GT),
    ("'", TICK),
    ("$", DOLLAR),
];

impl<'a> Lexer<'a> {
    fn new(text: &'a str, options: &Options<'a>, macros: Macros, depth: u32) -> Self {
        Lexer {
            text,
            pos: 0,
            standards: vec![options.standard],
            tokens: Vec::with_capacity(text.len() / 4),
            errors: Vec::new(),
            macros,
            conditionals: Vec::new(),
            includes: Vec::new(),
            include: options.include,
            depth,
            reads_left: MAX_INCLUDE_READS,
            token_start: 0,
        }
    }

    fn run(&mut self) {
        while self.pos < self.text.len() {
            let start = self.pos;
            self.token_start = start;
            let kind = self.next_token();
            debug_assert!(self.pos > start, "lexer must make progress");
            // Skipping a branch pushes its tokens itself and moves `token_start`.
            self.push(kind, self.token_start);
        }
        for cond in std::mem::take(&mut self.conditionals) {
            self.error_at("`ifdef without `endif", cond.start, cond.start + 1);
        }
    }

    fn push(&mut self, kind: SyntaxKind, start: usize) {
        if self.pos > start {
            self.tokens.push(Token {
                kind,
                len: TextSize::from((self.pos - start) as u32),
            });
        }
    }

    fn standard(&self) -> Standard {
        *self.standards.last().expect("never empty")
    }

    fn bytes(&self) -> &'a [u8] {
        self.text.as_bytes()
    }

    fn peek(&self) -> u8 {
        self.peek_at(0)
    }

    fn peek_at(&self, n: usize) -> u8 {
        self.bytes().get(self.pos + n).copied().unwrap_or(0)
    }

    fn rest(&self) -> &'a str {
        &self.text[self.pos..]
    }

    fn error_at(&mut self, message: &str, start: usize, end: usize) {
        self.errors.push(SyntaxError::new(
            message,
            TextRange::new((start as u32).into(), (end as u32).into()),
        ));
    }

    /// Advances to the next char boundary at or after `pos`.
    fn advance_to(&mut self, pos: usize) {
        let mut pos = pos.min(self.text.len());
        while !self.text.is_char_boundary(pos) {
            pos += 1;
        }
        self.pos = pos;
    }

    fn eat_while(&mut self, f: impl Fn(u8) -> bool) {
        let mut pos = self.pos;
        while pos < self.text.len() && f(self.bytes()[pos]) {
            pos += 1;
        }
        self.advance_to(pos);
    }

    /// Skips spaces and tabs (not newlines).
    fn eat_blanks(&mut self) {
        self.eat_while(|c| c == b' ' || c == b'\t');
    }

    fn next_token(&mut self) -> SyntaxKind {
        let c = self.peek();
        if is_space(c) {
            self.eat_while(is_space);
            return WHITESPACE;
        }
        match c {
            b'/' if self.peek_at(1) == b'/' => {
                self.line_comment();
                LINE_COMMENT
            }
            b'/' if self.peek_at(1) == b'*' => self.block_comment(),
            b'(' if self.peek_at(1) == b'*' && self.at_attribute() => ATTRIBUTE,
            b'`' => self.backtick(),
            b'"' => self.string(),
            b'\\' => {
                self.pos += 1;
                self.eat_while(|c| !is_space(c));
                self.advance_to(self.pos);
                ESC_IDENT
            }
            b'$' if is_ident_continue(self.peek_at(1)) && self.peek_at(1) != b'$' => {
                self.pos += 1;
                self.eat_while(is_ident_continue);
                SYSTEM_IDENT
            }
            c if is_ident_start(c) => {
                let start = self.pos;
                self.eat_while(is_ident_continue);
                let word = &self.text[start..self.pos];
                SyntaxKind::keyword(word, self.standard()).unwrap_or(IDENT)
            }
            c if c.is_ascii_digit() => self.number(),
            b'\'' => self.tick(),
            _ => self.punctuation(),
        }
    }

    fn line_comment(&mut self) {
        self.eat_while(|c| c != b'\n' && c != b'\r');
    }

    fn block_comment(&mut self) -> SyntaxKind {
        let start = self.pos;
        match self.text[self.pos + 2..].find("*/") {
            Some(end) => self.pos += 2 + end + 2,
            None => {
                self.pos = self.text.len();
                self.error_at("unterminated block comment", start, start + 2);
            }
        }
        BLOCK_COMMENT
    }

    /// `(* ... *)`, unless it is `(*)` as in `@(*)`. Consumes the attribute.
    fn at_attribute(&mut self) -> bool {
        let rest = &self.rest()[2..];
        if rest.trim_start_matches([' ', '\t']).starts_with(')') {
            return false;
        }
        match rest.find("*)") {
            // An attribute holds no `;`; this keeps a stray `(*` from
            // swallowing code.
            Some(end) if !rest[..end].contains(';') => {
                self.pos += 2 + end + 2;
                true
            }
            _ => false,
        }
    }

    fn string(&mut self) -> SyntaxKind {
        let start = self.pos;
        if self.rest().starts_with("\"\"\"") {
            match self.text[self.pos + 3..].find("\"\"\"") {
                Some(end) => self.pos += 3 + end + 3,
                None => {
                    self.pos = self.text.len();
                    self.error_at("unterminated string", start, start + 3);
                }
            }
            return STRING_LITERAL;
        }
        self.pos += 1;
        loop {
            match self.peek() {
                b'"' => {
                    self.pos += 1;
                    break;
                }
                b'\\' => {
                    // An escaped character, or a line continuation.
                    self.pos += 1;
                    if self.peek() == b'\r' && self.peek_at(1) == b'\n' {
                        self.pos += 1;
                    }
                    if self.pos < self.text.len() {
                        self.advance_to(self.pos + 1);
                    }
                }
                b'\n' | b'\r' | 0 if self.peek() != 0 || self.pos >= self.text.len() => {
                    self.error_at("unterminated string", start, start + 1);
                    break;
                }
                _ => self.advance_to(self.pos + 1),
            }
        }
        STRING_LITERAL
    }

    fn number(&mut self) -> SyntaxKind {
        self.eat_while(|c| c.is_ascii_digit() || c == b'_');
        let mut real = false;
        if self.peek() == b'.' && self.peek_at(1).is_ascii_digit() {
            self.pos += 1;
            self.eat_while(|c| c.is_ascii_digit() || c == b'_');
            real = true;
        }
        if matches!(self.peek(), b'e' | b'E') {
            let sign = usize::from(matches!(self.peek_at(1), b'+' | b'-'));
            if self.peek_at(1 + sign).is_ascii_digit() {
                self.pos += 1 + sign;
                self.eat_while(|c| c.is_ascii_digit() || c == b'_');
                real = true;
            }
        }
        // A time literal: `10ns`, `1.5ps`, `1step`.
        for unit in TIME_UNITS.iter().chain(&["step"]) {
            if self.rest().starts_with(unit) && !is_ident_continue(self.peek_at(unit.len())) {
                self.pos += unit.len();
                return TIME_LITERAL;
            }
        }
        if real {
            return REAL_NUMBER;
        }
        // A size before a base: `8'hFF`, `8 'h FF`.
        let save = self.pos;
        self.eat_blanks();
        if self.peek() == b'\'' && self.based_tail() {
            return BASED_NUMBER;
        }
        self.pos = save;
        INT_NUMBER
    }

    /// At `'`: consumes `'[s]base digits` if that is what follows.
    fn based_tail(&mut self) -> bool {
        let signed = usize::from(matches!(self.peek_at(1), b's' | b'S'));
        if !matches!(
            self.peek_at(1 + signed),
            b'b' | b'B' | b'o' | b'O' | b'd' | b'D' | b'h' | b'H'
        ) {
            return false;
        }
        let save = self.pos;
        self.pos += 2 + signed;
        self.eat_blanks();
        if !is_based_digit(self.peek()) {
            self.pos = save;
            return false;
        }
        self.eat_while(is_based_digit);
        true
    }

    fn tick(&mut self) -> SyntaxKind {
        if self.based_tail() {
            return BASED_NUMBER;
        }
        if matches!(self.peek_at(1), b'0' | b'1' | b'x' | b'X' | b'z' | b'Z')
            && !is_ident_continue(self.peek_at(2))
        {
            self.pos += 2;
            return UNBASED_UNSIZED;
        }
        self.pos += 1;
        TICK
    }

    fn punctuation(&mut self) -> SyntaxKind {
        let rest = self.rest();
        for &(text, kind) in PUNCTUATION {
            if rest.starts_with(text) {
                // `:/` followed by `/` or `*` is `:` then a comment.
                if kind == COLON_SLASH && matches!(self.peek_at(2), b'/' | b'*') {
                    continue;
                }
                self.pos += text.len();
                return kind;
            }
        }
        let start = self.pos;
        self.advance_to(self.pos + 1);
        let end = self.pos;
        self.error_at("unexpected character", start, end);
        ERROR_TOKEN
    }

    // ---- The preprocessor ------------------------------------------------

    /// At `` ` ``: a directive or a macro use.
    fn backtick(&mut self) -> SyntaxKind {
        let start = self.pos;
        if !is_ident_start(self.peek_at(1)) {
            self.pos += 1;
            // `` `" `` and ` `` ` only mean something in macro bodies.
            self.error_at("unexpected `` ` ``", start, self.pos);
            return ERROR_TOKEN;
        }
        self.pos += 1;
        self.eat_while(is_ident_continue);
        let name = &self.text[start + 1..self.pos];
        match name {
            "define" => {
                self.define();
                DIRECTIVE
            }
            "undef" => {
                if let Some(name) = self.directive_word() {
                    self.macros.remove(name);
                }
                DIRECTIVE
            }
            "undefineall" => {
                self.macros.clear();
                DIRECTIVE
            }
            "include" => {
                self.include_directive();
                DIRECTIVE
            }
            "ifdef" | "ifndef" => {
                let defined = self.condition();
                let compiled = defined == (name == "ifdef");
                self.conditionals.push(Conditional {
                    taken: compiled,
                    start,
                });
                if !compiled {
                    self.skip_branch(true);
                }
                DIRECTIVE
            }
            "elsif" => {
                let defined = self.condition();
                match self.conditionals.last_mut() {
                    None => self.error_at("`elsif without `ifdef", start, self.pos),
                    Some(cond) if cond.taken => self.skip_branch(false),
                    Some(cond) => {
                        if defined {
                            cond.taken = true;
                        } else {
                            self.skip_branch(true);
                        }
                    }
                }
                DIRECTIVE
            }
            "else" => {
                match self.conditionals.last_mut() {
                    None => self.error_at("`else without `ifdef", start, self.pos),
                    Some(cond) if cond.taken => self.skip_branch(false),
                    Some(cond) => cond.taken = true,
                }
                DIRECTIVE
            }
            "endif" => {
                if self.conditionals.pop().is_none() {
                    self.error_at("`endif without `ifdef", start, self.pos);
                }
                DIRECTIVE
            }
            "begin_keywords" => {
                let save = self.pos;
                self.eat_blanks();
                if self.peek() == b'"' {
                    let s = self.pos;
                    self.string();
                    let quoted = &self.text[s + 1..self.pos];
                    let version = quoted.strip_suffix('"').unwrap_or(quoted).trim();
                    match version.parse::<Standard>() {
                        Ok(std) => self.standards.push(std),
                        Err(_) => {
                            self.error_at("unknown keyword version", s, self.pos);
                            self.standards.push(self.standard());
                        }
                    }
                } else {
                    self.pos = save;
                    self.error_at("expected a version string", start, self.pos);
                }
                DIRECTIVE
            }
            "end_keywords" => {
                if self.standards.len() > 1 {
                    self.standards.pop();
                } else {
                    self.error_at("`end_keywords without `begin_keywords", start, self.pos);
                }
                DIRECTIVE
            }
            "resetall"
            | "celldefine"
            | "endcelldefine"
            | "nounconnected_drive"
            | "delay_mode_distributed"
            | "delay_mode_path"
            | "delay_mode_unit"
            | "delay_mode_zero"
            | "endprotect"
            | "endprotected" => DIRECTIVE,
            "protect" | "protected" => {
                self.skip_protected(&["`endprotected", "`endprotect"]);
                DIRECTIVE
            }
            "pragma" => {
                self.skip_to_end_of_line();
                let line = &self.text[start..self.pos];
                if line.contains("begin_protected") {
                    self.skip_protected(&["end_protected"]);
                }
                DIRECTIVE
            }
            "timescale"
            | "default_nettype"
            | "unconnected_drive"
            | "line"
            | "default_decay_time"
            | "default_trireg_strength" => {
                self.skip_to_end_of_line();
                DIRECTIVE
            }
            "__FILE__" | "__LINE__" => MACRO_REF,
            _ => match self.macros.get(name) {
                Some(false) => MACRO_REF,
                _ => MACRO_CALL,
            },
        }
    }

    /// Up to the end of the line or a comment, whichever comes first.
    fn skip_to_end_of_line(&mut self) {
        let rest = self.rest();
        let mut end = rest.find(['\n', '\r']).unwrap_or(rest.len());
        for comment in ["//", "/*"] {
            if let Some(i) = rest[..end].find(comment) {
                end = end.min(i);
            }
        }
        // Keep trailing blanks out of the directive.
        let end = rest[..end].trim_end_matches([' ', '\t']).len();
        self.pos += end;
    }

    /// The identifier after a directive, consumed with the blanks before it.
    fn directive_word(&mut self) -> Option<&'a str> {
        let save = self.pos;
        self.eat_blanks();
        let start = self.pos;
        if self.peek() == b'\\' {
            self.eat_while(|c| !is_space(c));
        } else {
            self.eat_while(is_ident_continue);
        }
        if self.pos == start {
            self.pos = save;
            self.error_at("expected a macro name", save, save);
            return None;
        }
        Some(&self.text[start..self.pos])
    }

    /// `` `define NAME[(args)] body ``, with `\` line continuations.
    fn define(&mut self) {
        let Some(name) = self.directive_word() else {
            return;
        };
        let has_args = self.peek() == b'(';
        let bytes = self.bytes();
        let mut pos = self.pos;
        while pos < bytes.len() {
            match bytes[pos] {
                b'\\' if bytes.get(pos + 1) == Some(&b'\n') => pos += 2,
                b'\\' if bytes.get(pos + 1) == Some(&b'\r') => {
                    pos += 2;
                    if bytes.get(pos) == Some(&b'\n') {
                        pos += 1;
                    }
                }
                b'\n' | b'\r' => break,
                // A block comment in the body can span lines.
                b'/' if bytes.get(pos + 1) == Some(&b'*') => {
                    pos = match self.text[pos + 2..].find("*/") {
                        Some(end) => pos + 2 + end + 2,
                        None => {
                            self.error_at("unterminated block comment", pos, pos + 2);
                            bytes.len()
                        }
                    };
                }
                // A line comment ends the body, unless it ends with a line
                // continuation as most tools accept. White space after the
                // `\` does not count, whatever it is: the formatter drops it.
                b'/' if bytes.get(pos + 1) == Some(&b'/') => {
                    let end = self.text[pos..]
                        .find(['\n', '\r'])
                        .map_or(bytes.len(), |e| pos + e);
                    let comment = &self.text[pos..end];
                    if comment.trim_end().ends_with('\\') {
                        // At the end of the file, the line break the
                        // formatter ends it with would be continued. Below
                        // catches a `\` right at the end.
                        if end == bytes.len() && !comment.ends_with('\\') {
                            self.error_at("line continuation at the end of the file", pos, end);
                        }
                        pos = end;
                        if bytes.get(pos) == Some(&b'\r') {
                            pos += 1;
                        }
                        if bytes.get(pos) == Some(&b'\n') {
                            pos += 1;
                        }
                    } else {
                        break;
                    }
                }
                _ => pos += 1,
            }
        }
        let body = &self.text[self.pos..pos];
        let mut end = body.trim_end_matches([' ', '\t']).len();
        if body[..end].ends_with('\\') && matches!(bytes.get(pos), None | Some(b'\n' | b'\r')) {
            if end == body.len() && pos == bytes.len() {
                self.error_at("line continuation at the end of the file", pos - 1, pos);
            }
            // The blanks between a `\` and the end of the line are kept in
            // the directive, so that formatting cannot make it a line
            // continuation.
            end = body.len();
        }
        self.advance_to(self.pos + end);
        self.macros.insert(name.to_owned(), has_args);
    }

    fn include_directive(&mut self) {
        let save = self.pos;
        self.eat_blanks();
        let name = match self.peek() {
            b'"' => {
                let start = self.pos;
                self.string();
                // An unterminated name has no closing quote.
                let quoted = &self.text[start + 1..self.pos];
                Some(quoted.strip_suffix('"').unwrap_or(quoted).to_owned())
            }
            b'<' => match self.rest().find(['>', '\n']) {
                Some(end) if self.rest().as_bytes()[end] == b'>' => {
                    let start = self.pos;
                    self.pos += end + 1;
                    Some(self.text[start + 1..self.pos - 1].to_owned())
                }
                _ => None,
            },
            // `` `include `FILE ``: the name comes from a macro.
            b'`' if is_ident_start(self.peek_at(1)) => {
                self.pos += 1;
                self.eat_while(is_ident_continue);
                if self.peek() == b'(' {
                    // Its arguments, on the same line.
                    let rest = self.rest();
                    let line = rest.find('\n').unwrap_or(rest.len());
                    if let Some(close) = rest[..line].rfind(')') {
                        self.pos += close + 1;
                    }
                }
                return;
            }
            _ => None,
        };
        if name.is_none() {
            self.pos = save;
            self.error_at("expected a file name", save, save);
        }
        let Some(name) = name else { return };
        self.includes.push(name.clone());
        if self.depth >= MAX_INCLUDE_DEPTH || self.reads_left == 0 {
            return;
        }
        let Some(read) = self.include else { return };
        let Some(text) = read(&name) else { return };
        self.reads_left -= 1;
        let options = Options {
            standard: self.standard(),
            defines: &[],
            include: self.include,
        };
        let macros = std::mem::take(&mut self.macros);
        let mut inner = Lexer::new(&text, &options, macros, self.depth + 1);
        inner.reads_left = self.reads_left;
        inner.run();
        self.reads_left = inner.reads_left;
        self.macros = inner.macros;
        self.includes.append(&mut inner.includes);
    }

    /// The condition after `` `ifdef ``, `` `ifndef `` or `` `elsif ``:
    /// whether it holds as if it were an `` `ifdef ``.
    fn condition(&mut self) -> bool {
        let save = self.pos;
        self.eat_blanks();
        if self.peek() == b'(' {
            // IEEE 1800-2023: `` `ifdef (A && !B) ``.
            let start = self.pos;
            let mut depth = 0;
            while self.pos < self.text.len() {
                match self.peek() {
                    b'(' => depth += 1,
                    b')' => {
                        depth -= 1;
                        if depth == 0 {
                            self.pos += 1;
                            break;
                        }
                    }
                    b'\n' => break,
                    _ => {}
                }
                self.advance_to(self.pos + 1);
            }
            let expr = &self.text[start..self.pos];
            return Condition::new(expr, &self.macros).eval();
        }
        self.pos = save;
        match self.directive_word() {
            Some(name) => self.macros.contains_key(name),
            None => false,
        }
    }

    /// Skips a branch that is not compiled, as one `INACTIVE` token, up to
    /// the `` `endif `` that closes it, or the next `` `elsif `` or
    /// `` `else `` if `to_next_branch`. The directive found is lexed next.
    fn skip_branch(&mut self, to_next_branch: bool) {
        // The directive that opened the branch is its own token.
        self.push(DIRECTIVE, self.token_start);
        let start = self.pos;
        let bytes = self.bytes();
        let mut pos = self.pos;
        let mut depth = 0u32;
        while pos < bytes.len() {
            match bytes[pos] {
                b'/' if bytes.get(pos + 1) == Some(&b'/') => {
                    while pos < bytes.len() && bytes[pos] != b'\n' {
                        pos += 1;
                    }
                }
                b'/' if bytes.get(pos + 1) == Some(&b'*') => {
                    pos = match self.text[pos + 2..].find("*/") {
                        Some(end) => pos + 2 + end + 2,
                        None => bytes.len(),
                    };
                }
                // A string ends where it does in code that is compiled
                // (`"""` strings too), so that the lines of a branch lexed
                // alone hold the same tokens. Its errors do not count.
                b'"' => {
                    self.pos = pos;
                    let errors = self.errors.len();
                    self.string();
                    self.errors.truncate(errors);
                    pos = self.pos;
                }
                b'`' => {
                    let word_start = pos + 1;
                    let mut end = word_start;
                    while end < bytes.len() && is_ident_continue(bytes[end]) {
                        end += 1;
                    }
                    match &self.text[word_start..end] {
                        "ifdef" | "ifndef" => depth += 1,
                        "endif" if depth == 0 => break,
                        "endif" => depth -= 1,
                        "elsif" | "else" if depth == 0 && to_next_branch => break,
                        _ => {}
                    }
                    pos = end;
                    continue;
                }
                _ => pos += 1,
            }
        }
        self.advance_to(pos);
        self.push(INACTIVE, start);
        self.token_start = self.pos;
    }

    /// Skips encrypted text after a protection directive, up to the line
    /// holding one of `ends`, as one `INACTIVE` token.
    fn skip_protected(&mut self, ends: &[&str]) {
        self.push(DIRECTIVE, self.token_start);
        let start = self.pos;
        let rest = self.rest();
        let end = ends
            .iter()
            .filter_map(|e| rest.find(e))
            .min()
            .map(|i| rest[..i].rfind('\n').map_or(0, |nl| nl + 1));
        let end = end.unwrap_or_else(|| {
            let directive = self.token_start;
            self.error_at("protected text without its end", directive, start);
            rest.len()
        });
        self.advance_to(self.pos + end);
        self.push(INACTIVE, start);
        self.token_start = self.pos;
    }
}

/// A 1800-2023 `` `ifdef `` condition: macro names with `!`, `&&`, `||`,
/// `->`, `<->` and parentheses.
struct Condition<'t> {
    tokens: Vec<&'t str>,
    pos: usize,
    macros: &'t Macros,
}

impl<'t> Condition<'t> {
    fn new(text: &'t str, macros: &'t Macros) -> Self {
        let mut tokens = Vec::new();
        let bytes = text.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            let c = bytes[i];
            if is_space(c) {
                i += 1;
            } else if is_ident_continue(c) {
                let s = i;
                while i < bytes.len() && is_ident_continue(bytes[i]) {
                    i += 1;
                }
                tokens.push(&text[s..i]);
            } else {
                let len = ["<->", "&&", "||", "->"]
                    .iter()
                    .find(|op| text[i..].starts_with(*op))
                    .map_or(1, |op| op.len());
                let len = (i + len..=text.len())
                    .find(|&e| text.is_char_boundary(e))
                    .unwrap_or(text.len())
                    - i;
                tokens.push(&text[i..i + len]);
                i += len;
            }
        }
        Condition {
            tokens,
            pos: 0,
            macros,
        }
    }

    fn eval(&mut self) -> bool {
        self.implication()
    }

    fn peek(&self) -> &'t str {
        self.tokens.get(self.pos).copied().unwrap_or("")
    }

    fn implication(&mut self) -> bool {
        let lhs = self.or();
        match self.peek() {
            "->" => {
                self.pos += 1;
                let rhs = self.implication();
                !lhs || rhs
            }
            "<->" => {
                self.pos += 1;
                let rhs = self.implication();
                lhs == rhs
            }
            _ => lhs,
        }
    }

    fn or(&mut self) -> bool {
        let mut value = self.and();
        while self.peek() == "||" {
            self.pos += 1;
            value |= self.and();
        }
        value
    }

    fn and(&mut self) -> bool {
        let mut value = self.not();
        while self.peek() == "&&" {
            self.pos += 1;
            value &= self.not();
        }
        value
    }

    fn not(&mut self) -> bool {
        match self.peek() {
            "!" => {
                self.pos += 1;
                !self.not()
            }
            "(" => {
                self.pos += 1;
                let value = self.implication();
                if self.peek() == ")" {
                    self.pos += 1;
                }
                value
            }
            "" => false,
            name => {
                self.pos += 1;
                self.macros.contains_key(name)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(text: &str, std: Standard) -> Vec<(SyntaxKind, &str)> {
        let lexed = lex(
            text,
            &Options {
                standard: std,
                ..Options::default()
            },
        );
        let mut pos = 0;
        let mut out = Vec::new();
        for t in &lexed.tokens {
            let len = usize::from(t.len);
            out.push((t.kind, &text[pos..pos + len]));
            pos += len;
        }
        assert_eq!(pos, text.len());
        out.retain(|(k, _)| *k != WHITESPACE);
        out
    }

    /// Found by fuzzing: an unterminated file name (or keyword version)
    /// ending in a multi-byte character panicked.
    #[test]
    fn unterminated_quoted_names() {
        let lexed = lex("`include \"a®", &Options::default());
        assert_eq!(lexed.includes, ["a®"]);
        let lexed = lex("`begin_keywords \"1364-2005®", &Options::default());
        assert!(!lexed.errors.is_empty());
    }

    /// Found by fuzzing: a macro body ending in `\` at the end of the file,
    /// or before blanks, became a line continuation once formatted.
    #[test]
    fn backslash_at_the_end_of_a_define() {
        let lexed = lex("`define A x \\", &Options::default());
        assert_eq!(lexed.errors.len(), 1, "{:?}", lexed.errors);
        let text = "`define A x \\  \nwire b;\n";
        let toks = kinds(text, Standard::default());
        assert_eq!(toks[0], (DIRECTIVE, "`define A x \\  "), "{toks:?}");
    }

    /// Found by fuzzing: a comment in a macro body ending in `\` and blanks
    /// at the end of the file, or in `\` and a form feed, was continued
    /// once formatted.
    #[test]
    fn backslash_and_blanks_ending_a_comment_in_a_define() {
        for text in ["`define F//\\ ", "`define F // x \\\t"] {
            let lexed = lex(text, &Options::default());
            assert_eq!(lexed.errors.len(), 1, "{text:?}: {:?}", lexed.errors);
        }
        let lexed = lex("`define F//\\", &Options::default());
        assert_eq!(lexed.errors.len(), 1, "{:?}", lexed.errors);
        let toks = kinds("`define A//\\\x0c\nwire b;\n", Standard::default());
        assert_eq!(
            toks[0],
            (DIRECTIVE, "`define A//\\\x0c\nwire b;"),
            "{toks:?}"
        );
    }

    /// Found by fuzzing: protected text with no end ran to the end of the
    /// file with no error, so the file was formatted.
    #[test]
    fn unterminated_protected_text() {
        let lexed = lex("`protected\nabc", &Options::default());
        assert_eq!(lexed.errors.len(), 1, "{:?}", lexed.errors);
        let lexed = lex("`protected\nabc\n`endprotected\n", &Options::default());
        assert!(lexed.errors.is_empty());
    }

    /// Found by fuzzing: a block comment left open in a macro body ran to
    /// the end of the file with no error, so the file was formatted.
    #[test]
    fn unterminated_comment_in_a_define() {
        let lexed = lex("`define A /* x", &Options::default());
        assert_eq!(lexed.errors.len(), 1);
        assert_eq!(lexed.errors[0].message, "unterminated block comment");
    }

    /// Found by fuzzing: a file that includes itself twice took
    /// exponential time.
    #[test]
    fn include_cycles_read_a_bounded_number_of_files() {
        let text = "`include \"a.vh\"\n`include \"a.vh\"\n";
        let reads = std::cell::Cell::new(0u32);
        let read = |_: &str| {
            reads.set(reads.get() + 1);
            Some(text.to_owned())
        };
        let lexed = lex(
            text,
            &Options {
                include: Some(&read),
                ..Options::default()
            },
        );
        assert_eq!(reads.get(), MAX_INCLUDE_READS);
        assert!(lexed.includes.len() <= 2 * MAX_INCLUDE_READS as usize + 2);
    }

    #[test]
    fn numbers() {
        let toks = kinds(
            "8'hFF 'b10 4 'sd 3 12 1.5 1e3 10ns 1step '0 'x 8'(a) '{",
            Standard::Sv2017,
        );
        let ks: Vec<_> = toks.iter().map(|t| t.0).collect();
        assert_eq!(
            ks,
            [
                BASED_NUMBER,
                BASED_NUMBER,
                BASED_NUMBER,
                INT_NUMBER,
                REAL_NUMBER,
                REAL_NUMBER,
                TIME_LITERAL,
                TIME_LITERAL,
                UNBASED_UNSIZED,
                UNBASED_UNSIZED,
                INT_NUMBER,
                TICK,
                LPAREN,
                IDENT,
                RPAREN,
                TICK,
                LBRACE
            ]
        );
    }

    #[test]
    fn attributes_but_not_star_events() {
        let toks = kinds("(* full_case *) @(*) @( * )", Standard::Sv2017);
        assert_eq!(toks[0].0, ATTRIBUTE);
        assert_eq!(toks[2].0, LPAREN);
        assert_eq!(toks[3].0, STAR);
    }

    #[test]
    fn inactive_branches() {
        let text =
            "`define A\n`ifdef A\na\n`else\nb\n`endif\n`ifndef A x `elsif B y `else z `endif";
        let toks = kinds(text, Standard::Sv2017);
        let significant: Vec<_> = toks
            .iter()
            .filter(|t| !t.0.is_trivia())
            .map(|t| t.1)
            .collect();
        assert_eq!(significant, ["a", "z"]);
        assert!(toks.iter().any(|t| t.0 == INACTIVE && t.1.contains('b')));
    }

    #[test]
    fn macros_with_and_without_arguments() {
        let text = "`define W 8\n`define F(x) x\n`W `F `U";
        let toks = kinds(text, Standard::Sv2017);
        let ks: Vec<_> = toks
            .iter()
            .filter(|t| !t.0.is_trivia())
            .map(|t| t.0)
            .collect();
        assert_eq!(ks, [MACRO_REF, MACRO_CALL, MACRO_CALL]);
    }

    #[test]
    fn keywords_follow_begin_keywords() {
        let text = "logic `begin_keywords \"1364-2005\" logic `end_keywords logic";
        let toks = kinds(text, Standard::Sv2017);
        let ks: Vec<_> = toks
            .iter()
            .filter(|t| !t.0.is_trivia())
            .map(|t| t.0)
            .collect();
        assert_eq!(ks, [LOGIC_KW, IDENT, LOGIC_KW]);
    }

    #[test]
    fn condition_expressions() {
        let mut macros = Macros::default();
        macros.insert("A".into(), false);
        assert!(Condition::new("(A && !B)", &macros).eval());
        assert!(!Condition::new("(B || !A)", &macros).eval());
        assert!(Condition::new("(B -> A)", &macros).eval());
    }

    #[test]
    fn define_with_continuation() {
        let text = "`define M(a) \\\n  a + 1\nx";
        let toks = kinds(text, Standard::Sv2017);
        assert_eq!(toks[0].0, DIRECTIVE);
        assert_eq!(toks[0].1, "`define M(a) \\\n  a + 1");
        assert_eq!(toks[1].1, "x");
    }
}

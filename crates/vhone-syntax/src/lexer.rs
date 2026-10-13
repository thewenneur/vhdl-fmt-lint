//! The lexer: splits source text into tokens, trivia included.
//!
//! Lexing never fails. Malformed input produces tokens that still cover every
//! byte, plus an error for each problem found.

use rowan::{TextRange, TextSize};

use crate::{Standard, SyntaxError, SyntaxKind};
use SyntaxKind::*;

/// One token: its kind and its length in bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Token {
    pub kind: SyntaxKind,
    pub len: TextSize,
}

/// Splits `text` into tokens. Concatenating the tokens gives back `text`.
pub fn lex(text: &str, std: Standard) -> (Vec<Token>, Vec<SyntaxError>) {
    let mut lexer = Lexer {
        text,
        pos: 0,
        std,
        tokens: Vec::with_capacity(text.len() / 4),
        errors: Vec::new(),
        prev_significant: EOF,
    };
    while lexer.pos < text.len() {
        let start = lexer.pos;
        let kind = lexer.next_token();
        debug_assert!(lexer.pos > start, "lexer must make progress");
        lexer.tokens.push(Token {
            kind,
            len: TextSize::from((lexer.pos - start) as u32),
        });
        if !kind.is_trivia() {
            lexer.prev_significant = kind;
        }
    }
    (lexer.tokens, lexer.errors)
}

struct Lexer<'a> {
    text: &'a str,
    pos: usize,
    std: Standard,
    tokens: Vec<Token>,
    errors: Vec<SyntaxError>,
    prev_significant: SyntaxKind,
}

/// Base specifiers of bit string literals, lower-cased.
const BASE_SPECIFIERS: [&str; 10] = ["b", "o", "x", "d", "ub", "uo", "ux", "sb", "so", "sx"];

fn is_ident_start(c: char) -> bool {
    c.is_ascii_alphabetic() || (!c.is_ascii() && c.is_alphabetic())
}

fn is_ident_continue(c: char) -> bool {
    c == '_' || c.is_ascii_alphanumeric() || (!c.is_ascii() && c.is_alphanumeric())
}

fn is_whitespace(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r' | '\x0b' | '\x0c' | '\u{a0}')
}

impl Lexer<'_> {
    fn rest(&self) -> &str {
        &self.text[self.pos..]
    }

    fn peek(&self) -> Option<char> {
        self.rest().chars().next()
    }

    fn peek_nth(&self, n: usize) -> Option<char> {
        self.rest().chars().nth(n)
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.pos += c.len_utf8();
        Some(c)
    }

    fn eat(&mut self, c: char) -> bool {
        if self.peek() == Some(c) {
            self.pos += c.len_utf8();
            true
        } else {
            false
        }
    }

    fn bump_while(&mut self, pred: impl Fn(char) -> bool) {
        while let Some(c) = self.peek() {
            if !pred(c) {
                break;
            }
            self.pos += c.len_utf8();
        }
    }

    fn error(&mut self, start: usize, message: impl Into<String>) {
        self.errors.push(SyntaxError::new(
            message,
            TextRange::new((start as u32).into(), (self.pos as u32).into()),
        ));
    }

    fn next_token(&mut self) -> SyntaxKind {
        let start = self.pos;
        let c = self.bump().expect("next_token called at end of input");
        match c {
            c if is_whitespace(c) => {
                self.bump_while(is_whitespace);
                WHITESPACE
            }
            '-' if self.peek() == Some('-') => {
                self.bump_while(|c| c != '\n' && c != '\r');
                LINE_COMMENT
            }
            '/' if self.peek() == Some('*') => {
                self.bump();
                match self.rest().find("*/") {
                    Some(end) => self.pos += end + 2,
                    None => {
                        self.pos = self.text.len();
                        self.error(start, "unterminated block comment");
                    }
                }
                if self.std < Standard::Vhdl2008 {
                    self.error(start, "block comments require VHDL-2008");
                }
                BLOCK_COMMENT
            }
            '`' => {
                self.bump_while(|c| c != '\n' && c != '\r');
                if self.std < Standard::Vhdl2019 {
                    self.error(start, "tool directives require VHDL-2019");
                }
                TOOL_DIRECTIVE
            }
            c if is_ident_start(c) => self.ident_or_keyword(start),
            '0'..='9' => self.number(start),
            '"' => {
                self.string_body(start, '"');
                STRING_LITERAL
            }
            '%' => {
                // `%` is the obsolete replacement for `"` as a string delimiter.
                self.string_body(start, '%');
                STRING_LITERAL
            }
            '\\' => {
                self.extended_identifier(start);
                EXT_IDENT
            }
            '\'' => self.tick_or_char(),
            _ => self.delimiter(start, c),
        }
    }

    fn ident_or_keyword(&mut self, start: usize) -> SyntaxKind {
        self.bump_while(is_ident_continue);
        let word = &self.text[start..self.pos];
        let quote = self.peek().filter(|&c| c == '"' || c == '%');
        if let Some(quote) = quote.filter(|_| is_base_specifier(word)) {
            self.bump();
            self.string_body(start, quote);
            return BIT_STRING_LITERAL;
        }
        // The longest reserved word is "configuration" (13 bytes).
        if word.len() <= 13 && word.is_ascii() {
            let mut buf = [0u8; 13];
            let lower = &mut buf[..word.len()];
            lower.copy_from_slice(word.as_bytes());
            lower.make_ascii_lowercase();
            let lower = std::str::from_utf8(lower).expect("ASCII is UTF-8");
            if let Some(kind) = SyntaxKind::keyword(lower, self.std) {
                return kind;
            }
        }
        IDENT
    }

    fn digits(&mut self) {
        self.bump_while(|c| c.is_ascii_digit() || c == '_');
    }

    fn exponent(&mut self) {
        let rest = self.rest().as_bytes();
        let has_exponent = matches!(
            rest,
            [b'e' | b'E', b'0'..=b'9', ..] | [b'e' | b'E', b'+' | b'-', b'0'..=b'9', ..]
        );
        if has_exponent {
            self.bump();
            if !self.eat('+') {
                self.eat('-');
            }
            self.digits();
        }
    }

    fn number(&mut self, start: usize) -> SyntaxKind {
        self.digits();
        // `:` is the obsolete replacement for `#`, as in `16:FF:`.
        let delimiter = match self.peek() {
            Some('#') => Some('#'),
            Some(':') if self.is_colon_based_literal() => Some(':'),
            _ => None,
        };
        if let Some(delimiter) = delimiter {
            self.bump();
            self.bump_while(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.');
            if !self.eat(delimiter) {
                self.error(start, "unterminated based literal");
                return BASED_LITERAL;
            }
            self.exponent();
            return BASED_LITERAL;
        }
        if self.peek() == Some('.') && self.peek_nth(1).is_some_and(|c| c.is_ascii_digit()) {
            self.bump();
            self.digits();
            self.exponent();
            return DECIMAL_LITERAL;
        }
        // A sized bit string literal, such as `12ux"F"` (VHDL-2008).
        let spec_len = self
            .rest()
            .find(|c: char| !c.is_ascii_alphabetic())
            .unwrap_or(self.rest().len());
        if spec_len > 0
            && self.rest()[spec_len..].starts_with('"')
            && is_base_specifier(&self.rest()[..spec_len])
        {
            self.pos += spec_len + 1;
            self.string_body(start, '"');
            if self.std < Standard::Vhdl2008 {
                self.error(start, "sized bit string literals require VHDL-2008");
            }
            return BIT_STRING_LITERAL;
        }
        self.exponent();
        DECIMAL_LITERAL
    }

    /// Whether the `:` at the cursor opens a based literal like `16:FF:`.
    fn is_colon_based_literal(&self) -> bool {
        let rest = &self.rest()[1..];
        let digits = rest
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '.'))
            .unwrap_or(rest.len());
        digits > 0 && rest[digits..].starts_with(':') && rest.as_bytes()[0].is_ascii_hexdigit()
    }

    /// Consumes the rest of a string whose opening `quote` was just bumped.
    fn string_body(&mut self, start: usize, quote: char) {
        loop {
            match self.peek() {
                Some(c) if c == quote => {
                    self.bump();
                    if !self.eat(quote) {
                        return;
                    }
                }
                Some('\n' | '\r') | None => {
                    self.error(start, "unterminated string literal");
                    return;
                }
                Some(_) => {
                    self.bump();
                }
            }
        }
    }

    fn extended_identifier(&mut self, start: usize) {
        loop {
            match self.peek() {
                Some('\\') => {
                    self.bump();
                    if !self.eat('\\') {
                        return;
                    }
                }
                Some('\n' | '\r') | None => {
                    self.error(start, "unterminated extended identifier");
                    return;
                }
                Some(_) => {
                    self.bump();
                }
            }
        }
    }

    /// `'` is an attribute tick after a name, and otherwise starts a character literal.
    fn tick_or_char(&mut self) -> SyntaxKind {
        let after_name = matches!(
            self.prev_significant,
            IDENT | EXT_IDENT | RPAREN | RBRACKET | ALL_KW | STRING_LITERAL
        );
        let mut chars = self.rest().chars();
        let is_char_literal = match (chars.next(), chars.next()) {
            (Some(c), Some('\'')) => !c.is_control(),
            _ => false,
        };
        // `t'('a')` and `x'('0', others => '1')` are qualified expressions,
        // which the name check below already handles.
        if is_char_literal && !after_name {
            self.bump();
            self.bump();
            CHAR_LITERAL
        } else {
            TICK
        }
    }

    fn delimiter(&mut self, start: usize, c: char) -> SyntaxKind {
        match c {
            '&' => AMP,
            '(' => LPAREN,
            ')' => RPAREN,
            '*' => {
                if self.eat('*') {
                    STAR2
                } else {
                    STAR
                }
            }
            '+' => PLUS,
            ',' => COMMA,
            '-' => MINUS,
            '.' => DOT,
            '/' => {
                if self.eat('=') {
                    NEQ
                } else {
                    SLASH
                }
            }
            ':' => {
                if self.eat('=') {
                    COLON_EQ
                } else {
                    COLON
                }
            }
            ';' => SEMI,
            '<' => {
                if self.eat('=') {
                    LTE
                } else if self.eat('>') {
                    BOX
                } else if self.eat('<') {
                    LTLT
                } else {
                    LT
                }
            }
            '=' => {
                if self.eat('>') {
                    ARROW
                } else {
                    EQ
                }
            }
            '>' => {
                if self.eat('=') {
                    GTE
                } else if self.eat('>') {
                    GTGT
                } else {
                    GT
                }
            }
            '|' | '!' => BAR,
            '[' => LBRACKET,
            ']' => RBRACKET,
            '{' => LBRACE,
            '}' => RBRACE,
            '^' => CARET,
            '@' => AT,
            '?' => {
                if self.eat('?') {
                    QQ
                } else if self.eat('=') {
                    QEQ
                } else if self.rest().starts_with("/=") {
                    self.pos += 2;
                    QNEQ
                } else if self.eat('<') {
                    if self.eat('=') { QLTE } else { QLT }
                } else if self.eat('>') {
                    if self.eat('=') { QGTE } else { QGT }
                } else {
                    QUESTION
                }
            }
            _ => {
                self.error(
                    start,
                    format!("unexpected character `{}`", c.escape_debug()),
                );
                ERROR_TOKEN
            }
        }
    }
}

fn is_base_specifier(word: &str) -> bool {
    word.len() <= 2
        && BASE_SPECIFIERS
            .iter()
            .any(|spec| spec.eq_ignore_ascii_case(word))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(text: &str) -> Vec<(SyntaxKind, String)> {
        let (tokens, errors) = lex(text, Standard::Vhdl2008);
        assert!(errors.is_empty(), "{errors:?}");
        let mut pos = 0;
        tokens
            .into_iter()
            .filter_map(|t| {
                let len = u32::from(t.len) as usize;
                let s = text[pos..pos + len].to_owned();
                pos += len;
                (t.kind != WHITESPACE).then_some((t.kind, s))
            })
            .collect()
    }

    fn just_kinds(text: &str) -> Vec<SyntaxKind> {
        kinds(text).into_iter().map(|(k, _)| k).collect()
    }

    #[test]
    fn keywords_are_case_insensitive() {
        assert_eq!(just_kinds("ENTITY Entity entity"), [ENTITY_KW; 3]);
    }

    #[test]
    fn tick_after_name_is_an_attribute() {
        assert_eq!(just_kinds("x'length"), [IDENT, TICK, IDENT]);
        assert_eq!(
            just_kinds("f(a)'range"),
            [IDENT, LPAREN, IDENT, RPAREN, TICK, RANGE_KW]
        );
        assert_eq!(
            just_kinds("t'('a')"),
            [IDENT, TICK, LPAREN, CHAR_LITERAL, RPAREN]
        );
    }

    #[test]
    fn char_literals() {
        assert_eq!(
            just_kinds("x <= '1' when '''"),
            [IDENT, LTE, CHAR_LITERAL, WHEN_KW, CHAR_LITERAL]
        );
    }

    #[test]
    fn numbers() {
        assert_eq!(
            just_kinds("1_000 3.14 1.0e-3 16#FF# 2#1.1#E+2 1E6 16:E:E1"),
            [
                DECIMAL_LITERAL,
                DECIMAL_LITERAL,
                DECIMAL_LITERAL,
                BASED_LITERAL,
                BASED_LITERAL,
                DECIMAL_LITERAL,
                BASED_LITERAL
            ]
        );
        assert_eq!(just_kinds("10 ns"), [DECIMAL_LITERAL, IDENT]);
    }

    #[test]
    fn bit_strings() {
        assert_eq!(
            just_kinds(r#"x"FF" B"1010" 12UX"F" sx"f" o"7""#),
            [BIT_STRING_LITERAL; 5]
        );
        assert_eq!(just_kinds("x \"F\""), [IDENT, STRING_LITERAL]);
    }

    #[test]
    fn strings_and_extended_identifiers() {
        assert_eq!(
            kinds(r#""say ""hi""" \a\\b\"#),
            [
                (STRING_LITERAL, r#""say ""hi""""#.into()),
                (EXT_IDENT, r"\a\\b\".into())
            ]
        );
    }

    #[test]
    fn comments() {
        assert_eq!(
            just_kinds("a -- c\n/* b\n */ b"),
            [IDENT, LINE_COMMENT, BLOCK_COMMENT, IDENT]
        );
    }

    #[test]
    fn compound_delimiters() {
        assert_eq!(
            just_kinds("<= >= => := /= <> ** ?? ?= ?/= ?< ?<= ?> ?>= << >>"),
            [
                LTE, GTE, ARROW, COLON_EQ, NEQ, BOX, STAR2, QQ, QEQ, QNEQ, QLT, QLTE, QGT, QGTE,
                LTLT, GTGT
            ]
        );
    }

    #[test]
    fn errors_still_cover_every_byte() {
        let text = "a $ \"open\n\\b";
        let (tokens, errors) = lex(text, Standard::Vhdl2008);
        let total: u32 = tokens.iter().map(|t| u32::from(t.len)).sum();
        assert_eq!(total as usize, text.len());
        assert_eq!(errors.len(), 3);
    }
}

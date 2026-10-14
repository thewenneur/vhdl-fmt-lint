//! Parser infrastructure: token cursor, tree building, errors and recovery.
//!
//! The grammar itself lives in [`crate::grammar`], as more `impl Parser` blocks.

use rowan::{Checkpoint, GreenNode, GreenNodeBuilder, TextRange, TextSize};

use crate::lexer::Token;
use crate::{SyntaxError, SyntaxKind};
use SyntaxKind::*;

/// A set of token kinds.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct TokenSet([u64; 8]);

impl TokenSet {
    pub const EMPTY: TokenSet = TokenSet([0; 8]);

    pub const fn new(kinds: &[SyntaxKind]) -> TokenSet {
        let mut bits = [0u64; 8];
        let mut i = 0;
        while i < kinds.len() {
            let k = kinds[i] as usize;
            bits[k / 64] |= 1 << (k % 64);
            i += 1;
        }
        TokenSet(bits)
    }

    pub const fn union(self, other: TokenSet) -> TokenSet {
        let mut bits = self.0;
        let mut i = 0;
        while i < 8 {
            bits[i] |= other.0[i];
            i += 1;
        }
        TokenSet(bits)
    }

    pub const fn contains(self, kind: SyntaxKind) -> bool {
        let k = kind as usize;
        k < 512 && self.0[k / 64] & (1 << (k % 64)) != 0
    }
}

pub(crate) struct Parser<'t> {
    text: &'t str,
    tokens: &'t [Token],
    /// Start offset of each token.
    starts: Vec<TextSize>,
    /// Indices of the non-trivia tokens.
    significant: Vec<usize>,
    /// Next raw token to emit into the tree.
    raw_pos: usize,
    /// Next significant token, as an index into `significant`.
    pos: usize,
    /// End offset of the last significant token consumed.
    prev_end: TextSize,
    /// Kind of the last significant token consumed.
    last_kind: SyntaxKind,
    builder: GreenNodeBuilder<'static>,
    pub(crate) errors: Vec<SyntaxError>,
    /// Number of nodes started and not finished yet.
    open_nodes: u32,
    /// Nesting depth, to bail out of pathological input instead of overflowing the stack.
    depth: u32,
}

const MAX_DEPTH: u32 = 128;

impl<'t> Parser<'t> {
    pub(crate) fn new(text: &'t str, tokens: &'t [Token]) -> Self {
        let mut starts = Vec::with_capacity(tokens.len() + 1);
        let mut offset = TextSize::from(0);
        for token in tokens {
            starts.push(offset);
            offset += token.len;
        }
        starts.push(offset);
        let significant = tokens
            .iter()
            .enumerate()
            .filter(|(_, t)| !t.kind.is_trivia())
            .map(|(i, _)| i)
            .collect();
        Parser {
            text,
            tokens,
            starts,
            significant,
            raw_pos: 0,
            pos: 0,
            prev_end: 0.into(),
            last_kind: EOF,
            builder: GreenNodeBuilder::new(),
            errors: Vec::new(),
            depth: 0,
            open_nodes: 0,
        }
    }

    /// Starts the root node, which also holds any leading trivia.
    pub(crate) fn start_root(&mut self, kind: SyntaxKind) {
        self.builder.start_node(kind.into());
        self.open_nodes += 1;
    }

    pub(crate) fn finish(mut self) -> (GreenNode, Vec<SyntaxError>) {
        self.flush_trivia(self.tokens.len());
        self.builder.finish_node();
        (self.builder.finish(), self.errors)
    }

    // ---- Looking at tokens ----------------------------------------------

    pub(crate) fn nth(&self, n: usize) -> SyntaxKind {
        self.significant
            .get(self.pos + n)
            .map_or(EOF, |&i| self.tokens[i].kind)
    }

    pub(crate) fn current(&self) -> SyntaxKind {
        self.nth(0)
    }

    pub(crate) fn at(&self, kind: SyntaxKind) -> bool {
        self.current() == kind
    }

    pub(crate) fn at_set(&self, set: TokenSet) -> bool {
        set.contains(self.current())
    }

    pub(crate) fn nth_at(&self, n: usize, kind: SyntaxKind) -> bool {
        self.nth(n) == kind
    }

    pub(crate) fn at_eof(&self) -> bool {
        self.pos >= self.significant.len()
    }

    /// Text of the `n`th significant token ahead.
    pub(crate) fn nth_text(&self, n: usize) -> &'t str {
        match self.significant.get(self.pos + n) {
            Some(&i) => {
                let range = TextRange::new(self.starts[i], self.starts[i + 1]);
                &self.text[range]
            }
            None => "",
        }
    }

    /// Kind of the last significant token consumed.
    pub(crate) fn last_kind(&self) -> SyntaxKind {
        self.last_kind
    }

    /// An opaque position, to check that a loop makes progress.
    pub(crate) fn progress(&self) -> usize {
        self.pos
    }

    fn current_range(&self) -> TextRange {
        match self.significant.get(self.pos) {
            Some(&i) => TextRange::new(self.starts[i], self.starts[i + 1]),
            None => TextRange::empty(*self.starts.last().expect("starts is never empty")),
        }
    }

    // ---- Building the tree ----------------------------------------------

    fn flush_trivia(&mut self, until: usize) {
        while self.raw_pos < until {
            let token = self.tokens[self.raw_pos];
            let range = TextRange::new(self.starts[self.raw_pos], self.starts[self.raw_pos + 1]);
            self.builder.token(token.kind.into(), &self.text[range]);
            self.raw_pos += 1;
        }
    }

    fn flush_leading_trivia(&mut self) {
        let next = self
            .significant
            .get(self.pos)
            .copied()
            .unwrap_or(self.tokens.len());
        self.flush_trivia(next);
    }

    /// Consumes the current token, whatever it is.
    pub(crate) fn bump(&mut self) {
        let Some(&i) = self.significant.get(self.pos) else {
            return;
        };
        self.flush_trivia(i);
        self.flush_trivia(i + 1);
        self.prev_end = self.starts[i + 1];
        self.last_kind = self.tokens[i].kind;
        self.pos += 1;
    }

    /// Consumes the current token if it is `kind`.
    pub(crate) fn eat(&mut self, kind: SyntaxKind) -> bool {
        if self.at(kind) {
            self.bump();
            true
        } else {
            false
        }
    }

    /// Consumes `kind`, or reports that it is missing.
    pub(crate) fn expect(&mut self, kind: SyntaxKind) -> bool {
        if self.eat(kind) {
            true
        } else {
            self.error_expected(&kind.describe());
            false
        }
    }

    /// Starts a node. Trivia before it stays in the parent.
    pub(crate) fn start(&mut self, kind: SyntaxKind) {
        self.flush_leading_trivia();
        self.builder.start_node(kind.into());
        self.open_nodes += 1;
    }

    pub(crate) fn finish_node(&mut self) {
        // The root is closed by `finish`, after the trailing trivia.
        assert!(self.open_nodes > 1, "unbalanced finish_node");
        self.open_nodes -= 1;
        self.builder.finish_node();
    }

    pub(crate) fn checkpoint(&mut self) -> Checkpoint {
        self.flush_leading_trivia();
        self.builder.checkpoint()
    }

    /// Starts a node that wraps everything since `checkpoint`.
    pub(crate) fn start_at(&mut self, checkpoint: Checkpoint, kind: SyntaxKind) {
        self.builder.start_node_at(checkpoint, kind.into());
        self.open_nodes += 1;
    }

    /// Runs `f` one level deeper, unless the input nests absurdly deep.
    pub(crate) fn nested(&mut self, f: impl FnOnce(&mut Self)) {
        if self.depth >= MAX_DEPTH {
            self.error_here("nesting too deep");
            self.bump_error();
            return;
        }
        self.depth += 1;
        f(self);
        self.depth -= 1;
    }

    // ---- Errors and recovery ----------------------------------------------

    /// Reports an error on the current token.
    pub(crate) fn error_here(&mut self, message: impl Into<String>) {
        let range = self.current_range();
        self.errors.push(SyntaxError::new(message, range));
    }

    /// Reports that `what` was expected, just after the previous token.
    pub(crate) fn error_expected(&mut self, what: &str) {
        let found = if self.at_eof() {
            "end of file".to_owned()
        } else {
            format!("`{}`", self.nth_text(0))
        };
        let message = format!("expected {what}, found {found}");
        // Don't pile up errors at the same spot.
        let at = self.prev_end;
        if self.errors.last().is_some_and(|e| e.range.start() == at) {
            return;
        }
        self.errors
            .push(SyntaxError::new(message, TextRange::empty(at)));
    }

    /// Wraps the current token in an `ERROR` node.
    pub(crate) fn bump_error(&mut self) {
        if self.at_eof() {
            return;
        }
        self.start(ERROR);
        self.bump();
        self.finish_node();
    }

    /// Reports `what` as expected and skips tokens into an `ERROR` node until
    /// one in `stop` (not consumed), or a `;` (consumed).
    pub(crate) fn recover(&mut self, what: &str, stop: TokenSet) {
        self.error_expected(what);
        if self.at_set(stop) || self.at_eof() {
            return;
        }
        self.start(ERROR);
        let mut depth = 0u32;
        while !self.at_eof() {
            match self.current() {
                LPAREN | LBRACKET | LBRACE => depth += 1,
                RPAREN | RBRACKET | RBRACE => depth = depth.saturating_sub(1),
                SEMI if depth == 0 => {
                    self.bump();
                    break;
                }
                kind if depth == 0 && stop.contains(kind) => break,
                _ => {}
            }
            self.bump();
        }
        self.finish_node();
    }
}

//! The Verilog and SystemVerilog grammar, as recursive descent over
//! [`Parser`].
//!
//! Each function parses one construct and leaves the cursor after it. On
//! malformed input a function reports an error and returns without
//! consuming what it doesn't understand; the enclosing list loops skip such
//! tokens, so every token always ends up in the tree.
//!
//! Item lists accept a superset of what each construct allows (a module
//! item in a package, say): that keeps the grammar small and the tree
//! useful on code that a stricter parser would reject. Checking what goes
//! where is left to later passes.

mod expressions;
mod items;
mod statements;
mod types;

use crate::SyntaxKind::{self, *};
use crate::parser::{Parser, TokenSet};

/// Keywords that end a design element. Item lists stop at them.
const UNIT_END: TokenSet = TokenSet::new(&[
    ENDMODULE_KW,
    ENDINTERFACE_KW,
    ENDPROGRAM_KW,
    ENDPACKAGE_KW,
    ENDPRIMITIVE_KW,
    ENDCONFIG_KW,
    ENDCHECKER_KW,
    ENDCLASS_KW,
]);

/// Keywords that end any construct. Statement lists stop at them.
const ANY_END: TokenSet = UNIT_END.union(TokenSet::new(&[
    END_KW,
    ENDCASE_KW,
    ENDFUNCTION_KW,
    ENDTASK_KW,
    ENDGENERATE_KW,
    JOIN_KW,
    JOIN_ANY_KW,
    JOIN_NONE_KW,
    ENDSPECIFY_KW,
    ENDCLOCKING_KW,
    ENDGROUP_KW,
    ENDPROPERTY_KW,
    ENDSEQUENCE_KW,
    ENDTABLE_KW,
]));

/// Tokens that can be a name.
const NAME_TOKENS: TokenSet = TokenSet::new(&[IDENT, ESC_IDENT, MACRO_REF, MACRO_CALL]);

impl Parser<'_> {
    pub(crate) fn source_file(&mut self) {
        self.start_root(SOURCE_FILE);
        while !self.at_eof() {
            let progress = self.progress();
            if self.at_set(ANY_END) {
                self.error_here(format!("unexpected `{}`", self.nth_text(0)));
                self.bump_error();
            } else {
                self.item();
            }
            if self.progress() == progress {
                self.bump_error();
            }
        }
        // The root node is closed by `Parser::finish`, after the trailing trivia.
    }

    /// Whether the `n`th token ahead can be a name.
    pub(crate) fn nth_is_name(&self, n: usize) -> bool {
        NAME_TOKENS.contains(self.nth(n))
    }

    /// A declared name, in a `NAME` node.
    pub(crate) fn name(&mut self) {
        if self.at_set(NAME_TOKENS) {
            self.start(NAME);
            self.bump_name_token();
            self.finish_node();
        } else {
            self.error_expected("identifier");
        }
    }

    /// Consumes a name token, with the arguments of a macro.
    pub(crate) fn bump_name_token(&mut self) {
        let macro_call = self.at(MACRO_CALL);
        self.bump();
        if macro_call && self.at(LPAREN) {
            self.macro_args();
        }
    }

    /// `: name` after an `end` keyword.
    pub(crate) fn end_label(&mut self) {
        if self.at(COLON) {
            self.start(END_LABEL);
            self.bump();
            if self.at_set(NAME_TOKENS) || self.at(NEW_KW) {
                self.bump_name_token();
            } else {
                self.error_expected("identifier");
            }
            self.finish_node();
        }
    }

    /// `name :` before a statement or an item, if there is one.
    pub(crate) fn opt_label(&mut self) -> bool {
        if matches!(self.current(), IDENT | ESC_IDENT) && self.nth_at(1, COLON) {
            self.start(LABEL);
            self.bump();
            self.bump();
            self.finish_node();
            true
        } else {
            false
        }
    }

    /// The `(...)` after a macro name, unparsed.
    pub(crate) fn macro_args(&mut self) {
        self.start(MACRO_ARGS);
        self.balanced();
        self.finish_node();
    }

    /// At `(`, `[` or `{`: consumes up to and including the matching
    /// closing bracket.
    pub(crate) fn balanced(&mut self) {
        let mut depth = 0u32;
        while !self.at_eof() {
            match self.current() {
                LPAREN | LBRACKET | LBRACE => depth += 1,
                RPAREN | RBRACKET | RBRACE => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        self.bump();
                        return;
                    }
                }
                _ => {}
            }
            self.bump();
        }
        self.error_expected("closing bracket");
    }

    /// The index just after the bracket group that opens at the `n`th
    /// token ahead.
    pub(crate) fn skip_group_ahead(&self, mut n: usize) -> usize {
        let mut depth = 0u32;
        loop {
            match self.nth(n) {
                EOF => return n,
                LPAREN | LBRACKET | LBRACE => depth += 1,
                RPAREN | RBRACKET | RBRACE => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        return n + 1;
                    }
                }
                _ => {}
            }
            n += 1;
        }
    }

    /// Consumes tokens up to a `;` at bracket depth 0 (included), in a
    /// `kind` node.
    pub(crate) fn opaque_to_semi(&mut self, kind: SyntaxKind) {
        self.start(kind);
        let mut depth = 0u32;
        while !self.at_eof() {
            match self.current() {
                LPAREN | LBRACKET | LBRACE => depth += 1,
                RPAREN | RBRACKET | RBRACE => depth = depth.saturating_sub(1),
                SEMI if depth == 0 => {
                    self.bump();
                    break;
                }
                k if depth == 0 && UNIT_END.contains(k) => {
                    self.error_expected("`;`");
                    break;
                }
                _ => {}
            }
            self.bump();
        }
        self.finish_node();
    }

    /// Consumes tokens up to the `end` keyword that closes the construct
    /// (included), with its label, in a `kind` node. If `nested`, `open`
    /// keywords open inner constructs of the same kind.
    pub(crate) fn opaque_block(
        &mut self,
        kind: SyntaxKind,
        open: SyntaxKind,
        end: SyntaxKind,
        nested: bool,
    ) {
        self.start(kind);
        self.bump();
        let mut depth = 1u32;
        while !self.at_eof() {
            let k = self.current();
            if k == end {
                depth -= 1;
                self.bump();
                if depth == 0 {
                    self.end_label();
                    self.finish_node();
                    return;
                }
                continue;
            }
            // `typedef class c;` declares no class body.
            if nested && k == open && self.last_kind() != TYPEDEF_KW {
                depth += 1;
            }
            self.bump();
        }
        self.error_expected(&end.describe());
        self.finish_node();
    }
}

/// Whether `kind` is an assignment operator, `=` excluded.
fn is_compound_assign(kind: SyntaxKind) -> bool {
    matches!(
        kind,
        PLUS_EQ
            | MINUS_EQ
            | STAR_EQ
            | SLASH_EQ
            | PERCENT_EQ
            | AMP_EQ
            | PIPE_EQ
            | CARET_EQ
            | SHL_EQ
            | SHR_EQ
            | ASHL_EQ
            | ASHR_EQ
    )
}

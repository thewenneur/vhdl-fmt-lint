//! Expressions, by precedence climbing.

use super::is_compound_assign;
use super::types::TYPE_KEYWORDS;
use crate::SyntaxKind::{self, *};
use crate::parser::{Parser, TokenSet};

/// Unary operators.
const UNARY_OPS: TokenSet = TokenSet::new(&[
    PLUS,
    MINUS,
    BANG,
    TILDE,
    AMP,
    TILDE_AMP,
    PIPE,
    TILDE_PIPE,
    CARET,
    TILDE_CARET,
    PLUS2,
    MINUS2,
]);

/// Binding power of the operands of unary operators.
const UNARY_BP: u8 = 30;

/// The left and right binding powers of a binary operator.
fn binary_bp(kind: SyntaxKind) -> Option<(u8, u8)> {
    Some(match kind {
        ARROW | EQUIV => (2, 1),
        PIPE2 => (4, 5),
        AMP2 => (6, 7),
        PIPE => (8, 9),
        CARET | TILDE_CARET => (10, 11),
        AMP => (12, 13),
        EQ2 | NEQ | EQ3 | NEQ2 | EQ2_Q | NEQ_Q => (14, 15),
        LT | LTE | GT | GTE => (16, 17),
        SHL | SHR | ASHL | ASHR => (18, 19),
        PLUS | MINUS => (20, 21),
        STAR | SLASH | PERCENT => (22, 23),
        STAR2 => (24, 25),
        _ => return None,
    })
}

/// Binding power of `?:`, `inside` and `dist`.
const CONDITIONAL_BP: u8 = 3;
const INSIDE_BP: u8 = 16;

impl Parser<'_> {
    pub(crate) fn expr(&mut self) {
        self.expr_bp(0);
    }

    fn expr_bp(&mut self, min_bp: u8) {
        self.nested(|p| p.expr_bp_inner(min_bp));
    }

    fn expr_bp_inner(&mut self, min_bp: u8) {
        let checkpoint = self.checkpoint();
        self.unary_expr();
        loop {
            let op = self.current();
            if op == QUESTION {
                if CONDITIONAL_BP < min_bp {
                    break;
                }
                self.bump();
                self.expr_bp(0);
                self.expect(COLON);
                self.expr_bp(CONDITIONAL_BP);
                self.start_at(checkpoint, CONDITIONAL_EXPR);
                self.finish_node();
                continue;
            }
            if op == MATCHES_KW {
                if INSIDE_BP < min_bp {
                    break;
                }
                self.bump();
                self.pattern();
                self.start_at(checkpoint, MATCHES_EXPR);
                self.finish_node();
                continue;
            }
            if op == INSIDE_KW || op == DIST_KW {
                if INSIDE_BP < min_bp {
                    break;
                }
                self.bump();
                self.range_list();
                self.start_at(checkpoint, INSIDE_EXPR);
                self.finish_node();
                continue;
            }
            let Some((left, right)) = binary_bp(op) else {
                break;
            };
            if left < min_bp {
                break;
            }
            self.bump();
            self.expr_bp(right);
            self.start_at(checkpoint, BINARY_EXPR);
            self.finish_node();
        }
    }

    /// A unary expression: operators, then a primary with its selects.
    pub(crate) fn unary_expr(&mut self) {
        if self.at_set(UNARY_OPS) {
            self.start(UNARY_EXPR);
            self.bump();
            self.expr_bp(UNARY_BP);
            self.finish_node();
        } else {
            self.primary_expr();
        }
    }

    /// A primary followed by selects, calls and casts.
    pub(crate) fn primary_expr(&mut self) {
        let checkpoint = self.checkpoint();
        let callable = self.primary();
        self.postfix(checkpoint, callable);
    }

    /// Returns whether a call can follow (the primary is a name).
    fn primary(&mut self) -> bool {
        match self.current() {
            INT_NUMBER | BASED_NUMBER | REAL_NUMBER | TIME_LITERAL | UNBASED_UNSIZED
            | STRING_LITERAL | NULL_KW | DOLLAR => {
                self.start(LITERAL);
                self.bump();
                self.finish_node();
                false
            }
            IDENT | ESC_IDENT | SYSTEM_IDENT | THIS_KW | SUPER_KW | LOCAL_KW => {
                self.scoped_name();
                true
            }
            MACRO_REF | MACRO_CALL => {
                self.start(MACRO_EXPR);
                self.bump_name_token();
                self.finish_node();
                // `` `F(x) `` may itself expand to a name: allow calls.
                true
            }
            LPAREN => {
                self.paren_expr();
                false
            }
            LBRACE => {
                self.brace_expr();
                false
            }
            TICK if self.nth_at(1, LBRACE) => {
                self.start(ASSIGNMENT_PATTERN);
                self.assignment_pattern();
                self.finish_node();
                false
            }
            NEW_KW => {
                self.start(NEW_EXPR);
                self.bump();
                if self.at(LBRACKET) {
                    self.bump();
                    self.expr();
                    self.expect(RBRACKET);
                }
                if self.at(LPAREN) {
                    self.arg_list();
                } else if matches!(self.current(), IDENT | ESC_IDENT | THIS_KW) {
                    // A shallow copy: `new other`.
                    self.primary_expr();
                }
                self.finish_node();
                false
            }
            TAGGED_KW => {
                self.start(TAGGED_EXPR);
                self.bump();
                self.name();
                if matches!(
                    self.current(),
                    LPAREN | LBRACE | TICK | IDENT | INT_NUMBER | BASED_NUMBER
                ) {
                    self.primary_expr();
                }
                self.finish_node();
                false
            }
            // `int'(x)`, `signed'(x)`, `const'(x)`, `void'(f())`.
            k if (TYPE_KEYWORDS.contains(k) || matches!(k, SIGNED_KW | UNSIGNED_KW | CONST_KW))
                && self.nth_at(1, TICK) =>
            {
                self.start(TYPE_EXPR);
                self.bump();
                self.finish_node();
                false
            }
            // A type in an expression: `$bits(logic [7:0])`.
            k if TYPE_KEYWORDS.contains(k) || matches!(k, SIGNED_KW | UNSIGNED_KW) => {
                self.start(TYPE_EXPR);
                self.data_type();
                self.finish_node();
                false
            }
            _ => {
                self.error_expected("expression");
                false
            }
        }
    }

    /// `a`, `p::a`, `$unit::a`, `$display`, `this`, `super.x`.
    pub(crate) fn scoped_name(&mut self) {
        self.start(NAME_REF);
        self.bump();
        while self.at(COLON2) && matches!(self.nth(1), IDENT | ESC_IDENT | NEW_KW) {
            self.bump();
            self.bump();
        }
        // `c#(8)::x`, or a parameterized class type as a parameter value.
        if self.at(HASH) && self.nth_at(1, LPAREN) {
            self.param_values();
            while self.at(COLON2) && matches!(self.nth(1), IDENT | ESC_IDENT | NEW_KW) {
                self.bump();
                self.bump();
            }
        }
        self.finish_node();
    }

    fn postfix(&mut self, checkpoint: rowan::Checkpoint, mut callable: bool) {
        loop {
            match self.current() {
                LBRACKET => {
                    self.bump();
                    self.expr();
                    if matches!(self.current(), COLON | PLUS_COLON | MINUS_COLON) {
                        self.bump();
                        self.expr();
                    }
                    self.expect(RBRACKET);
                    self.start_at(checkpoint, INDEX_EXPR);
                    self.finish_node();
                    callable = false;
                }
                DOT if self.nth_is_member(1) => {
                    self.bump();
                    self.bump();
                    self.start_at(checkpoint, FIELD_EXPR);
                    self.finish_node();
                    callable = true;
                }
                LPAREN if callable => {
                    self.arg_list();
                    self.start_at(checkpoint, CALL_EXPR);
                    self.finish_node();
                    if self.at(WITH_KW) {
                        self.with_clause();
                    }
                    callable = false;
                }
                // `arr.sum with (item * 2)`, `obj.randomize with {...}`,
                // without parentheses.
                WITH_KW if callable && matches!(self.nth(1), LPAREN | LBRACE) => {
                    self.with_clause();
                    self.start_at(checkpoint, CALL_EXPR);
                    self.finish_node();
                    callable = false;
                }
                TICK if self.nth_at(1, LPAREN) => {
                    self.bump();
                    self.bump();
                    self.expr();
                    self.expect(RPAREN);
                    self.start_at(checkpoint, CAST_EXPR);
                    self.finish_node();
                    callable = false;
                }
                TICK if self.nth_at(1, LBRACE) => {
                    self.assignment_pattern();
                    self.start_at(checkpoint, ASSIGNMENT_PATTERN);
                    self.finish_node();
                    callable = false;
                }
                PLUS2 | MINUS2 => {
                    self.bump();
                    self.start_at(checkpoint, POSTFIX_EXPR);
                    self.finish_node();
                    callable = false;
                }
                _ => break,
            }
        }
    }

    /// Whether the `n`th token can name a member after `.`: an identifier,
    /// or an array method spelled like a keyword (`and`, `or`, `unique`).
    fn nth_is_member(&self, n: usize) -> bool {
        matches!(
            self.nth(n),
            IDENT | ESC_IDENT | MACRO_REF | AND_KW | OR_KW | XOR_KW | UNIQUE_KW | NEW_KW
        )
    }

    /// `with (expr)`, `with {...}` or `with (x) {...}` after a call.
    fn with_clause(&mut self) {
        self.start(WITH_CLAUSE);
        self.bump();
        if self.at(LPAREN) {
            self.bump();
            if !self.at(RPAREN) {
                self.expr();
                while self.eat(COMMA) {
                    self.expr();
                }
            }
            self.expect(RPAREN);
        }
        if self.at(LBRACE) {
            self.balanced();
        }
        self.finish_node();
    }

    /// The pattern after `matches`: up to `?`, `&&&`, `)`, `:` or `;`
    /// outside brackets.
    fn pattern(&mut self) {
        self.start(PATTERN);
        while !self.at_eof()
            && !matches!(
                self.current(),
                QUESTION | AMP3 | RPAREN | COLON | SEMI | COMMA | RBRACE
            )
        {
            if matches!(self.current(), LPAREN | LBRACKET | LBRACE) {
                self.balanced();
            } else {
                self.bump();
            }
        }
        self.finish_node();
    }

    /// `(a, .b(c), , d)`.
    pub(crate) fn arg_list(&mut self) {
        self.start(ARG_LIST);
        self.bump();
        if !self.at(RPAREN) {
            loop {
                match self.current() {
                    DOT => {
                        self.start(NAMED_ARG);
                        self.bump();
                        self.name();
                        if self.expect(LPAREN) {
                            if !self.at(RPAREN) {
                                self.expr_or_type();
                            }
                            self.expect(RPAREN);
                        }
                        self.finish_node();
                    }
                    // An empty argument.
                    COMMA | RPAREN => {}
                    _ => self.expr_or_type(),
                }
                if !self.eat(COMMA) {
                    break;
                }
            }
        }
        self.expect(RPAREN);
        self.finish_node();
    }

    /// `(e)`, `(a:b:c)`, `(a = b)`.
    fn paren_expr(&mut self) {
        let checkpoint = self.checkpoint();
        self.bump();
        let inner = self.checkpoint();
        self.expr();
        let mut kind = PAREN_EXPR;
        if self.at(COLON) {
            self.bump();
            self.expr();
            self.expect(COLON);
            self.expr();
            kind = MINTYPMAX_EXPR;
        } else if self.at(EQ) || is_compound_assign(self.current()) {
            self.bump();
            self.expr();
            self.start_at(inner, ASSIGN_EXPR);
            self.finish_node();
        }
        self.expect(RPAREN);
        self.start_at(checkpoint, kind);
        self.finish_node();
    }

    /// `{a, b}`, `{4{a}}`, `{<<8{a}}`, `{}`.
    fn brace_expr(&mut self) {
        let checkpoint = self.checkpoint();
        self.bump();
        if self.at(RBRACE) {
            self.bump();
            self.start_at(checkpoint, CONCAT_EXPR);
            self.finish_node();
            return;
        }
        if (self.at(SHL) || self.at(SHR)) && !self.nth_at(1, COMMA) {
            self.bump();
            if !self.at(LBRACE) {
                // The slice size: a number or a type.
                self.expr_or_type();
            }
            if self.at(LBRACE) {
                self.brace_expr();
            } else {
                self.error_expected("`{`");
            }
            self.expect(RBRACE);
            self.start_at(checkpoint, STREAMING_EXPR);
            self.finish_node();
            return;
        }
        self.expr();
        if self.at(LBRACE) {
            self.brace_expr();
            self.expect(RBRACE);
            self.start_at(checkpoint, REPLICATION_EXPR);
            self.finish_node();
            return;
        }
        self.stream_with();
        while self.eat(COMMA) {
            self.expr();
            self.stream_with();
        }
        self.expect(RBRACE);
        self.start_at(checkpoint, CONCAT_EXPR);
        self.finish_node();
    }

    /// `with [0 +: n]` after an array in a streaming concatenation.
    fn stream_with(&mut self) {
        if self.at(WITH_KW) && self.nth_at(1, LBRACKET) {
            self.start(WITH_CLAUSE);
            self.bump();
            self.balanced();
            self.finish_node();
        }
    }

    /// `'{a, b}`, `'{default: 0, x: 1}`, `'{4{a}}`, without its node.
    fn assignment_pattern(&mut self) {
        self.bump();
        self.bump();
        if !self.at(RBRACE) {
            let first = self.checkpoint();
            self.pattern_key();
            if self.at(LBRACE) {
                // `'{n{a, b}}`.
                self.brace_expr();
            } else {
                self.pattern_value(first);
                while self.eat(COMMA) {
                    let item = self.checkpoint();
                    self.pattern_key();
                    self.pattern_value(item);
                }
            }
        }
        self.expect(RBRACE);
    }

    /// The key or the value of a pattern item.
    fn pattern_key(&mut self) {
        if self.at(DEFAULT_KW) {
            self.bump();
        } else {
            self.expr_or_type();
        }
    }

    /// After the key: `: value` makes it a keyed item.
    fn pattern_value(&mut self, item: rowan::Checkpoint) {
        if self.eat(COLON) {
            self.expr();
            self.start_at(item, PATTERN_ITEM);
            self.finish_node();
        }
    }

    /// `{[1:3], 5, X}` after `inside`.
    fn range_list(&mut self) {
        self.start(RANGE_LIST);
        if self.expect(LBRACE) {
            loop {
                if self.at(LBRACKET) {
                    self.value_range();
                } else {
                    self.expr();
                }
                // `dist` weights: `:= 1`, `:/ 2`.
                if self.at(COLON_EQ) || self.at(COLON_SLASH) {
                    self.bump();
                    self.expr();
                }
                if !self.eat(COMMA) {
                    break;
                }
            }
            self.expect(RBRACE);
        }
        self.finish_node();
    }

    /// `[a:b]` in a range list or a `case inside` item.
    pub(crate) fn value_range(&mut self) {
        self.start(VALUE_RANGE);
        self.bump();
        self.expr();
        if self.at(COLON) || self.at(PLUS_COLON) || self.at(MINUS_COLON) {
            self.bump();
            self.expr();
        }
        self.expect(RBRACKET);
        self.finish_node();
    }

    /// An expression, or a type where one can go (parameter values,
    /// `$bits`, dimensions).
    pub(crate) fn expr_or_type(&mut self) {
        let keyword_type = (self.at_set(TYPE_KEYWORDS) && !self.nth_at(1, TICK))
            || (matches!(self.current(), SIGNED_KW | UNSIGNED_KW) && !self.nth_at(1, TICK));
        if keyword_type {
            self.start(TYPE_EXPR);
            self.data_type();
            self.finish_node();
        } else {
            self.expr();
        }
    }

    /// `lhs = rhs`, in an `ASSIGN_EXPR`.
    pub(crate) fn assign_expr(&mut self) {
        let checkpoint = self.checkpoint();
        self.expr();
        if self.expect(EQ) {
            self.expr();
        }
        self.start_at(checkpoint, ASSIGN_EXPR);
        self.finish_node();
    }

    /// An expression, an assignment or an increment: the step of a `for`.
    pub(crate) fn expr_with_assign(&mut self) {
        let checkpoint = self.checkpoint();
        self.expr();
        if self.at(EQ) || is_compound_assign(self.current()) {
            self.bump();
            self.expr();
            self.start_at(checkpoint, ASSIGN_EXPR);
            self.finish_node();
        }
    }
}

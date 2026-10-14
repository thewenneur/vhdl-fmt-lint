//! Procedural statements.

use super::{ANY_END, is_compound_assign};
use crate::SyntaxKind::{self, *};
use crate::parser::Parser;

impl Parser<'_> {
    /// A declaration or a statement, in a block or a subroutine body.
    pub(crate) fn block_item(&mut self) {
        match self.current() {
            TYPEDEF_KW => self.typedef_decl(),
            PARAMETER_KW | LOCALPARAM_KW => self.param_decl_stmt(),
            IMPORT_KW => self.package_import(),
            LET_KW => self.opaque_to_semi(OPAQUE_DECL),
            // `void'(f());`, `int'(x)` is not a declaration.
            _ if self.nth_at(1, TICK) => self.statement(),
            _ if self.at_declaration() => self.data_decl(LOCAL_DECL),
            _ => self.statement(),
        }
    }

    /// Block items up to `end`, `join` or another closing keyword.
    fn block_items(&mut self) {
        while !self.at_eof() && !self.at_set(ANY_END) {
            let progress = self.progress();
            self.block_item();
            if self.progress() == progress {
                self.bump_error();
            }
        }
    }

    /// A statement, or `;`.
    pub(crate) fn statement(&mut self) {
        self.nested(Self::statement_inner);
    }

    fn statement_inner(&mut self) {
        match self.current() {
            IDENT | ESC_IDENT if self.nth_at(1, COLON) => {
                let checkpoint = self.checkpoint();
                self.opt_label();
                let kind = self.statement_kind();
                self.start_at(checkpoint, kind);
                self.finish_node();
            }
            _ => {
                let checkpoint = self.checkpoint();
                let kind = self.statement_kind();
                self.start_at(checkpoint, kind);
                self.finish_node();
            }
        }
    }

    /// Parses a statement's tokens and returns the node kind that wraps them.
    fn statement_kind(&mut self) -> SyntaxKind {
        match self.current() {
            BEGIN_KW => {
                self.bump();
                self.end_label();
                self.block_items();
                self.expect(END_KW);
                self.end_label();
                BLOCK_STMT
            }
            FORK_KW => {
                self.bump();
                self.end_label();
                self.block_items();
                if matches!(self.current(), JOIN_KW | JOIN_ANY_KW | JOIN_NONE_KW) {
                    self.bump();
                } else {
                    self.error_expected("`join`");
                }
                self.end_label();
                FORK_STMT
            }
            UNIQUE_KW | UNIQUE0_KW | PRIORITY_KW if self.nth_at(1, IF_KW) => {
                self.bump();
                self.if_rest()
            }
            UNIQUE_KW | UNIQUE0_KW | PRIORITY_KW => {
                self.bump();
                self.case_rest()
            }
            IF_KW => self.if_rest(),
            CASE_KW | CASEX_KW | CASEZ_KW => self.case_rest(),
            FOR_KW => self.for_rest(),
            FOREACH_KW => {
                self.bump();
                if self.at(LPAREN) {
                    self.balanced();
                } else {
                    self.error_expected("`(`");
                }
                self.statement_or_null();
                FOREACH_STMT
            }
            WHILE_KW => {
                self.bump();
                self.paren_condition();
                self.statement_or_null();
                WHILE_STMT
            }
            REPEAT_KW => {
                self.bump();
                self.paren_condition();
                self.statement_or_null();
                REPEAT_STMT
            }
            FOREVER_KW => {
                self.bump();
                self.statement_or_null();
                FOREVER_STMT
            }
            DO_KW => {
                self.bump();
                self.statement_or_null();
                self.expect(WHILE_KW);
                self.paren_condition();
                self.expect(SEMI);
                DO_WHILE_STMT
            }
            AT | AT2 => {
                self.event_control();
                self.statement_or_null();
                TIMED_STMT
            }
            HASH => {
                self.delay();
                self.statement_or_null();
                TIMED_STMT
            }
            HASH2 => {
                // A cycle delay: `##2 stmt`.
                self.start(DELAY);
                self.bump();
                if self.at(LPAREN) {
                    self.balanced();
                } else {
                    self.primary_expr();
                }
                self.finish_node();
                self.statement_or_null();
                TIMED_STMT
            }
            WAIT_KW => {
                self.bump();
                if self.eat(FORK_KW) {
                    self.expect(SEMI);
                } else {
                    self.paren_condition();
                    self.statement_or_null();
                }
                WAIT_STMT
            }
            DISABLE_KW => {
                self.bump();
                if !self.eat(FORK_KW) {
                    self.expr();
                }
                self.expect(SEMI);
                DISABLE_STMT
            }
            ARROW | DARROW => {
                self.bump();
                if self.at(HASH) {
                    self.delay();
                } else if self.at(AT) {
                    self.event_control();
                }
                self.expr();
                self.expect(SEMI);
                EVENT_TRIGGER
            }
            RETURN_KW => {
                self.bump();
                if !self.at(SEMI) {
                    self.expr();
                }
                self.expect(SEMI);
                RETURN_STMT
            }
            BREAK_KW => {
                self.bump();
                self.expect(SEMI);
                BREAK_STMT
            }
            CONTINUE_KW => {
                self.bump();
                self.expect(SEMI);
                CONTINUE_STMT
            }
            SEMI => {
                self.bump();
                NULL_STMT
            }
            ASSIGN_KW | FORCE_KW => {
                self.bump();
                self.assign_expr();
                self.expect(SEMI);
                PROCEDURAL_CONTINUOUS
            }
            DEASSIGN_KW | RELEASE_KW => {
                self.bump();
                self.expr();
                self.expect(SEMI);
                PROCEDURAL_CONTINUOUS
            }
            ASSERT_KW | ASSUME_KW | COVER_KW | RESTRICT_KW | EXPECT_KW => {
                let concurrent = matches!(self.nth(1), PROPERTY_KW | SEQUENCE_KW)
                    || self.at(EXPECT_KW)
                    || self.at(RESTRICT_KW);
                self.assertion_body();
                if concurrent {
                    ASSERTION_ITEM
                } else {
                    IMMEDIATE_ASSERT
                }
            }
            RANDCASE_KW => {
                self.bump();
                while !self.at_eof() && !self.at(ENDCASE_KW) {
                    self.bump();
                }
                self.expect(ENDCASE_KW);
                OPAQUE_STMT
            }
            RANDSEQUENCE_KW => {
                self.bump();
                while !self.at_eof() && !self.at(ENDSEQUENCE_KW) {
                    self.bump();
                }
                self.expect(ENDSEQUENCE_KW);
                OPAQUE_STMT
            }
            WAIT_ORDER_KW => {
                self.bump();
                if self.at(LPAREN) {
                    self.balanced();
                }
                self.action_block();
                OPAQUE_STMT
            }
            k if ANY_END.contains(k) => {
                self.error_expected("statement");
                ERROR
            }
            _ => self.assignment_or_expr_stmt(),
        }
    }

    /// A statement, or `;` alone.
    fn statement_or_null(&mut self) {
        if self.at_set(ANY_END) {
            self.error_expected("statement");
        } else {
            self.statement();
        }
    }

    /// `(expr)` after `while`, `repeat`, `wait`.
    fn paren_condition(&mut self) {
        if self.expect(LPAREN) {
            self.expr();
            self.expect(RPAREN);
        }
    }

    /// At `if`: `if (c) s [else s]`.
    fn if_rest(&mut self) -> SyntaxKind {
        self.bump();
        if self.expect(LPAREN) {
            self.expr();
            self.expect(RPAREN);
        }
        self.statement_or_null();
        if self.at(ELSE_KW) {
            self.start(ELSE_CLAUSE);
            self.bump();
            self.statement_or_null();
            self.finish_node();
        }
        IF_STMT
    }

    /// At `case`: `case (e) [inside] items endcase`.
    fn case_rest(&mut self) -> SyntaxKind {
        if !matches!(self.current(), CASE_KW | CASEX_KW | CASEZ_KW) {
            self.error_expected("`if` or `case`");
            return ERROR;
        }
        self.bump();
        if self.expect(LPAREN) {
            self.expr();
            self.expect(RPAREN);
        }
        let matches = self.at(MATCHES_KW);
        if self.at(INSIDE_KW) || matches {
            self.bump();
        }
        while !self.at_eof() && !self.at_set(ANY_END) {
            let progress = self.progress();
            self.start(CASE_ITEM);
            if matches && !self.at(DEFAULT_KW) {
                // A pattern, kept as is up to the `:`.
                while !self.at_eof() && !self.at(COLON) && !self.at(SEMI) {
                    if matches!(self.current(), LPAREN | LBRACE | LBRACKET) {
                        self.balanced();
                    } else {
                        self.bump();
                    }
                }
                self.expect(COLON);
            } else {
                self.case_labels();
            }
            self.statement_or_null();
            self.finish_node();
            if self.progress() == progress {
                self.bump_error();
            }
        }
        self.expect(ENDCASE_KW);
        CASE_STMT
    }

    /// At `for`: `for (int i = 0, j = 0; i < n; i++, j++) s`.
    fn for_rest(&mut self) -> SyntaxKind {
        self.bump();
        if self.expect(LPAREN) {
            if !self.at(SEMI) {
                loop {
                    if self.at(VAR_KW) || (self.at_data_type() && !self.at(LBRACKET)) {
                        self.start(LOCAL_DECL);
                        self.data_decl_body_single();
                        self.finish_node();
                    } else {
                        self.expr_with_assign();
                    }
                    if !self.eat(COMMA) {
                        break;
                    }
                }
            }
            self.expect(SEMI);
            if !self.at(SEMI) {
                self.expr();
            }
            self.expect(SEMI);
            if !self.at(RPAREN) {
                loop {
                    self.expr_with_assign();
                    if !self.eat(COMMA) {
                        break;
                    }
                }
            }
            self.expect(RPAREN);
        }
        self.statement_or_null();
        FOR_STMT
    }

    /// `int i = 0` in a `for` header: one declarator, since a `,` starts
    /// the next initialization.
    fn data_decl_body_single(&mut self) {
        self.data_type();
        self.start(DECLARATOR);
        self.name();
        if self.eat(EQ) {
            self.expr();
        }
        self.finish_node();
    }

    /// `@(posedge clk or negedge rst_n)`, `@*`, `@(*)`, `@ev`.
    pub(crate) fn event_control(&mut self) {
        self.start(EVENT_CONTROL);
        self.bump();
        match self.current() {
            STAR => self.bump(),
            LPAREN => {
                self.bump();
                if self.at(STAR) && self.nth_at(1, RPAREN) {
                    self.bump();
                } else {
                    loop {
                        self.event_expr();
                        if !(self.eat(OR_KW) || self.eat(COMMA)) {
                            break;
                        }
                    }
                }
                self.expect(RPAREN);
            }
            _ => self.primary_expr(),
        }
        self.finish_node();
    }

    fn event_expr(&mut self) {
        self.start(EVENT_EXPR);
        if matches!(self.current(), POSEDGE_KW | NEGEDGE_KW | EDGE_KW) {
            self.bump();
        }
        self.expr();
        if self.eat(IFF_KW) {
            self.expr();
        }
        self.finish_node();
    }

    /// An assignment, a call, `i++`, or a macro used as a statement.
    fn assignment_or_expr_stmt(&mut self) -> SyntaxKind {
        let is_macro = matches!(self.current(), MACRO_CALL | MACRO_REF);
        self.unary_expr();
        match self.current() {
            EQ | LTE => {
                let kind = if self.at(EQ) {
                    BLOCKING_ASSIGN
                } else {
                    NONBLOCKING_ASSIGN
                };
                self.bump();
                // An intra-assignment timing control: `a = #1 b`, `a <= @(e) b`.
                match self.current() {
                    HASH => self.delay(),
                    AT | AT2 => self.event_control(),
                    REPEAT_KW => {
                        self.bump();
                        self.paren_condition();
                        self.event_control();
                    }
                    _ => {}
                }
                self.expr();
                self.expect(SEMI);
                kind
            }
            k if is_compound_assign(k) => {
                self.bump();
                self.expr();
                self.expect(SEMI);
                COMPOUND_ASSIGN
            }
            SEMI => {
                self.bump();
                if is_macro { MACRO_ITEM } else { EXPR_STMT }
            }
            // A macro that expands to a whole statement needs no `;`.
            _ if is_macro => MACRO_ITEM,
            _ => {
                self.error_expected("`;`");
                EXPR_STMT
            }
        }
    }
}

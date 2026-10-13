//! Concurrent statements: processes, blocks, generates, instantiations and
//! concurrent assignments.

use super::sequential::{Region, STATEMENT_RECOVERY};
use crate::Standard;
use crate::SyntaxKind::*;
use crate::parser::{Parser, TokenSet};

/// Tokens that end a list of concurrent statements.
const CONCURRENT_END: TokenSet = TokenSet::new(&[END_KW, ELSIF_KW, ELSE_KW, WHEN_KW]);

/// PSL directives and declarations that may appear among concurrent statements.
const PSL_WORDS: [&str; 11] = [
    "assume",
    "assume_guarantee",
    "restrict",
    "restrict_guarantee",
    "cover",
    "fairness",
    "strong",
    "property",
    "sequence",
    "default",
    "vunit",
];

/// PSL temporal operators that make an `assert` a PSL assertion.
const PSL_ASSERT_WORDS: [&str; 5] = ["always", "never", "eventually", "next", "within"];

impl Parser<'_> {
    pub(crate) fn concurrent_statements(&mut self) {
        while !self.at_set(CONCURRENT_END) && !self.at_eof() {
            let before = self.progress();
            self.concurrent_statement();
            if self.progress() == before {
                self.recover("concurrent statement", STATEMENT_RECOVERY);
                if self.progress() == before {
                    break;
                }
            }
        }
    }

    fn at_psl_statement(&self) -> bool {
        if self.at(ASSERT_KW) {
            let is_psl_word = |n: usize| {
                self.nth_at(n, IDENT)
                    && PSL_ASSERT_WORDS
                        .iter()
                        .any(|w| self.nth_text(n).eq_ignore_ascii_case(w))
            };
            return self.nth_at(1, LBRACE)
                || is_psl_word(1)
                || self.nth_at(1, LPAREN) && is_psl_word(2);
        }
        self.at(IDENT)
            && PSL_WORDS
                .iter()
                .any(|w| self.nth_text(0).eq_ignore_ascii_case(w))
            && !matches!(
                self.nth(1),
                LTE | LPAREN | DOT | TICK | SEMI | COLON_EQ | GENERIC_KW | PORT_KW | LBRACKET
            )
    }

    fn concurrent_statement(&mut self) {
        let checkpoint = self.checkpoint();
        let labeled = self.opt_label();
        if self.at_psl_statement() {
            self.psl_directive();
            self.start_at(checkpoint, PSL_DIRECTIVE);
            self.finish_node();
            return;
        }
        self.eat(POSTPONED_KW);
        let kind = match self.current() {
            PROCESS_KW => {
                self.process();
                PROCESS_STMT
            }
            BLOCK_KW => {
                self.block();
                BLOCK_STMT
            }
            FOR_KW => {
                self.for_generate();
                FOR_GENERATE_STMT
            }
            IF_KW => {
                self.if_generate();
                IF_GENERATE_STMT
            }
            CASE_KW => {
                self.case_generate();
                CASE_GENERATE_STMT
            }
            ASSERT_KW => {
                self.assertion();
                self.expect(SEMI);
                ASSERT_STMT
            }
            WITH_KW => {
                self.selected_assignment(Region::Concurrent);
                self.expect(SEMI);
                SELECTED_ASSIGN_STMT
            }
            ENTITY_KW | CONFIGURATION_KW | COMPONENT_KW => {
                self.bump();
                self.name();
                self.instantiation_maps();
                COMPONENT_INSTANTIATION_STMT
            }
            IDENT | EXT_IDENT | LPAREN | LTLT | STRING_LITERAL => {
                let kind = if self.at(LPAREN) {
                    self.paren_or_aggregate();
                    PROCEDURE_CALL_STMT
                } else {
                    self.name();
                    match self.current() {
                        GENERIC_KW | PORT_KW => COMPONENT_INSTANTIATION_STMT,
                        // `u1 : my_component;` instantiates a component without maps;
                        // `p(x);` and an unlabeled `p;` are procedure calls.
                        SEMI if labeled && self.last_kind() != RPAREN => {
                            COMPONENT_INSTANTIATION_STMT
                        }
                        _ => PROCEDURE_CALL_STMT,
                    }
                };
                if self.eat(LTE) {
                    self.signal_assignment_rhs(Region::Concurrent);
                    self.expect(SEMI);
                    SIGNAL_ASSIGN_STMT
                } else if kind == COMPONENT_INSTANTIATION_STMT {
                    self.instantiation_maps();
                    kind
                } else {
                    self.expect(SEMI);
                    kind
                }
            }
            _ => {
                if labeled {
                    self.error_expected("concurrent statement");
                }
                return;
            }
        };
        self.start_at(checkpoint, kind);
        self.finish_node();
    }

    /// `[generic map (...)] [port map (...)] ;`
    fn instantiation_maps(&mut self) {
        if self.at(GENERIC_KW) {
            self.map_aspect(GENERIC_KW);
        }
        if self.at(PORT_KW) {
            self.map_aspect(PORT_KW);
        }
        self.expect(SEMI);
    }

    fn process(&mut self) {
        self.bump();
        if self.at(LPAREN) {
            self.start(SENSITIVITY_LIST);
            self.bump();
            if self.at(ALL_KW) {
                self.require(Standard::Vhdl2008, "`process (all)`");
                self.bump();
            } else {
                self.name();
                while self.eat(COMMA) {
                    self.name();
                }
            }
            self.expect(RPAREN);
            self.finish_node();
        }
        self.eat(IS_KW);
        self.declarative_part();
        self.expect(BEGIN_KW);
        self.sequential_statements();
        self.end_clause(&[PROCESS_KW], true);
    }

    fn block(&mut self) {
        self.bump();
        if self.eat(LPAREN) {
            self.expression();
            self.expect(RPAREN);
        }
        self.eat(IS_KW);
        if self.at(GENERIC_KW) {
            self.generic_clause();
            if self.at(GENERIC_KW) {
                self.map_aspect(GENERIC_KW);
                self.expect(SEMI);
            }
        }
        if self.at(PORT_KW) {
            self.port_clause();
            if self.at(PORT_KW) {
                self.map_aspect(PORT_KW);
                self.expect(SEMI);
            }
        }
        self.declarative_part();
        self.expect(BEGIN_KW);
        self.concurrent_statements();
        self.end_clause(&[BLOCK_KW], true);
    }

    fn for_generate(&mut self) {
        self.bump();
        self.ident();
        self.expect(IN_KW);
        self.discrete_range();
        self.expect(GENERATE_KW);
        self.generate_body();
        self.end_clause(&[GENERATE_KW], true);
    }

    fn if_generate(&mut self) {
        self.bump();
        self.opt_label();
        self.expression();
        self.expect(GENERATE_KW);
        self.generate_body();
        loop {
            match self.current() {
                ELSIF_KW => {
                    self.require(Standard::Vhdl2008, "`elsif` in if-generate statements");
                    self.bump();
                    self.opt_label();
                    self.expression();
                }
                ELSE_KW => {
                    self.require(Standard::Vhdl2008, "`else` in if-generate statements");
                    self.bump();
                    self.opt_label();
                }
                _ => break,
            }
            self.expect(GENERATE_KW);
            self.generate_body();
        }
        self.end_clause(&[GENERATE_KW], true);
    }

    fn case_generate(&mut self) {
        self.require(Standard::Vhdl2008, "case-generate statements");
        self.bump();
        self.expression();
        self.expect(GENERATE_KW);
        while self.at(WHEN_KW) {
            self.start(CASE_ALTERNATIVE);
            self.bump();
            self.opt_label();
            self.choices();
            self.expect(ARROW);
            self.generate_body();
            self.finish_node();
        }
        self.end_clause(&[GENERATE_KW], true);
    }

    /// `[declarations begin] statements [end [alternative_label] ;]`
    fn generate_body(&mut self) {
        self.start(GENERATE_BODY);
        if self.at_declaration() || self.at(BEGIN_KW) {
            self.declarative_part();
            self.expect(BEGIN_KW);
        }
        self.concurrent_statements();
        if self.at(END_KW) && matches!(self.nth(1), IDENT | EXT_IDENT | SEMI) {
            // VHDL-2008 lets each alternative have its own `end`.
            self.bump();
            if self.at(IDENT) || self.at(EXT_IDENT) {
                self.bump();
            }
            self.expect(SEMI);
        }
        self.finish_node();
    }
}

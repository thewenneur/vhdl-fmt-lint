//! Sequential statements, and the assignment forms shared with concurrent code.

use crate::Standard;
use crate::SyntaxKind::{self, *};
use crate::parser::{Parser, TokenSet};

/// Tokens that end a list of sequential statements.
const SEQUENTIAL_END: TokenSet = TokenSet::new(&[END_KW, ELSE_KW, ELSIF_KW, WHEN_KW]);

/// Where to resume after a statement that makes no sense.
pub(crate) const STATEMENT_RECOVERY: TokenSet = TokenSet::new(&[
    END_KW, BEGIN_KW, ELSIF_KW, PROCESS_KW, IF_KW, CASE_KW, WHILE_KW, WAIT_KW, ASSERT_KW,
    REPORT_KW, RETURN_KW, EXIT_KW, NEXT_KW, WITH_KW,
]);

/// Which region an assignment appears in; it decides which forms are VHDL-93.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Region {
    Concurrent,
    Sequential,
}

impl Parser<'_> {
    pub(crate) fn sequential_statements(&mut self) {
        while !self.at_set(SEQUENTIAL_END) && !self.at_eof() {
            let before = self.progress();
            self.sequential_statement();
            if self.progress() == before {
                self.recover("statement", STATEMENT_RECOVERY);
                if self.progress() == before {
                    break;
                }
            }
        }
    }

    fn sequential_statement(&mut self) {
        let checkpoint = self.checkpoint();
        let labeled = self.opt_label();
        let kind = match self.current() {
            WAIT_KW => self.wait_statement(),
            ASSERT_KW => self.assertion(),
            REPORT_KW => {
                self.bump();
                self.expression();
                if self.eat(SEVERITY_KW) {
                    self.expression();
                }
                REPORT_STMT
            }
            IF_KW => self.if_statement(),
            CASE_KW => self.case_statement(),
            WHILE_KW | FOR_KW | LOOP_KW => self.loop_statement(),
            NEXT_KW | EXIT_KW => {
                let kind = if self.at(NEXT_KW) {
                    NEXT_STMT
                } else {
                    EXIT_STMT
                };
                self.bump();
                if self.at(IDENT) || self.at(EXT_IDENT) {
                    self.bump();
                }
                if self.eat(WHEN_KW) {
                    self.expression();
                }
                kind
            }
            RETURN_KW => {
                self.bump();
                if !self.at(SEMI) {
                    self.conditional_expression(Standard::Vhdl2019);
                }
                RETURN_STMT
            }
            NULL_KW => {
                self.bump();
                NULL_STMT
            }
            BLOCK_KW => {
                self.require(Standard::Vhdl2019, "sequential block statements");
                self.bump();
                self.eat(IS_KW);
                self.declarative_part();
                self.expect(BEGIN_KW);
                self.sequential_statements();
                self.end_clause(&[BLOCK_KW], true);
                self.start_at(checkpoint, SEQUENTIAL_BLOCK_STMT);
                self.finish_node();
                return;
            }
            WITH_KW => {
                self.require(Standard::Vhdl2008, "sequential selected assignments");
                self.selected_assignment(Region::Sequential);
                SELECTED_ASSIGN_STMT
            }
            IDENT | EXT_IDENT | LPAREN | LTLT | STRING_LITERAL => {
                self.assignment_or_call(Region::Sequential)
            }
            _ => {
                // Leave the tokens to the caller's recovery.
                if labeled {
                    self.error_expected("statement");
                }
                return;
            }
        };
        // Compound statements consume their own `;` through `end_clause`.
        if !matches!(kind, IF_STMT | CASE_STMT | LOOP_STMT) {
            self.expect(SEMI);
        }
        self.start_at(checkpoint, kind);
        self.finish_node();
    }

    fn wait_statement(&mut self) -> SyntaxKind {
        self.bump();
        if self.eat(ON_KW) {
            self.start(SENSITIVITY_LIST);
            self.name();
            while self.eat(COMMA) {
                self.name();
            }
            self.finish_node();
        }
        if self.eat(UNTIL_KW) {
            self.expression();
        }
        if self.eat(FOR_KW) {
            self.expression();
        }
        WAIT_STMT
    }

    /// `assert cond [report expr] [severity expr]`, without the `;`.
    pub(crate) fn assertion(&mut self) -> SyntaxKind {
        self.bump();
        self.expression();
        if self.eat(REPORT_KW) {
            self.expression();
        }
        if self.eat(SEVERITY_KW) {
            self.expression();
        }
        ASSERT_STMT
    }

    fn if_statement(&mut self) -> SyntaxKind {
        self.bump();
        self.expression();
        self.expect(THEN_KW);
        self.sequential_statements();
        loop {
            match self.current() {
                ELSIF_KW => {
                    self.bump();
                    self.expression();
                    self.expect(THEN_KW);
                    self.sequential_statements();
                }
                ELSE_KW => {
                    self.bump();
                    self.sequential_statements();
                }
                _ => break,
            }
        }
        self.end_clause(&[IF_KW], true);
        IF_STMT
    }

    fn case_statement(&mut self) -> SyntaxKind {
        self.bump();
        if self.eat(QUESTION) {
            self.require(Standard::Vhdl2008, "matching case statements");
        }
        self.expression();
        self.expect(IS_KW);
        while self.at(WHEN_KW) {
            self.start(CASE_ALTERNATIVE);
            self.bump();
            self.choices();
            self.expect(ARROW);
            self.sequential_statements();
            self.finish_node();
            // A stray `else` or `elsif` would otherwise end the case statement early.
            while self.at(ELSE_KW) || self.at(ELSIF_KW) {
                self.recover("`when` or `end case`", TokenSet::new(&[WHEN_KW, END_KW]));
                self.sequential_statements();
            }
        }
        self.end_clause(&[CASE_KW], true);
        CASE_STMT
    }

    fn loop_statement(&mut self) -> SyntaxKind {
        match self.current() {
            WHILE_KW => {
                self.start(WHILE_SCHEME);
                self.bump();
                self.expression();
                self.finish_node();
            }
            FOR_KW => {
                self.start(FOR_SCHEME);
                self.bump();
                self.ident();
                self.expect(IN_KW);
                self.discrete_range();
                self.finish_node();
            }
            _ => {}
        }
        self.expect(LOOP_KW);
        self.sequential_statements();
        self.end_clause(&[LOOP_KW], true);
        LOOP_STMT
    }

    /// A statement that starts with a name or an aggregate: an assignment or
    /// a procedure call. Returns its kind; the caller wraps it.
    pub(crate) fn assignment_or_call(&mut self, region: Region) -> SyntaxKind {
        if self.at(LPAREN) {
            self.paren_or_aggregate();
        } else {
            self.name();
        }
        match self.current() {
            LTE => {
                self.bump();
                self.signal_assignment_rhs(region);
                SIGNAL_ASSIGN_STMT
            }
            COLON_EQ => {
                self.bump();
                self.conditional_expression(Standard::Vhdl2008);
                VARIABLE_ASSIGN_STMT
            }
            _ => PROCEDURE_CALL_STMT,
        }
    }

    /// Everything after `<=` in a signal assignment.
    pub(crate) fn signal_assignment_rhs(&mut self, region: Region) {
        if region == Region::Concurrent {
            self.eat(GUARDED_KW);
        }
        if self.at(FORCE_KW) || self.at(RELEASE_KW) {
            let force = self.at(FORCE_KW);
            self.bump();
            if self.at(IN_KW) || self.at(OUT_KW) {
                self.bump();
            }
            if force {
                self.conditional_expression(Standard::Vhdl2008);
            }
            return;
        }
        self.delay_mechanism();
        let checkpoint = self.checkpoint();
        self.waveform();
        if self.at(WHEN_KW) {
            self.start_at(checkpoint, CONDITIONAL_WAVEFORMS);
            if region == Region::Sequential {
                self.require(
                    Standard::Vhdl2008,
                    "sequential conditional signal assignments",
                );
            }
            while self.eat(WHEN_KW) {
                self.expression();
                if !self.eat(ELSE_KW) {
                    break;
                }
                self.waveform();
            }
            self.finish_node();
        }
    }

    fn delay_mechanism(&mut self) {
        if self.at(TRANSPORT_KW) || self.at(REJECT_KW) || self.at(INERTIAL_KW) {
            self.start(DELAY_MECHANISM);
            if !self.eat(TRANSPORT_KW) {
                if self.eat(REJECT_KW) {
                    self.expression();
                }
                self.expect(INERTIAL_KW);
            }
            self.finish_node();
        }
    }

    /// `elem [after t] {, elem [after t]}` or `unaffected`.
    fn waveform(&mut self) {
        self.start(WAVEFORM);
        if !self.eat(UNAFFECTED_KW) {
            loop {
                self.start(WAVEFORM_ELEMENT);
                self.expression();
                if self.eat(AFTER_KW) {
                    self.expression();
                }
                self.finish_node();
                if !self.eat(COMMA) {
                    break;
                }
            }
        }
        self.finish_node();
    }

    /// `with expr select[?] target <= ... when choices, ...`, without the `;`.
    pub(crate) fn selected_assignment(&mut self, region: Region) {
        self.bump();
        self.expression();
        self.expect(SELECT_KW);
        if self.eat(QUESTION) {
            self.require(Standard::Vhdl2008, "matching selected assignments");
        }
        if self.at(LPAREN) {
            self.paren_or_aggregate();
        } else {
            self.name();
        }
        let is_variable = self.at(COLON_EQ);
        if is_variable || self.at(LTE) {
            self.bump();
        } else {
            self.error_expected("`<=` or `:=`");
        }
        if !is_variable {
            if region == Region::Concurrent {
                self.eat(GUARDED_KW);
            }
            if self.eat(FORCE_KW) && (self.at(IN_KW) || self.at(OUT_KW)) {
                self.bump();
            }
            self.delay_mechanism();
        }
        self.start(SELECTED_WAVEFORMS);
        loop {
            if is_variable {
                self.expression();
            } else {
                self.waveform();
            }
            self.expect(WHEN_KW);
            self.choices();
            if !self.eat(COMMA) {
                break;
            }
        }
        self.finish_node();
    }
}

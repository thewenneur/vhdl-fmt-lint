//! Expressions and names.

use crate::Standard;
use crate::SyntaxKind::*;
use crate::parser::{Parser, TokenSet};

const LOGICAL_OPS: TokenSet = TokenSet::new(&[AND_KW, OR_KW, NAND_KW, NOR_KW, XOR_KW, XNOR_KW]);
const RELATIONAL_OPS: TokenSet =
    TokenSet::new(&[EQ, NEQ, LT, LTE, GT, GTE, QEQ, QNEQ, QLT, QLTE, QGT, QGTE]);
const MATCHING_OPS: TokenSet = TokenSet::new(&[QEQ, QNEQ, QLT, QLTE, QGT, QGTE]);
const SHIFT_OPS: TokenSet = TokenSet::new(&[SLL_KW, SRL_KW, SLA_KW, SRA_KW, ROL_KW, ROR_KW]);
const ADDING_OPS: TokenSet = TokenSet::new(&[PLUS, MINUS, AMP]);
const MULTIPLYING_OPS: TokenSet = TokenSet::new(&[STAR, SLASH, MOD_KW, REM_KW]);

impl Parser<'_> {
    pub(crate) fn expression(&mut self) {
        self.nested(|p| {
            if p.at(QQ) {
                p.require(Standard::Vhdl2008, "the condition operator `??`");
                p.start(UNARY_EXPR);
                p.bump();
                p.primary();
                p.finish_node();
            } else {
                p.logical_expression();
            }
        });
    }

    /// `expr [when cond else expr ...]`. `since` is the standard that allows
    /// the conditional form in this position.
    pub(crate) fn conditional_expression(&mut self, since: Standard) {
        let checkpoint = self.checkpoint();
        self.expression();
        if self.at(WHEN_KW) {
            self.start_at(checkpoint, CONDITIONAL_EXPR);
            self.require(since, "conditional expressions here");
            while self.eat(WHEN_KW) {
                self.expression();
                if !self.eat(ELSE_KW) {
                    break;
                }
                self.expression();
            }
            self.finish_node();
        }
    }

    fn logical_expression(&mut self) {
        let checkpoint = self.checkpoint();
        self.relation();
        while self.at_set(LOGICAL_OPS) {
            self.start_at(checkpoint, BINARY_EXPR);
            self.bump();
            self.relation();
            self.finish_node();
        }
    }

    fn relation(&mut self) {
        let checkpoint = self.checkpoint();
        self.shift_expression();
        if self.at_set(RELATIONAL_OPS) {
            if self.at_set(MATCHING_OPS) {
                self.require(Standard::Vhdl2008, "matching relational operators");
            }
            self.start_at(checkpoint, BINARY_EXPR);
            self.bump();
            self.shift_expression();
            self.finish_node();
        }
    }

    fn shift_expression(&mut self) {
        let checkpoint = self.checkpoint();
        self.simple_expression();
        if self.at_set(SHIFT_OPS) {
            self.start_at(checkpoint, BINARY_EXPR);
            self.bump();
            self.simple_expression();
            self.finish_node();
        }
    }

    fn simple_expression(&mut self) {
        let checkpoint = self.checkpoint();
        if self.at(PLUS) || self.at(MINUS) {
            self.start(UNARY_EXPR);
            self.bump();
            self.term();
            self.finish_node();
        } else {
            self.term();
        }
        while self.at_set(ADDING_OPS) {
            self.start_at(checkpoint, BINARY_EXPR);
            self.bump();
            self.term();
            self.finish_node();
        }
    }

    fn term(&mut self) {
        let checkpoint = self.checkpoint();
        self.factor();
        while self.at_set(MULTIPLYING_OPS) {
            self.start_at(checkpoint, BINARY_EXPR);
            self.bump();
            self.factor();
            self.finish_node();
        }
    }

    fn factor(&mut self) {
        match self.current() {
            ABS_KW | NOT_KW => {
                self.start(UNARY_EXPR);
                self.bump();
                self.primary();
                self.finish_node();
            }
            kind if LOGICAL_OPS.contains(kind) => {
                self.require(Standard::Vhdl2008, "unary logical operators");
                self.start(UNARY_EXPR);
                self.bump();
                self.primary();
                self.finish_node();
            }
            _ => {
                let checkpoint = self.checkpoint();
                self.primary();
                if self.at(STAR2) {
                    self.start_at(checkpoint, BINARY_EXPR);
                    self.bump();
                    self.primary();
                    self.finish_node();
                }
            }
        }
    }

    fn primary(&mut self) {
        match self.current() {
            DECIMAL_LITERAL | BASED_LITERAL => {
                let checkpoint = self.checkpoint();
                self.start(LITERAL);
                self.bump();
                self.finish_node();
                // `10 ns`: a physical literal.
                if self.at(IDENT) || self.at(EXT_IDENT) {
                    self.start_at(checkpoint, PHYSICAL_LITERAL);
                    self.simple_name();
                    self.finish_node();
                }
            }
            // `"and"(a, b)`: an operator symbol called like a function.
            STRING_LITERAL if matches!(self.nth(1), LPAREN | DOT) => self.name(),
            CHAR_LITERAL | STRING_LITERAL | BIT_STRING_LITERAL | NULL_KW => {
                self.start(LITERAL);
                self.bump();
                self.finish_node();
            }
            IDENT | EXT_IDENT | LTLT => self.name(),
            LPAREN => self.nested(|p| p.paren_or_aggregate()),
            NEW_KW => {
                self.start(ALLOCATOR);
                self.bump();
                self.subtype_indication();
                self.finish_node();
            }
            _ => self.error_expected("expression"),
        }
    }

    /// `(expr)`, or an aggregate: `(a, b)`, `(others => '0')`, `(0 to 3 => x)`.
    pub(crate) fn paren_or_aggregate(&mut self) {
        let checkpoint = self.checkpoint();
        self.bump();
        let element = self.checkpoint();
        let simple = self.choice();
        if self.at(BAR) || self.at(ARROW) {
            self.start_at(element, CHOICES);
            while self.eat(BAR) {
                self.choice();
            }
            self.finish_node();
            self.start_at(element, ELEMENT_ASSOCIATION);
            self.expect(ARROW);
            self.expression();
            self.finish_node();
        } else if simple && self.at(RPAREN) {
            self.bump();
            self.start_at(checkpoint, PAREN_EXPR);
            self.finish_node();
            return;
        } else {
            self.start_at(element, ELEMENT_ASSOCIATION);
            self.finish_node();
        }
        while self.eat(COMMA) {
            self.element_association();
        }
        self.expect(RPAREN);
        self.start_at(checkpoint, AGGREGATE);
        self.finish_node();
    }

    fn element_association(&mut self) {
        self.start(ELEMENT_ASSOCIATION);
        let checkpoint = self.checkpoint();
        self.choice();
        if self.at(BAR) || self.at(ARROW) {
            self.start_at(checkpoint, CHOICES);
            while self.eat(BAR) {
                self.choice();
            }
            self.finish_node();
            self.expect(ARROW);
            self.expression();
        }
        self.finish_node();
    }

    /// One choice: `others`, an expression or a discrete range. Returns
    /// whether it was a plain expression.
    fn choice(&mut self) -> bool {
        if self.eat(OTHERS_KW) {
            return false;
        }
        let checkpoint = self.checkpoint();
        self.expression();
        match self.current() {
            TO_KW | DOWNTO_KW => {
                self.start_at(checkpoint, RANGE_EXPR);
                self.bump();
                self.expression();
                self.finish_node();
                false
            }
            RANGE_KW => {
                self.start_at(checkpoint, SUBTYPE_INDICATION);
                self.start(RANGE_CONSTRAINT);
                self.bump();
                self.range();
                self.finish_node();
                self.finish_node();
                false
            }
            _ => true,
        }
    }

    /// `choice { | choice }`, as in case alternatives and selected assignments.
    pub(crate) fn choices(&mut self) {
        self.start(CHOICES);
        self.choice();
        while self.eat(BAR) {
            self.choice();
        }
        self.finish_node();
    }

    fn simple_name(&mut self) {
        self.start(NAME);
        self.bump();
        self.finish_node();
    }

    /// A name with all its suffixes: `a.b(c)'d`.
    pub(crate) fn name(&mut self) {
        let checkpoint = self.checkpoint();
        match self.current() {
            IDENT | EXT_IDENT | STRING_LITERAL | CHAR_LITERAL => self.simple_name(),
            LTLT => self.external_name(),
            _ => {
                self.error_expected("name");
                return;
            }
        }
        loop {
            match self.current() {
                DOT => {
                    self.start_at(checkpoint, SELECTED_NAME);
                    self.bump();
                    match self.current() {
                        IDENT | EXT_IDENT | CHAR_LITERAL | STRING_LITERAL | ALL_KW => self.bump(),
                        _ => self.error_expected("name after `.`"),
                    }
                    self.finish_node();
                }
                LPAREN => {
                    self.start_at(checkpoint, CALL_EXPR);
                    self.nested(|p| p.association_list());
                    self.finish_node();
                }
                LBRACKET => self.signature(),
                TICK if self.nth_at(1, LPAREN) => {
                    self.start_at(checkpoint, QUALIFIED_EXPR);
                    self.bump();
                    self.nested(|p| p.paren_or_aggregate());
                    self.finish_node();
                }
                TICK => {
                    self.start_at(checkpoint, ATTRIBUTE_NAME);
                    self.bump();
                    match self.current() {
                        IDENT | EXT_IDENT | RANGE_KW | SUBTYPE_KW => self.bump(),
                        _ => self.error_expected("attribute name"),
                    }
                    self.finish_node();
                }
                _ => break,
            }
        }
    }

    /// `[t1, t2 return t3]`.
    pub(crate) fn signature(&mut self) {
        self.start(SIGNATURE);
        self.bump();
        if !self.at(RBRACKET) && !self.at(RETURN_KW) {
            self.type_mark();
            while self.eat(COMMA) {
                self.type_mark();
            }
        }
        if self.eat(RETURN_KW) {
            self.type_mark();
        }
        self.expect(RBRACKET);
        self.finish_node();
    }

    /// `<< signal .top.dut.x : std_logic >>` (VHDL-2008).
    fn external_name(&mut self) {
        self.start(EXTERNAL_NAME);
        self.require(Standard::Vhdl2008, "external names");
        self.bump();
        if matches!(self.current(), CONSTANT_KW | SIGNAL_KW | VARIABLE_KW) {
            self.bump();
        } else {
            self.error_expected("`constant`, `signal` or `variable`");
        }
        self.start(EXTERNAL_PATH);
        self.skip_balanced_until(TokenSet::new(&[COLON, GTGT, SEMI]));
        self.finish_node();
        self.expect(COLON);
        self.subtype_indication();
        self.expect(GTGT);
        self.finish_node();
    }
}

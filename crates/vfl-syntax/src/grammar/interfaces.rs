//! Generic and port clauses, interface lists, and map aspects.

use crate::Standard;
use crate::SyntaxKind::{self, *};
use crate::parser::{Parser, TokenSet};

const MODES: TokenSet = TokenSet::new(&[IN_KW, OUT_KW, INOUT_KW, BUFFER_KW, LINKAGE_KW]);

/// Where to resume inside an interface list.
const INTERFACE_RECOVERY: TokenSet =
    TokenSet::new(&[RPAREN, SEMI, BEGIN_KW, END_KW, IS_KW, PORT_KW, GENERIC_KW]);

impl Parser<'_> {
    pub(crate) fn generic_clause(&mut self) {
        self.start(GENERIC_CLAUSE);
        self.bump();
        self.interface_list();
        self.expect(SEMI);
        self.finish_node();
    }

    pub(crate) fn port_clause(&mut self) {
        self.start(PORT_CLAUSE);
        self.bump();
        self.interface_list();
        self.expect(SEMI);
        self.finish_node();
    }

    /// `( element { ; element } )`.
    pub(crate) fn interface_list(&mut self) {
        self.start(INTERFACE_LIST);
        if self.expect(LPAREN) {
            loop {
                self.interface_element();
                if self.eat(SEMI) {
                    if self.at(RPAREN) {
                        // A trailing `;` before `)` is new in VHDL-2019.
                        self.require(Standard::Vhdl2019, "a trailing `;` in an interface list");
                        break;
                    }
                    continue;
                }
                if self.at(RPAREN) || self.at_set(INTERFACE_RECOVERY) || self.at_eof() {
                    break;
                }
                // Skips through the next `;`, or up to a token that ends the list.
                self.recover("`;` or `)`", INTERFACE_RECOVERY);
                if self.at_set(INTERFACE_RECOVERY) || self.at_eof() {
                    break;
                }
            }
            self.expect(RPAREN);
        }
        self.finish_node();
    }

    fn interface_element(&mut self) {
        match self.current() {
            TYPE_KW => {
                self.start(INTERFACE_TYPE_DECL);
                self.require(Standard::Vhdl2008, "generic types");
                self.bump();
                self.ident();
                if self.eat(IS_KW) {
                    // VHDL-2019 type classes: `is private`, `is <>`, `is (<>)`,
                    // `is range <>`, `is array (...) of ...`, ...
                    self.require(Standard::Vhdl2019, "generic type classes");
                    self.skip_balanced_until(TokenSet::new(&[SEMI, RPAREN]));
                }
                self.finish_node();
            }
            FUNCTION_KW | PROCEDURE_KW | PURE_KW | IMPURE_KW => {
                self.start(INTERFACE_SUBPROGRAM_DECL);
                self.require(Standard::Vhdl2008, "generic subprograms");
                self.subprogram_spec();
                if self.eat(IS_KW) && !self.eat(BOX) {
                    self.name();
                }
                self.finish_node();
            }
            PACKAGE_KW => {
                self.start(INTERFACE_PACKAGE_DECL);
                self.require(Standard::Vhdl2008, "generic packages");
                self.bump();
                self.ident();
                self.expect(IS_KW);
                self.expect(NEW_KW);
                self.name();
                if self.at(GENERIC_KW) {
                    self.map_aspect(GENERIC_KW);
                } else {
                    self.error_expected("`generic map`");
                }
                self.finish_node();
            }
            IDENT | EXT_IDENT | CONSTANT_KW | SIGNAL_KW | VARIABLE_KW | FILE_KW => {
                self.start(INTERFACE_OBJECT_DECL);
                if matches!(
                    self.current(),
                    CONSTANT_KW | SIGNAL_KW | VARIABLE_KW | FILE_KW
                ) {
                    self.bump();
                }
                self.ident_list();
                self.expect(COLON);
                self.mode_or_view();
                if !self.at(SEMI) && !self.at(RPAREN) && !self.at(COLON_EQ) {
                    self.subtype_indication();
                }
                self.eat(BUS_KW);
                if self.eat(COLON_EQ) {
                    self.expression();
                }
                self.finish_node();
            }
            _ => self.error_expected("interface declaration"),
        }
    }

    /// An optional mode, or a VHDL-2019 mode view: `view v [of t]`, `view (v) of t`.
    pub(crate) fn mode_or_view(&mut self) {
        if self.at_set(MODES) {
            self.bump();
        } else if self.at(VIEW_KW) {
            self.start(MODE_VIEW_INDICATION);
            self.bump();
            if self.at(LPAREN) {
                self.bump();
                self.name();
                self.expect(RPAREN);
            } else {
                self.name();
            }
            if self.eat(OF_KW) {
                self.subtype_indication();
            }
            self.finish_node();
        }
    }

    /// Skips tokens until one in `stop` at paren depth zero.
    pub(crate) fn skip_balanced_until(&mut self, stop: TokenSet) {
        let mut depth = 0u32;
        while !self.at_eof() {
            match self.current() {
                LPAREN => depth += 1,
                RPAREN if depth == 0 => break,
                RPAREN => depth -= 1,
                kind if depth == 0 && stop.contains(kind) => break,
                _ => {}
            }
            self.bump();
        }
    }

    /// `generic map (...)`, `port map (...)` or `parameter map (...)`.
    pub(crate) fn map_aspect(&mut self, keyword: SyntaxKind) {
        let kind = match keyword {
            GENERIC_KW => GENERIC_MAP_ASPECT,
            PORT_KW => PORT_MAP_ASPECT,
            _ => PARAMETER_MAP_ASPECT,
        };
        self.start(kind);
        self.bump();
        self.expect(MAP_KW);
        if self.at(LPAREN) {
            self.association_list();
        } else {
            self.error_expected("`(`");
        }
        self.finish_node();
    }

    /// `( [formal =>] actual { , [formal =>] actual } )`.
    pub(crate) fn association_list(&mut self) {
        self.start(ASSOCIATION_LIST);
        self.bump();
        if !self.at(RPAREN) {
            loop {
                self.association_element();
                if !self.eat(COMMA) {
                    break;
                }
            }
        }
        self.expect(RPAREN);
        self.finish_node();
    }

    fn association_element(&mut self) {
        self.start(ASSOCIATION_ELEMENT);
        self.actual_part();
        if self.eat(ARROW) {
            self.actual_part();
        }
        self.finish_node();
    }

    /// An actual (or a formal, which has the same shape): an expression, a
    /// range, a subtype indication, `open`, `<>` or `inertial expr`.
    fn actual_part(&mut self) {
        match self.current() {
            OPEN_KW | BOX => self.bump(),
            INERTIAL_KW => {
                self.require(Standard::Vhdl2008, "`inertial` in port maps");
                self.bump();
                self.expression();
            }
            _ => self.discrete_range(),
        }
    }
}

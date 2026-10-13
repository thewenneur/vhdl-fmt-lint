//! Type declarations, subtype indications and ranges.

use crate::Standard;
use crate::SyntaxKind::*;
use crate::parser::{Parser, TokenSet};

impl Parser<'_> {
    pub(crate) fn type_decl(&mut self) {
        self.start(TYPE_DECL);
        self.bump();
        self.ident();
        // `type t;` is an incomplete type declaration.
        if self.eat(IS_KW) {
            self.type_definition();
        }
        self.expect(SEMI);
        self.finish_node();
    }

    fn type_definition(&mut self) {
        match self.current() {
            LPAREN => {
                self.start(ENUM_TYPE_DEF);
                self.bump();
                loop {
                    if matches!(self.current(), IDENT | EXT_IDENT | CHAR_LITERAL) {
                        self.bump();
                    } else {
                        self.error_expected("enumeration literal");
                        break;
                    }
                    if !self.eat(COMMA) {
                        break;
                    }
                }
                self.expect(RPAREN);
                self.finish_node();
            }
            RANGE_KW => {
                let checkpoint = self.checkpoint();
                self.bump();
                self.range();
                if self.at(UNITS_KW) {
                    self.start_at(checkpoint, PHYSICAL_TYPE_DEF);
                    self.bump();
                    while !self.at(END_KW) && !self.at_eof() {
                        if !(self.at(IDENT) || self.at(EXT_IDENT)) {
                            self.recover("unit declaration", TokenSet::new(&[END_KW]));
                            continue;
                        }
                        self.start(UNIT_DECL);
                        self.bump();
                        if self.eat(EQ) {
                            self.expression();
                        }
                        self.expect(SEMI);
                        self.finish_node();
                    }
                    self.end_without_semi(UNITS_KW);
                    self.finish_node();
                    return;
                }
                self.start_at(checkpoint, RANGE_TYPE_DEF);
                self.finish_node();
            }
            ARRAY_KW => {
                self.start(ARRAY_TYPE_DEF);
                self.bump();
                self.expect(LPAREN);
                loop {
                    if self.nth_at(1, RANGE_KW) && self.nth_at(2, BOX) {
                        self.start(INDEX_SUBTYPE_DEF);
                        self.type_mark();
                        self.bump();
                        self.bump();
                        self.finish_node();
                    } else {
                        self.discrete_range();
                    }
                    if !self.eat(COMMA) {
                        break;
                    }
                }
                self.expect(RPAREN);
                self.expect(OF_KW);
                self.subtype_indication();
                self.finish_node();
            }
            RECORD_KW => {
                self.start(RECORD_TYPE_DEF);
                self.bump();
                while !self.at(END_KW) && !self.at_eof() {
                    if !(self.at(IDENT) || self.at(EXT_IDENT)) {
                        self.recover("element declaration", TokenSet::new(&[END_KW]));
                        continue;
                    }
                    self.start(ELEMENT_DECL);
                    self.ident_list();
                    self.expect(COLON);
                    self.subtype_indication();
                    self.expect(SEMI);
                    self.finish_node();
                }
                self.end_without_semi(RECORD_KW);
                self.finish_node();
            }
            ACCESS_KW => {
                self.start(ACCESS_TYPE_DEF);
                self.bump();
                self.subtype_indication();
                self.finish_node();
            }
            FILE_KW => {
                self.start(FILE_TYPE_DEF);
                self.bump();
                self.expect(OF_KW);
                self.type_mark();
                self.finish_node();
            }
            PROTECTED_KW => {
                let is_body = self.nth_at(1, BODY_KW);
                self.start(if is_body {
                    PROTECTED_TYPE_BODY
                } else {
                    PROTECTED_TYPE_DEF
                });
                self.bump();
                if is_body {
                    self.bump();
                }
                if !is_body && self.at(GENERIC_KW) {
                    self.require(Standard::Vhdl2019, "protected type generics");
                    self.generic_clause();
                    if self.at(GENERIC_KW) {
                        self.map_aspect(GENERIC_KW);
                        self.expect(SEMI);
                    }
                }
                self.declarative_part();
                self.protected_end(is_body);
                self.finish_node();
            }
            PRIVATE_KW => {
                self.start(PRIVATE_TYPE_DEF);
                self.bump();
                self.finish_node();
            }
            _ => self.error_expected("type definition"),
        }
    }

    /// `end <keyword> [name]`, without the `;`, which belongs to the type declaration.
    fn end_without_semi(&mut self, keyword: crate::SyntaxKind) {
        if self.at(END_KW) && self.nth_at(1, keyword) {
            self.bump();
            self.bump();
            if self.at(IDENT) || self.at(EXT_IDENT) {
                self.bump();
            }
        } else {
            self.error_expected(&format!("`end {}`", keyword.text().unwrap_or_default()));
        }
    }

    /// `end protected [body] [name]`, without the `;`.
    fn protected_end(&mut self, is_body: bool) {
        if self.at(END_KW) && self.nth_at(1, PROTECTED_KW) {
            self.bump();
            self.bump();
            if is_body {
                self.expect(BODY_KW);
            }
            if self.at(IDENT) || self.at(EXT_IDENT) {
                self.bump();
            }
        } else {
            self.error_expected("`end protected`");
        }
    }

    /// `[resolution] type_mark [constraint]`.
    ///
    /// An index or record constraint `t(...)` parses as part of the type
    /// mark's name.
    pub(crate) fn subtype_indication(&mut self) {
        self.start(SUBTYPE_INDICATION);
        if self.at(LPAREN) {
            self.element_resolution();
        }
        let checkpoint = self.checkpoint();
        self.type_mark();
        // `resolved std_ulogic`: the first name was a resolution function.
        if self.at(IDENT) || self.at(EXT_IDENT) {
            self.start_at(checkpoint, RESOLUTION_INDICATION);
            self.finish_node();
            self.type_mark();
        }
        if self.at(RANGE_KW) {
            self.range_constraint();
        }
        self.finish_node();
    }

    /// `(resolved)`, `((resolved))`, `(a resolved, b fn)`: VHDL-2008 element resolution.
    fn element_resolution(&mut self) {
        self.start(RESOLUTION_INDICATION);
        self.require(Standard::Vhdl2008, "element resolution");
        self.bump();
        self.skip_balanced_until(TokenSet::EMPTY);
        self.expect(RPAREN);
        self.finish_node();
    }

    pub(crate) fn type_mark(&mut self) {
        self.name();
    }

    fn range_constraint(&mut self) {
        self.start(RANGE_CONSTRAINT);
        self.bump();
        self.range();
        self.finish_node();
    }

    /// `a to b`, `a downto b`, or a range attribute such as `x'range`.
    pub(crate) fn range(&mut self) {
        let checkpoint = self.checkpoint();
        self.expression();
        if self.at(TO_KW) || self.at(DOWNTO_KW) {
            self.start_at(checkpoint, RANGE_EXPR);
            self.bump();
            self.expression();
            self.finish_node();
        }
    }

    /// An expression, a range, or a subtype indication with a range
    /// constraint (`integer range 0 to 7`). Used wherever these can't be
    /// told apart without semantic analysis.
    pub(crate) fn discrete_range(&mut self) {
        let checkpoint = self.checkpoint();
        self.expression();
        match self.current() {
            TO_KW | DOWNTO_KW => {
                self.start_at(checkpoint, RANGE_EXPR);
                self.bump();
                self.expression();
                self.finish_node();
            }
            RANGE_KW => {
                self.start_at(checkpoint, SUBTYPE_INDICATION);
                self.range_constraint();
                self.finish_node();
            }
            _ => {}
        }
    }
}

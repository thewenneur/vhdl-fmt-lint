//! Declarative parts and the declarations in them.

use crate::Standard;
use crate::SyntaxKind::*;
use crate::parser::{Parser, TokenSet};

/// Tokens that start a declaration.
pub(crate) const DECLARATION_START: TokenSet = TokenSet::new(&[
    TYPE_KW,
    SUBTYPE_KW,
    CONSTANT_KW,
    SIGNAL_KW,
    VARIABLE_KW,
    SHARED_KW,
    FILE_KW,
    ALIAS_KW,
    ATTRIBUTE_KW,
    COMPONENT_KW,
    FUNCTION_KW,
    PROCEDURE_KW,
    PURE_KW,
    IMPURE_KW,
    USE_KW,
    GROUP_KW,
    DISCONNECT_KW,
    FOR_KW,
    PACKAGE_KW,
    VIEW_KW,
]);

/// Where to resume after a broken declaration.
const DECLARATION_RECOVERY: TokenSet = DECLARATION_START.union(TokenSet::new(&[BEGIN_KW, END_KW]));

/// PSL declarations and directives that may appear among VHDL-2008 declarations.
const PSL_DECLARATIONS: [&str; 3] = ["default", "property", "sequence"];

impl Parser<'_> {
    pub(crate) fn at_declaration(&self) -> bool {
        self.at_set(DECLARATION_START) || self.at_psl_declaration()
    }

    fn at_psl_declaration(&self) -> bool {
        self.at(IDENT)
            && PSL_DECLARATIONS
                .iter()
                .any(|word| self.nth_text(0).eq_ignore_ascii_case(word))
            && matches!(self.nth(1), IDENT | EXT_IDENT)
    }

    /// Declarations up to `begin`, `end` or end of file.
    pub(crate) fn declarative_part(&mut self) {
        while !self.at(BEGIN_KW) && !self.at(END_KW) && !self.at_eof() {
            if self.at_declaration() {
                let before = self.progress();
                self.declaration();
                if self.progress() == before {
                    self.bump_error();
                }
            } else {
                self.recover("declaration", DECLARATION_RECOVERY);
            }
        }
    }

    pub(crate) fn declaration(&mut self) {
        match self.current() {
            TYPE_KW => self.type_decl(),
            SUBTYPE_KW => {
                self.start(SUBTYPE_DECL);
                self.bump();
                self.ident();
                self.expect(IS_KW);
                self.subtype_indication();
                self.expect(SEMI);
                self.finish_node();
            }
            CONSTANT_KW => self.object_decl(CONSTANT_DECL),
            SIGNAL_KW => self.object_decl(SIGNAL_DECL),
            VARIABLE_KW | SHARED_KW => self.object_decl(VARIABLE_DECL),
            FILE_KW => self.object_decl(FILE_DECL),
            ALIAS_KW => self.alias_decl(),
            ATTRIBUTE_KW => self.attribute_decl_or_spec(),
            COMPONENT_KW => self.component_decl(),
            FUNCTION_KW | PROCEDURE_KW | PURE_KW | IMPURE_KW => self.subprogram(),
            USE_KW => self.use_clause(),
            GROUP_KW => self.group_decl(),
            DISCONNECT_KW => {
                self.start(DISCONNECTION_SPEC);
                self.bump();
                self.signal_list();
                self.expect(COLON);
                self.name();
                self.expect(AFTER_KW);
                self.expression();
                self.expect(SEMI);
                self.finish_node();
            }
            FOR_KW => self.configuration_spec(),
            PACKAGE_KW => {
                self.require(Standard::Vhdl2008, "nested packages");
                self.package_unit();
            }
            VIEW_KW => self.mode_view_decl(),
            _ if self.at_psl_declaration() => self.psl_directive(),
            _ => self.recover("declaration", DECLARATION_RECOVERY),
        }
    }

    /// A PSL declaration or directive, kept as an opaque run of tokens up to `;`.
    pub(crate) fn psl_directive(&mut self) {
        self.start(PSL_DIRECTIVE);
        let mut depth = 0u32;
        while !self.at_eof() {
            match self.current() {
                LPAREN | LBRACE | LBRACKET => depth += 1,
                RPAREN | RBRACE | RBRACKET => depth = depth.saturating_sub(1),
                SEMI if depth == 0 => break,
                END_KW if depth == 0 => break,
                _ => {}
            }
            self.bump();
        }
        self.expect(SEMI);
        self.finish_node();
    }

    /// `signal`, `constant`, `variable` and `file` declarations.
    fn object_decl(&mut self, kind: crate::SyntaxKind) {
        self.start(kind);
        self.eat(SHARED_KW);
        self.bump();
        self.ident_list();
        self.expect(COLON);
        self.subtype_indication();
        if kind == SIGNAL_DECL && !self.eat(REGISTER_KW) {
            self.eat(BUS_KW);
        }
        if kind == FILE_DECL {
            if self.at(OPEN_KW) || self.at(IS_KW) {
                self.start(FILE_OPEN_INFO);
                if self.eat(OPEN_KW) {
                    self.expression();
                }
                if self.eat(IS_KW) {
                    // VHDL-87 style `is in "name"`.
                    if self.at(IN_KW) || self.at(OUT_KW) {
                        self.bump();
                    }
                    self.expression();
                }
                self.finish_node();
            }
        } else if self.eat(COLON_EQ) {
            self.conditional_expression(Standard::Vhdl2019);
        }
        self.expect(SEMI);
        self.finish_node();
    }

    fn alias_decl(&mut self) {
        self.start(ALIAS_DECL);
        self.bump();
        if self.at(IDENT) || self.at(EXT_IDENT) || self.at(CHAR_LITERAL) || self.at(STRING_LITERAL)
        {
            self.bump();
        } else {
            self.error_expected("alias designator");
        }
        if self.eat(COLON) {
            self.subtype_indication();
        }
        self.expect(IS_KW);
        self.name();
        self.expect(SEMI);
        self.finish_node();
    }

    fn attribute_decl_or_spec(&mut self) {
        if self.nth_at(2, COLON) {
            self.start(ATTRIBUTE_DECL);
            self.bump();
            self.ident();
            self.bump();
            self.name();
        } else {
            self.start(ATTRIBUTE_SPEC);
            self.bump();
            self.ident();
            self.expect(OF_KW);
            self.start(ENTITY_SPEC);
            if !self.eat(ALL_KW) && !self.eat(OTHERS_KW) {
                loop {
                    if self.at(IDENT)
                        || self.at(EXT_IDENT)
                        || self.at(CHAR_LITERAL)
                        || self.at(STRING_LITERAL)
                    {
                        self.bump();
                        if self.at(LBRACKET) {
                            self.signature();
                        }
                    } else {
                        self.error_expected("entity designator");
                        break;
                    }
                    if !self.eat(COMMA) {
                        break;
                    }
                }
            }
            self.expect(COLON);
            // The entity class: `signal`, `entity`, `label`, `literal`, ...
            if self.at(IDENT) || self.current().is_keyword() {
                self.bump();
            } else {
                self.error_expected("entity class");
            }
            self.finish_node();
            self.expect(IS_KW);
            self.conditional_expression(Standard::Vhdl2019);
        }
        self.expect(SEMI);
        self.finish_node();
    }

    fn component_decl(&mut self) {
        self.start(COMPONENT_DECL);
        self.bump();
        self.ident();
        self.eat(IS_KW);
        if self.at(GENERIC_KW) {
            self.generic_clause();
        }
        if self.at(PORT_KW) {
            self.port_clause();
        }
        self.end_clause(&[COMPONENT_KW], true);
        self.finish_node();
    }

    fn group_decl(&mut self) {
        if self.nth_at(2, IS_KW) {
            self.start(GROUP_TEMPLATE_DECL);
            self.bump();
            self.ident();
            self.bump();
            self.expect(LPAREN);
            loop {
                if self.at(IDENT) || self.current().is_keyword() {
                    self.bump();
                } else {
                    self.error_expected("entity class");
                    break;
                }
                self.eat(BOX);
                if !self.eat(COMMA) {
                    break;
                }
            }
            self.expect(RPAREN);
        } else {
            self.start(GROUP_DECL);
            self.bump();
            self.ident();
            self.expect(COLON);
            self.name();
        }
        self.expect(SEMI);
        self.finish_node();
    }

    /// `all`, `others`, or a list of signal names.
    fn signal_list(&mut self) {
        if !self.eat(ALL_KW) && !self.eat(OTHERS_KW) {
            self.name();
            while self.eat(COMMA) {
                self.name();
            }
        }
    }

    fn mode_view_decl(&mut self) {
        self.start(MODE_VIEW_DECL);
        self.bump();
        self.ident();
        self.expect(OF_KW);
        self.subtype_indication();
        self.expect(IS_KW);
        while !self.at(END_KW) && !self.at_eof() {
            if !(self.at(IDENT) || self.at(EXT_IDENT)) {
                self.recover("mode view element", TokenSet::new(&[END_KW]));
                continue;
            }
            self.start(MODE_VIEW_ELEMENT);
            self.ident_list();
            self.expect(COLON);
            self.mode_or_view();
            self.expect(SEMI);
            self.finish_node();
        }
        self.end_clause(&[VIEW_KW], true);
        self.finish_node();
    }

    // ---- Subprograms ------------------------------------------------------

    fn at_subprogram_instantiation(&self) -> bool {
        let offset = if matches!(self.current(), PURE_KW | IMPURE_KW) {
            1
        } else {
            0
        };
        self.nth_at(offset + 2, IS_KW) && self.nth_at(offset + 3, NEW_KW)
    }

    /// A subprogram declaration, body or instantiation.
    pub(crate) fn subprogram(&mut self) {
        let checkpoint = self.checkpoint();
        let end_kw = if self.at(PROCEDURE_KW) {
            PROCEDURE_KW
        } else {
            FUNCTION_KW
        };
        if self.at_subprogram_instantiation() {
            self.start(SUBPROGRAM_INSTANTIATION_DECL);
            self.require(Standard::Vhdl2008, "subprogram instantiations");
            if !self.eat(PURE_KW) {
                self.eat(IMPURE_KW);
            }
            self.bump();
            self.bump();
            self.bump();
            self.bump();
            self.name();
            if self.at(GENERIC_KW) {
                self.map_aspect(GENERIC_KW);
            }
            self.expect(SEMI);
            self.finish_node();
            return;
        }
        self.subprogram_spec();
        if self.at(IS_KW) {
            self.start_at(checkpoint, SUBPROGRAM_BODY);
            self.bump();
            self.declarative_part();
            self.expect(BEGIN_KW);
            self.sequential_statements();
            self.end_clause(&[end_kw], false);
        } else {
            self.start_at(checkpoint, SUBPROGRAM_DECL);
            self.expect(SEMI);
        }
        self.finish_node();
    }

    /// `[pure | impure] function f [generic (...)] [[parameter] (...)] return t`
    /// or `procedure p [generic (...)] [[parameter] (...)]`.
    pub(crate) fn subprogram_spec(&mut self) {
        let is_function = !self.at(PROCEDURE_KW);
        self.start(if is_function {
            FUNCTION_SPEC
        } else {
            PROCEDURE_SPEC
        });
        if !self.eat(PURE_KW) {
            self.eat(IMPURE_KW);
        }
        self.bump();
        if self.at(IDENT) || self.at(EXT_IDENT) || self.at(STRING_LITERAL) {
            self.bump();
        } else {
            self.error_expected("subprogram name");
        }
        if self.at(GENERIC_KW) {
            self.require(Standard::Vhdl2008, "subprogram generics");
            self.start(GENERIC_CLAUSE);
            self.bump();
            self.interface_list();
            self.finish_node();
            if self.at(GENERIC_KW) {
                self.map_aspect(GENERIC_KW);
            }
        }
        if self.at(PARAMETER_KW) || self.at(LPAREN) {
            self.eat(PARAMETER_KW);
            self.interface_list();
        }
        if is_function {
            self.expect(RETURN_KW);
            // VHDL-2019: `return r of t` names the return value.
            if (self.at(IDENT) || self.at(EXT_IDENT)) && self.nth_at(1, OF_KW) {
                self.require(Standard::Vhdl2019, "named return values");
                self.bump();
                self.bump();
            }
            self.type_mark();
        }
        self.finish_node();
    }
}

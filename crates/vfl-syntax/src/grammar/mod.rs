//! The VHDL grammar, as recursive descent over [`Parser`].
//!
//! Each function parses one construct and leaves the cursor after it. On
//! malformed input a function reports an error and returns without
//! consuming what it doesn't understand; the enclosing list loops skip such
//! tokens, so every token always ends up in the tree.

mod concurrent;
mod declarations;
mod expressions;
mod interfaces;
mod sequential;
mod types;

use crate::Standard;
use crate::SyntaxKind::{self, *};
use crate::parser::{Parser, TokenSet};

/// Tokens that start a design unit or a context item.
const UNIT_START: TokenSet = TokenSet::new(&[
    LIBRARY_KW,
    USE_KW,
    CONTEXT_KW,
    ENTITY_KW,
    ARCHITECTURE_KW,
    PACKAGE_KW,
    CONFIGURATION_KW,
]);

/// Keywords that can follow `end` to close a construct.
const END_KEYWORDS: TokenSet = TokenSet::new(&[
    IF_KW,
    CASE_KW,
    LOOP_KW,
    PROCESS_KW,
    POSTPONED_KW,
    BLOCK_KW,
    GENERATE_KW,
    RECORD_KW,
    UNITS_KW,
    PROTECTED_KW,
    COMPONENT_KW,
    ENTITY_KW,
    ARCHITECTURE_KW,
    PACKAGE_KW,
    CONFIGURATION_KW,
    FUNCTION_KW,
    PROCEDURE_KW,
    CONTEXT_KW,
    FOR_KW,
    VIEW_KW,
]);

/// Tokens that can name something being declared or closed.
const DESIGNATOR: TokenSet = TokenSet::new(&[IDENT, EXT_IDENT, STRING_LITERAL, CHAR_LITERAL]);

impl Parser<'_> {
    pub(crate) fn source_file(&mut self) {
        self.start_root(SOURCE_FILE);
        while !self.at_eof() {
            if self.at_set(UNIT_START) {
                self.design_unit();
            } else if self.at_psl_verification_unit() {
                self.psl_verification_unit();
            } else {
                self.recover("design unit", UNIT_START);
            }
        }
        // The root node is closed by `Parser::finish`, after the trailing trivia.
    }

    fn at_psl_verification_unit(&self) -> bool {
        ["vunit", "vmode", "vprop"]
            .iter()
            .any(|word| self.at_contextual(word))
            && matches!(self.nth(1), IDENT | EXT_IDENT)
    }

    /// `vunit name [(binding)] { ... }`: a PSL verification unit, kept as an
    /// opaque run of tokens.
    fn psl_verification_unit(&mut self) {
        self.start(PSL_DIRECTIVE);
        while !self.at_eof() && !self.at(LBRACE) {
            self.bump();
        }
        let mut depth = 0u32;
        while !self.at_eof() {
            match self.current() {
                LBRACE => depth += 1,
                RBRACE => depth -= 1,
                _ => {}
            }
            self.bump();
            if depth == 0 {
                break;
            }
        }
        self.finish_node();
    }

    fn at_context_item(&self) -> bool {
        match self.current() {
            LIBRARY_KW | USE_KW => true,
            CONTEXT_KW => !self.nth_at(2, IS_KW),
            _ => false,
        }
    }

    fn design_unit(&mut self) {
        self.start(DESIGN_UNIT);
        if self.at_context_item() {
            self.start(CONTEXT_CLAUSE);
            while self.at_context_item() {
                self.context_item();
            }
            self.finish_node();
        }
        match self.current() {
            ENTITY_KW => self.entity_decl(),
            ARCHITECTURE_KW => self.architecture_body(),
            PACKAGE_KW => self.package_unit(),
            CONFIGURATION_KW => self.configuration_decl(),
            CONTEXT_KW => self.context_decl(),
            _ if self.at_psl_verification_unit() => self.psl_verification_unit(),
            _ => self.error_expected("design unit"),
        }
        self.finish_node();
    }

    fn context_item(&mut self) {
        match self.current() {
            LIBRARY_KW => {
                self.start(LIBRARY_CLAUSE);
                self.bump();
                self.ident_list();
                self.expect(SEMI);
                self.finish_node();
            }
            USE_KW => self.use_clause(),
            _ => {
                self.start(CONTEXT_REFERENCE);
                self.require(Standard::Vhdl2008, "context references");
                self.bump();
                self.name_list();
                self.expect(SEMI);
                self.finish_node();
            }
        }
    }

    pub(crate) fn use_clause(&mut self) {
        self.start(USE_CLAUSE);
        self.expect(USE_KW);
        self.name_list();
        self.expect(SEMI);
        self.finish_node();
    }

    fn name_list(&mut self) {
        self.name();
        while self.eat(COMMA) {
            self.name();
        }
    }

    fn entity_decl(&mut self) {
        self.start(ENTITY_DECL);
        self.bump();
        self.ident();
        self.expect(IS_KW);
        if self.at(GENERIC_KW) {
            self.generic_clause();
        }
        if self.at(PORT_KW) {
            self.port_clause();
        }
        self.declarative_part();
        if self.eat(BEGIN_KW) {
            self.concurrent_statements();
        }
        self.end_clause(&[ENTITY_KW], false);
        self.finish_node();
    }

    fn architecture_body(&mut self) {
        self.start(ARCHITECTURE_BODY);
        self.bump();
        self.ident();
        self.expect(OF_KW);
        self.name();
        self.expect(IS_KW);
        self.declarative_part();
        self.expect(BEGIN_KW);
        self.concurrent_statements();
        self.end_clause(&[ARCHITECTURE_KW], false);
        self.finish_node();
    }

    /// A package declaration, package body or package instantiation.
    pub(crate) fn package_unit(&mut self) {
        if self.nth_at(1, BODY_KW) {
            self.start(PACKAGE_BODY);
            self.bump();
            self.bump();
            self.ident();
            self.expect(IS_KW);
            self.declarative_part();
            self.end_clause(&[PACKAGE_KW, BODY_KW], false);
            self.finish_node();
        } else if self.nth_at(2, IS_KW) && self.nth_at(3, NEW_KW) {
            self.start(PACKAGE_INSTANTIATION_DECL);
            self.require(Standard::Vhdl2008, "package instantiations");
            self.bump();
            self.ident();
            self.bump();
            self.bump();
            self.name();
            if self.at(GENERIC_KW) {
                self.map_aspect(GENERIC_KW);
            }
            self.expect(SEMI);
            self.finish_node();
        } else {
            self.start(PACKAGE_DECL);
            self.bump();
            self.ident();
            self.expect(IS_KW);
            if self.at(GENERIC_KW) {
                self.require(Standard::Vhdl2008, "package generics");
                self.generic_clause();
                if self.at(GENERIC_KW) {
                    self.map_aspect(GENERIC_KW);
                    self.expect(SEMI);
                }
            }
            self.declarative_part();
            self.end_clause(&[PACKAGE_KW], false);
            self.finish_node();
        }
    }

    fn context_decl(&mut self) {
        self.start(CONTEXT_DECL);
        self.bump();
        self.ident();
        self.expect(IS_KW);
        while !self.at(END_KW) && !self.at_eof() {
            if self.at_context_item() {
                self.context_item();
            } else {
                let before = self.progress();
                self.recover(
                    "library, use or context clause",
                    UNIT_START.union(TokenSet::new(&[END_KW])),
                );
                if self.progress() == before {
                    break;
                }
            }
        }
        self.end_clause(&[CONTEXT_KW], false);
        self.finish_node();
    }

    fn configuration_decl(&mut self) {
        self.start(CONFIGURATION_DECL);
        self.bump();
        self.ident();
        self.expect(OF_KW);
        self.name();
        self.expect(IS_KW);
        loop {
            match self.current() {
                USE_KW => self.use_clause(),
                ATTRIBUTE_KW | GROUP_KW => self.declaration(),
                _ => break,
            }
        }
        if self.at(FOR_KW) {
            self.block_configuration();
        } else {
            self.error_expected("block configuration");
        }
        self.end_clause(&[CONFIGURATION_KW], false);
        self.finish_node();
    }

    /// `for` followed by an instantiation list and `:` starts a component configuration.
    fn at_component_configuration(&self) -> bool {
        match self.nth(1) {
            ALL_KW | OTHERS_KW => true,
            IDENT | EXT_IDENT => matches!(self.nth(2), COLON | COMMA),
            _ => false,
        }
    }

    fn block_configuration(&mut self) {
        self.start(BLOCK_CONFIGURATION);
        self.bump();
        self.name();
        while self.at(USE_KW) {
            self.use_clause();
        }
        while self.at(FOR_KW) {
            if self.at_component_configuration() {
                self.component_configuration();
            } else {
                self.block_configuration();
            }
        }
        self.end_clause(&[FOR_KW], true);
        self.finish_node();
    }

    fn component_configuration(&mut self) {
        self.start(COMPONENT_CONFIGURATION);
        self.bump();
        self.instantiation_list();
        self.expect(COLON);
        self.name();
        if self.at_set(TokenSet::new(&[USE_KW, GENERIC_KW, PORT_KW])) {
            self.binding_indication();
            self.expect(SEMI);
        }
        if self.at(FOR_KW) {
            self.block_configuration();
        }
        self.end_clause(&[FOR_KW], true);
        self.finish_node();
    }

    /// A configuration specification, in a declarative part.
    pub(crate) fn configuration_spec(&mut self) {
        self.start(CONFIGURATION_SPEC);
        self.bump();
        self.instantiation_list();
        self.expect(COLON);
        self.name();
        self.binding_indication();
        self.expect(SEMI);
        if self.at(END_KW) && self.nth_at(1, FOR_KW) {
            self.end_clause(&[FOR_KW], true);
        }
        self.finish_node();
    }

    fn instantiation_list(&mut self) {
        if !self.eat(ALL_KW) && !self.eat(OTHERS_KW) {
            self.ident_list();
        }
    }

    fn binding_indication(&mut self) {
        self.start(BINDING_INDICATION);
        if self.eat(USE_KW) {
            match self.current() {
                ENTITY_KW | CONFIGURATION_KW => {
                    self.bump();
                    self.name();
                }
                OPEN_KW => self.bump(),
                _ if self.at_contextual("vunit") => {
                    self.bump();
                    self.name_list();
                }
                _ => self.error_expected("`entity`, `configuration` or `open`"),
            }
        }
        if self.at(GENERIC_KW) {
            self.map_aspect(GENERIC_KW);
        }
        if self.at(PORT_KW) {
            self.map_aspect(PORT_KW);
        }
        self.finish_node();
    }

    // ---- Small shared pieces ----------------------------------------------

    /// The identifier a declaration introduces.
    pub(crate) fn ident(&mut self) {
        if self.at(IDENT) || self.at(EXT_IDENT) {
            self.bump();
        } else {
            self.error_expected("identifier");
        }
    }

    pub(crate) fn ident_list(&mut self) {
        self.start(IDENT_LIST);
        self.ident();
        while self.eat(COMMA) {
            self.ident();
        }
        self.finish_node();
    }

    /// An optional `label :` in front of a statement.
    pub(crate) fn opt_label(&mut self) -> bool {
        if (self.at(IDENT) || self.at(EXT_IDENT)) && self.nth_at(1, COLON) {
            self.start(LABEL);
            self.bump();
            self.bump();
            self.finish_node();
            true
        } else {
            false
        }
    }

    /// `end [keywords] [designator] ;`.
    ///
    /// With `required`, the keywords must be there. If `end` is followed by
    /// some other construct's keyword, this `end` belongs to an enclosing
    /// construct: report the missing `end` and leave it alone.
    pub(crate) fn end_clause(&mut self, keywords: &[SyntaxKind], required: bool) {
        if !self.at(END_KW) {
            let what = match keywords.first() {
                Some(kw) => format!("`end {}`", kw.text().unwrap_or_default()),
                None => "`end`".to_owned(),
            };
            self.error_expected(&what);
            return;
        }
        let next = self.nth(1);
        if let Some(&first) = keywords.first() {
            let other_construct = END_KEYWORDS.contains(next) && next != first;
            // `end postponed process` closes a process.
            let postponed = first == PROCESS_KW && next == POSTPONED_KW;
            if other_construct && !postponed {
                let what = format!("`end {}`", first.text().unwrap_or_default());
                self.error_expected(&what);
                return;
            }
        }
        self.bump();
        if keywords.first() == Some(&PROCESS_KW) {
            self.eat(POSTPONED_KW);
        }
        for (i, &kw) in keywords.iter().enumerate() {
            if !self.eat(kw) {
                if required || i > 0 {
                    self.error_expected(&kw.describe());
                }
                break;
            }
        }
        if keywords.first() == Some(&CASE_KW) {
            self.eat(QUESTION);
        }
        if self.at_set(DESIGNATOR) {
            self.bump();
        }
        self.expect(SEMI);
    }
}

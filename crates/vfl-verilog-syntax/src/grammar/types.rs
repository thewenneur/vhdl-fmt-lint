//! Data types, net types, dimensions and declarations.

use super::NAME_TOKENS;
use crate::SyntaxKind::*;
use crate::parser::{Parser, TokenSet};

/// Keywords that start a data type.
pub(super) const TYPE_KEYWORDS: TokenSet = TokenSet::new(&[
    BIT_KW,
    LOGIC_KW,
    REG_KW,
    BYTE_KW,
    SHORTINT_KW,
    INT_KW,
    LONGINT_KW,
    INTEGER_KW,
    TIME_KW,
    SHORTREAL_KW,
    REAL_KW,
    REALTIME_KW,
    STRING_KW,
    CHANDLE_KW,
    EVENT_KW,
    VOID_KW,
    STRUCT_KW,
    UNION_KW,
    ENUM_KW,
    TYPE_KW,
    VIRTUAL_KW,
]);

/// Net type keywords.
pub(super) const NET_TYPES: TokenSet = TokenSet::new(&[
    WIRE_KW,
    TRI_KW,
    TRI0_KW,
    TRI1_KW,
    TRIAND_KW,
    TRIOR_KW,
    TRIREG_KW,
    WAND_KW,
    WOR_KW,
    UWIRE_KW,
    SUPPLY0_KW,
    SUPPLY1_KW,
    INTERCONNECT_KW,
]);

/// Keywords in a drive or charge strength.
const STRENGTHS: TokenSet = TokenSet::new(&[
    SUPPLY0_KW, SUPPLY1_KW, STRONG0_KW, STRONG1_KW, PULL0_KW, PULL1_KW, WEAK0_KW, WEAK1_KW,
    HIGHZ0_KW, HIGHZ1_KW, SMALL_KW, MEDIUM_KW, LARGE_KW,
]);

/// Port directions.
pub(super) const DIRECTIONS: TokenSet = TokenSet::new(&[INPUT_KW, OUTPUT_KW, INOUT_KW, REF_KW]);

/// Lifetimes and qualifiers that can start a variable declaration.
const VAR_QUALIFIERS: TokenSet = TokenSet::new(&[CONST_KW, VAR_KW, STATIC_KW, AUTOMATIC_KW]);

impl Parser<'_> {
    // ---- Looking ahead -----------------------------------------------------

    /// If a type name (`t`, `p::t`, `c#(8)::t`, `` `T ``, `i.mp`) starts at
    /// the `n`th token ahead, the index just after it.
    pub(super) fn type_name_end(&self, mut n: usize) -> Option<usize> {
        match self.nth(n) {
            IDENT | ESC_IDENT | MACRO_REF => n += 1,
            MACRO_CALL => {
                n += 1;
                if self.nth_at(n, LPAREN) {
                    n = self.skip_group_ahead(n);
                }
            }
            // `$unit::t`.
            SYSTEM_IDENT if self.nth_at(n + 1, COLON2) => n += 1,
            _ => return None,
        }
        loop {
            match self.nth(n) {
                COLON2 if matches!(self.nth(n + 1), IDENT | ESC_IDENT) => n += 2,
                HASH if self.nth_at(n + 1, LPAREN) => n = self.skip_group_ahead(n + 1),
                // An interface port with a modport: `bus.slave s`.
                DOT if matches!(self.nth(n + 1), IDENT | ESC_IDENT)
                    && matches!(self.nth(n + 2), IDENT | ESC_IDENT) =>
                {
                    n += 2
                }
                _ => return Some(n),
            }
        }
    }

    /// Whether a declaration with a named type starts here: a type name,
    /// packed dimensions, then the name being declared.
    pub(super) fn at_named_type_decl(&self) -> bool {
        self.named_type_decl_name(0).is_some()
    }

    /// The index of the declared name, if a named type starts at `n`.
    pub(super) fn named_type_decl_name(&self, n: usize) -> Option<usize> {
        let macro_type = matches!(self.nth(n), MACRO_REF | MACRO_CALL);
        let mut n = self.type_name_end(n)?;
        while self.nth_at(n, LBRACKET) {
            n = self.skip_group_ahead(n);
        }
        if macro_type {
            // Macros used as statements follow each other without `;`:
            // `` `FF(a, b) `FF(c, d) ``, `` `debug(...) x <= 1; `` and
            // `` `TYPEDEF(t) t [1:0] x; `` are not declarations of a macro
            // type. One needs a plain name followed by `;`, `,` or `=`.
            return (matches!(self.nth(n), IDENT | ESC_IDENT)
                && matches!(self.nth(n + 1), SEMI | COMMA | EQ))
            .then_some(n);
        }
        matches!(self.nth(n), IDENT | ESC_IDENT | MACRO_REF | MACRO_CALL).then_some(n)
    }

    /// Whether a data type starts here (a keyword type, an implicit type,
    /// or a named type followed by the name being declared).
    pub(super) fn at_data_type(&self) -> bool {
        self.at_set(TYPE_KEYWORDS)
            || matches!(self.current(), SIGNED_KW | UNSIGNED_KW | LBRACKET)
            || self.at_named_type_decl()
    }

    /// Whether a variable, net or type declaration starts here, in a block
    /// or a module body.
    pub(super) fn at_declaration(&self) -> bool {
        match self.current() {
            // `virtual class` is not a declaration of a variable.
            VIRTUAL_KW => !self.nth_at(1, CLASS_KW),
            // `type(x) y;`, but not `type` alone.
            k if TYPE_KEYWORDS.contains(k) => true,
            k if VAR_QUALIFIERS.contains(k) || NET_TYPES.contains(k) => true,
            TYPEDEF_KW | PARAMETER_KW | LOCALPARAM_KW => true,
            _ => self.at_named_type_decl(),
        }
    }

    // ---- Types -------------------------------------------------------------

    /// A data type, in a `DATA_TYPE` node. Accepts a net type first
    /// (`wire logic [7:0]`), and implicit types (`signed [7:0]`).
    pub(crate) fn data_type(&mut self) {
        self.start(DATA_TYPE);
        self.data_type_inner();
        self.finish_node();
    }

    fn data_type_inner(&mut self) {
        if self.at_set(NET_TYPES) || self.at(VAR_KW) {
            self.bump();
            if self.at(LPAREN) && STRENGTHS.contains(self.nth(1)) {
                self.strength();
            }
            if self.at(VECTORED_KW) || self.at(SCALARED_KW) {
                self.bump();
            }
            // The data type is optional after a net type or `var`.
            if !(self.at_set(TYPE_KEYWORDS)
                || matches!(self.current(), SIGNED_KW | UNSIGNED_KW | LBRACKET)
                || self.at_named_type_decl())
            {
                return;
            }
        }
        match self.current() {
            STRUCT_KW | UNION_KW => self.struct_type(),
            ENUM_KW => self.enum_type(),
            TYPE_KW => {
                self.start(TYPE_REFERENCE);
                self.bump();
                if self.at(LPAREN) {
                    self.bump();
                    self.expr_or_type();
                    self.expect(RPAREN);
                }
                self.finish_node();
            }
            VIRTUAL_KW => {
                self.bump();
                self.eat(INTERFACE_KW);
                self.type_name();
            }
            k if TYPE_KEYWORDS.contains(k) => self.bump(),
            SIGNED_KW | UNSIGNED_KW | LBRACKET => {}
            k if NAME_TOKENS.contains(k) || k == SYSTEM_IDENT => self.type_name(),
            _ => {
                self.error_expected("data type");
                return;
            }
        }
        if self.at(SIGNED_KW) || self.at(UNSIGNED_KW) {
            self.bump();
        }
        while self.at(LBRACKET) {
            self.dimension();
        }
    }

    /// A named type: `t`, `p::t`, `c#(8)::t`, `i.mp`, in a `NAME_REF`.
    fn type_name(&mut self) {
        self.start(NAME_REF);
        self.bump_name_token();
        loop {
            match self.current() {
                COLON2 if matches!(self.nth(1), IDENT | ESC_IDENT) => {
                    self.bump();
                    self.bump();
                }
                HASH if self.nth_at(1, LPAREN) => self.param_values(),
                DOT if matches!(self.nth(1), IDENT | ESC_IDENT) => {
                    self.bump();
                    self.bump();
                }
                _ => break,
            }
        }
        self.finish_node();
    }

    /// `struct packed signed { ... }`, `union tagged { ... }`.
    fn struct_type(&mut self) {
        self.start(STRUCT_TYPE);
        self.bump();
        self.eat(TAGGED_KW);
        if self.eat(PACKED_KW) && (self.at(SIGNED_KW) || self.at(UNSIGNED_KW)) {
            self.bump();
        }
        if self.expect(LBRACE) {
            while !self.at_eof() && !self.at(RBRACE) {
                let progress = self.progress();
                self.struct_member();
                if self.progress() == progress {
                    self.bump_error();
                }
            }
            self.expect(RBRACE);
        }
        self.finish_node();
    }

    fn struct_member(&mut self) {
        if (self.at(MACRO_CALL) || self.at(MACRO_REF)) && !self.at_named_type_decl() {
            self.macro_item();
            return;
        }
        self.start(STRUCT_MEMBER);
        if self.at(RAND_KW) || self.at(RANDC_KW) {
            self.bump();
        }
        self.data_type();
        self.declarators();
        self.expect(SEMI);
        self.finish_node();
    }

    /// `enum logic [1:0] { A, B = 2, C[3] }`.
    fn enum_type(&mut self) {
        self.start(ENUM_TYPE);
        self.bump();
        if !self.at(LBRACE) {
            self.data_type();
        }
        if self.expect(LBRACE) {
            loop {
                if self.at(RBRACE) {
                    break;
                }
                self.start(ENUM_MEMBER);
                self.name();
                if self.at(LBRACKET) {
                    self.dimension();
                }
                if self.eat(EQ) {
                    self.expr();
                }
                self.finish_node();
                if !self.eat(COMMA) {
                    break;
                }
            }
            self.expect(RBRACE);
        }
        self.finish_node();
    }

    /// `[7:0]`, `[N]`, `[]`, `[$]`, `[$:7]`, `[*]`, `[string]`.
    pub(crate) fn dimension(&mut self) {
        self.start(DIMENSION);
        self.bump();
        match self.current() {
            RBRACKET => {}
            STAR if self.nth_at(1, RBRACKET) => self.bump(),
            _ => {
                self.expr_or_type();
                if matches!(self.current(), COLON | PLUS_COLON | MINUS_COLON) {
                    self.bump();
                    self.expr();
                }
            }
        }
        self.expect(RBRACKET);
        self.finish_node();
    }

    /// `(strong0, weak1)`, `(small)`.
    pub(crate) fn strength(&mut self) {
        self.start(STRENGTH);
        self.balanced();
        self.finish_node();
    }

    /// `#5`, `#(1, 2)`, `#1ns`, `#d`, in a `DELAY` node.
    pub(crate) fn delay(&mut self) {
        self.start(DELAY);
        self.bump();
        match self.current() {
            LPAREN => {
                self.bump();
                loop {
                    self.expr();
                    // `#(1:2:3)`.
                    while self.eat(COLON) {
                        self.expr();
                    }
                    if !self.eat(COMMA) {
                        break;
                    }
                }
                self.expect(RPAREN);
            }
            INT_NUMBER | REAL_NUMBER | TIME_LITERAL | BASED_NUMBER | MACRO_REF => self.bump(),
            MACRO_CALL => self.bump_name_token(),
            IDENT | ESC_IDENT | SYSTEM_IDENT => {
                let checkpoint = self.checkpoint();
                self.scoped_name();
                // `#cfg.delay`.
                while self.at(DOT) && matches!(self.nth(1), IDENT | ESC_IDENT) {
                    self.bump();
                    self.bump();
                    self.start_at(checkpoint, FIELD_EXPR);
                    self.finish_node();
                }
            }
            _ => self.error_expected("delay value"),
        }
        self.finish_node();
    }

    // ---- Declarations ------------------------------------------------------

    /// `name [dims] [= value]` declarators, separated by commas.
    pub(crate) fn declarators(&mut self) {
        loop {
            self.declarator();
            if !self.eat(COMMA) {
                break;
            }
        }
    }

    fn declarator(&mut self) {
        self.start(DECLARATOR);
        self.name();
        while self.at(LBRACKET) {
            self.dimension();
        }
        if self.eat(EQ) {
            if self.at(NEW_KW) {
                self.expr();
            } else {
                self.expr_or_type();
            }
        }
        self.finish_node();
    }

    /// A net declaration: `wire [7:0] a, b = c;`.
    pub(crate) fn net_decl(&mut self) {
        self.start(NET_DECL);
        self.start(DATA_TYPE);
        // The net type, strength and data type, then a delay.
        self.data_type_inner();
        self.finish_node();
        if self.at(HASH) {
            self.delay();
        }
        self.declarators();
        self.expect(SEMI);
        self.finish_node();
    }

    /// A variable declaration: `logic [7:0] a = 0, b;`, `const int c = 1;`,
    /// `t_state state;`. `kind` is `DATA_DECL` or `LOCAL_DECL`.
    pub(crate) fn data_decl(&mut self, kind: crate::SyntaxKind) {
        self.start(kind);
        self.data_decl_body();
        self.expect(SEMI);
        self.finish_node();
    }

    /// A declaration without its `;`, as in a `for` header.
    pub(crate) fn data_decl_body(&mut self) {
        while self.at(CONST_KW) || self.at(STATIC_KW) || self.at(AUTOMATIC_KW) {
            self.bump();
        }
        if self.at(VAR_KW) {
            self.data_type();
        } else {
            // A type, unless the declaration only names variables after
            // `const` or a lifetime (`automatic i = 0`).
            if !(self.nth_is_name(0) && !self.at_named_type_decl()) {
                self.data_type();
            }
        }
        self.declarators();
    }

    /// `typedef logic [7:0] byte_t;`, `typedef enum {...} state_t;`,
    /// `typedef class c;`.
    pub(crate) fn typedef_decl(&mut self) {
        self.start(TYPEDEF_DECL);
        self.bump();
        let forward = match self.current() {
            CLASS_KW | ENUM_KW | STRUCT_KW | UNION_KW => {
                self.nth_is_name(1) && self.nth_at(2, SEMI)
            }
            INTERFACE_KW => self.nth_at(1, CLASS_KW),
            _ => self.nth_is_name(0) && self.nth_at(1, SEMI),
        };
        if forward {
            while !self.at_eof() && !self.at(SEMI) && !self.nth_is_name(0) {
                self.bump();
            }
            self.name();
        } else {
            self.data_type();
            self.name();
            while self.at(LBRACKET) {
                self.dimension();
            }
        }
        self.expect(SEMI);
        self.finish_node();
    }

    /// `parameter int W = 8, D = 2;`, `localparam type T = logic;`.
    pub(crate) fn param_decl_stmt(&mut self) {
        self.start(PARAM_DECL);
        self.bump();
        self.param_type();
        self.param_declarators();
        self.expect(SEMI);
        self.finish_node();
    }

    /// One parameter of a `#(...)` list.
    pub(crate) fn param_port(&mut self) {
        self.start(PARAM_DECL);
        if matches!(self.current(), PARAMETER_KW | LOCALPARAM_KW) {
            self.bump();
        }
        self.param_type();
        self.declarator();
        self.finish_node();
    }

    fn param_type(&mut self) {
        if self.at(TYPE_KW) && !self.nth_at(1, LPAREN) {
            self.bump();
        } else if self.at_set(TYPE_KEYWORDS)
            || matches!(self.current(), SIGNED_KW | UNSIGNED_KW | LBRACKET)
            || self.at_named_type_decl()
        {
            self.data_type();
        }
    }

    fn param_declarators(&mut self) {
        loop {
            self.declarator();
            // `parameter A = 1, parameter B = 2` does not exist, but `,`
            // followed by a type does in parameter port lists.
            if !self.eat(COMMA) {
                break;
            }
        }
    }

    /// `input [7:0] a, b;` in a module body or an old-style function.
    pub(crate) fn port_decl(&mut self) {
        self.start(PORT_DECL);
        if self.at(CONST_KW) {
            self.bump();
        }
        self.bump();
        if self.at_set(NET_TYPES)
            || self.at(VAR_KW)
            || self.at_set(TYPE_KEYWORDS)
            || matches!(self.current(), SIGNED_KW | UNSIGNED_KW | LBRACKET)
            || self.at_named_type_decl()
        {
            self.data_type();
        }
        self.declarators();
        self.expect(SEMI);
        self.finish_node();
    }

    /// `genvar i, j;`
    pub(crate) fn genvar_decl(&mut self) {
        self.start(GENVAR_DECL);
        self.bump();
        self.declarators();
        self.expect(SEMI);
        self.finish_node();
    }
}

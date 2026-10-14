//! Design elements and the items in their bodies.

use super::types::{DIRECTIONS, NET_TYPES};
use super::{ANY_END, NAME_TOKENS, UNIT_END};
use crate::SyntaxKind::{self, *};
use crate::parser::{Parser, TokenSet};

/// Gate and switch primitives.
const GATES: TokenSet = TokenSet::new(&[
    AND_KW,
    NAND_KW,
    OR_KW,
    NOR_KW,
    XOR_KW,
    XNOR_KW,
    BUF_KW,
    NOT_KW,
    BUFIF0_KW,
    BUFIF1_KW,
    NOTIF0_KW,
    NOTIF1_KW,
    NMOS_KW,
    PMOS_KW,
    RNMOS_KW,
    RPMOS_KW,
    CMOS_KW,
    RCMOS_KW,
    TRAN_KW,
    RTRAN_KW,
    TRANIF0_KW,
    TRANIF1_KW,
    RTRANIF0_KW,
    RTRANIF1_KW,
    PULLUP_KW,
    PULLDOWN_KW,
]);

/// Keywords that start an item, for error recovery.
const ITEM_START: TokenSet = TokenSet::new(&[
    MODULE_KW,
    MACROMODULE_KW,
    INTERFACE_KW,
    PROGRAM_KW,
    PACKAGE_KW,
    INPUT_KW,
    OUTPUT_KW,
    INOUT_KW,
    PARAMETER_KW,
    LOCALPARAM_KW,
    TYPEDEF_KW,
    FUNCTION_KW,
    TASK_KW,
    ASSIGN_KW,
    ALWAYS_KW,
    ALWAYS_COMB_KW,
    ALWAYS_FF_KW,
    ALWAYS_LATCH_KW,
    INITIAL_KW,
    FINAL_KW,
    GENERATE_KW,
    WIRE_KW,
    REG_KW,
    LOGIC_KW,
    IMPORT_KW,
])
.union(ANY_END);

impl Parser<'_> {
    /// Any item: a design element, a declaration, a module item.
    pub(crate) fn item(&mut self) {
        self.nested(Self::item_inner);
    }

    fn item_inner(&mut self) {
        match self.current() {
            MODULE_KW | MACROMODULE_KW => self.module_like(MODULE_DECL, ENDMODULE_KW),
            INTERFACE_KW if self.nth_at(1, CLASS_KW) => {
                self.opaque_block(CLASS_DECL, INTERFACE_KW, ENDCLASS_KW, false)
            }
            INTERFACE_KW => self.module_like(INTERFACE_DECL, ENDINTERFACE_KW),
            PROGRAM_KW => self.module_like(PROGRAM_DECL, ENDPROGRAM_KW),
            PACKAGE_KW => self.package_decl(),
            PRIMITIVE_KW => self.opaque_block(PRIMITIVE_DECL, PRIMITIVE_KW, ENDPRIMITIVE_KW, false),
            CONFIG_KW => self.opaque_block(CONFIG_DECL, CONFIG_KW, ENDCONFIG_KW, false),
            CLASS_KW => self.opaque_block(CLASS_DECL, CLASS_KW, ENDCLASS_KW, true),
            VIRTUAL_KW if self.nth_at(1, CLASS_KW) => {
                self.opaque_block(CLASS_DECL, VIRTUAL_KW, ENDCLASS_KW, false)
            }
            CHECKER_KW => self.opaque_block(CHECKER_DECL, CHECKER_KW, ENDCHECKER_KW, false),
            EXTERN_KW => self.opaque_to_semi(EXTERN_DECL),
            IMPORT_KW | EXPORT_KW if self.nth_at(1, STRING_LITERAL) => {
                self.opaque_to_semi(DPI_DECL)
            }
            IMPORT_KW => self.package_import(),
            EXPORT_KW => self.package_export(),
            TIMEUNIT_KW | TIMEPRECISION_KW => self.opaque_to_semi(TIMEUNIT_DECL),
            CONST_KW if self.nth_at(1, REF_KW) => self.port_decl(),
            INPUT_KW | OUTPUT_KW | INOUT_KW | REF_KW => self.port_decl(),
            PARAMETER_KW | LOCALPARAM_KW | SPECPARAM_KW => self.param_decl_stmt(),
            TYPEDEF_KW => self.typedef_decl(),
            GENVAR_KW => self.genvar_decl(),
            FUNCTION_KW => self.function_decl(),
            TASK_KW => self.task_decl(),
            ASSIGN_KW => self.continuous_assign(),
            ALWAYS_KW | ALWAYS_COMB_KW | ALWAYS_FF_KW | ALWAYS_LATCH_KW | INITIAL_KW | FINAL_KW => {
                self.procedure()
            }
            GENERATE_KW => self.generate_region(),
            FOR_KW => self.generate_for(),
            IF_KW => self.generate_if(),
            CASE_KW | CASEX_KW | CASEZ_KW => self.generate_case(),
            BEGIN_KW => self.generate_block(),
            DEFPARAM_KW => self.defparam(),
            SPECIFY_KW => self.opaque_block(SPECIFY_BLOCK, SPECIFY_KW, ENDSPECIFY_KW, false),
            MODPORT_KW => self.modport_decl(),
            CLOCKING_KW => self.clocking(),
            DEFAULT_KW | GLOBAL_KW if self.nth_at(1, CLOCKING_KW) => self.clocking(),
            DEFAULT_KW if self.nth_at(1, DISABLE_KW) => self.opaque_to_semi(OPAQUE_DECL),
            PROPERTY_KW => self.opaque_block(OPAQUE_DECL, PROPERTY_KW, ENDPROPERTY_KW, false),
            SEQUENCE_KW => self.opaque_block(OPAQUE_DECL, SEQUENCE_KW, ENDSEQUENCE_KW, false),
            COVERGROUP_KW => self.opaque_block(OPAQUE_DECL, COVERGROUP_KW, ENDGROUP_KW, false),
            CONSTRAINT_KW => self.constraint(),
            LET_KW | NETTYPE_KW | ALIAS_KW | BIND_KW => self.opaque_to_semi(OPAQUE_DECL),
            ASSERT_KW | ASSUME_KW | COVER_KW | RESTRICT_KW | EXPECT_KW => self.assertion(),
            k if GATES.contains(k) => self.gate_instantiation(),
            k if NET_TYPES.contains(k) => self.net_decl(),
            SEMI => {
                self.start(NULL_STMT);
                self.bump();
                self.finish_node();
            }
            IDENT | ESC_IDENT if self.nth_at(1, COLON) => {
                self.start_labeled();
            }
            SYSTEM_IDENT if !self.nth_at(1, COLON2) => {
                // An elaboration task: `$error("...");`.
                self.statement();
            }
            // `` `MEM #(.W(8)) u_mem (...); ``
            MACRO_REF | MACRO_CALL if self.nth_at(1, HASH) => self.declaration_or_instantiation(),
            MACRO_CALL | MACRO_REF if !self.at_named_type_decl() => self.macro_item(),
            IDENT | ESC_IDENT | MACRO_CALL | MACRO_REF | SYSTEM_IDENT => {
                self.declaration_or_instantiation()
            }
            _ if self.at_declaration() => self.data_decl(DATA_DECL),
            _ => self.recover("item", ITEM_START),
        }
    }

    /// `name: item`: an assertion, a generate construct or a block.
    fn start_labeled(&mut self) {
        // The label belongs to the item that follows; wrap both.
        let checkpoint = self.checkpoint();
        self.opt_label();
        let kind = match self.current() {
            ASSERT_KW | ASSUME_KW | COVER_KW | RESTRICT_KW | EXPECT_KW => {
                self.assertion_body();
                ASSERTION_ITEM
            }
            FOR_KW => {
                self.generate_for_body();
                GENERATE_FOR
            }
            IF_KW => {
                self.generate_if_body();
                GENERATE_IF
            }
            CASE_KW | CASEX_KW | CASEZ_KW => {
                self.generate_case_body();
                GENERATE_CASE
            }
            BEGIN_KW => {
                self.generate_block_body();
                GENERATE_BLOCK
            }
            _ => {
                self.error_expected("item after label");
                ERROR
            }
        };
        self.start_at(checkpoint, kind);
        self.finish_node();
    }

    /// A macro where an item goes: `` `uvm_component_utils(c) ``,
    /// `` `ASSERT(name, a |-> b) ``.
    pub(crate) fn macro_item(&mut self) {
        self.start(MACRO_ITEM);
        self.bump_name_token();
        self.eat(SEMI);
        self.finish_node();
    }

    /// Items up to one of the `end` keywords.
    fn items_until_end(&mut self, end: SyntaxKind) {
        while !self.at_eof() && !self.at(end) && !self.at_set(UNIT_END) {
            let progress = self.progress();
            if self.at_set(ANY_END) {
                self.error_here(format!("unexpected `{}`", self.nth_text(0)));
                self.bump_error();
            } else {
                self.item();
            }
            if self.progress() == progress {
                self.bump_error();
            }
        }
    }

    /// Items of a generate block, up to `end` or another closing keyword.
    fn generate_items(&mut self) {
        while !self.at_eof() && !self.at_set(ANY_END) {
            let progress = self.progress();
            self.item();
            if self.progress() == progress {
                self.bump_error();
            }
        }
    }

    // ---- Design elements ---------------------------------------------------

    /// `module`, `interface` or `program`.
    fn module_like(&mut self, kind: SyntaxKind, end: SyntaxKind) {
        self.start(kind);
        self.bump();
        if self.at(STATIC_KW) || self.at(AUTOMATIC_KW) {
            self.bump();
        }
        self.name();
        while self.at(IMPORT_KW) {
            self.package_import();
        }
        if self.at(HASH) {
            self.param_port_list();
        }
        if self.at(LPAREN) {
            self.port_list();
        }
        self.expect(SEMI);
        self.items_until_end(end);
        self.expect(end);
        self.end_label();
        self.finish_node();
    }

    fn package_decl(&mut self) {
        self.start(PACKAGE_DECL);
        self.bump();
        if self.at(STATIC_KW) || self.at(AUTOMATIC_KW) {
            self.bump();
        }
        self.name();
        self.expect(SEMI);
        self.items_until_end(ENDPACKAGE_KW);
        self.expect(ENDPACKAGE_KW);
        self.end_label();
        self.finish_node();
    }

    /// `import p::*, q::x;`
    pub(crate) fn package_import(&mut self) {
        self.start(PACKAGE_IMPORT);
        self.bump();
        self.import_items();
        self.expect(SEMI);
        self.finish_node();
    }

    fn package_export(&mut self) {
        self.start(PACKAGE_EXPORT);
        self.bump();
        if self.at(STAR) && self.nth_at(1, COLON2) {
            // `export *::*;`
            self.start(IMPORT_ITEM);
            self.bump();
            self.bump();
            self.expect(STAR);
            self.finish_node();
        } else {
            self.import_items();
        }
        self.expect(SEMI);
        self.finish_node();
    }

    fn import_items(&mut self) {
        loop {
            self.start(IMPORT_ITEM);
            if self.at_set(NAME_TOKENS) {
                self.bump_name_token();
            } else {
                self.error_expected("package name");
            }
            if self.expect(COLON2) && !self.eat(STAR) {
                if self.at_set(NAME_TOKENS) {
                    self.bump_name_token();
                } else {
                    self.error_expected("identifier or `*`");
                }
            }
            self.finish_node();
            if !self.eat(COMMA) {
                break;
            }
        }
    }

    // ---- Parameters and ports ----------------------------------------------

    /// `#(parameter W = 8, type T = logic)`.
    fn param_port_list(&mut self) {
        self.start(PARAM_PORT_LIST);
        self.bump();
        if self.expect(LPAREN) {
            while !self.at_eof() && !self.at(RPAREN) {
                self.param_port();
                if !self.eat(COMMA) {
                    break;
                }
            }
            self.expect(RPAREN);
        }
        self.finish_node();
    }

    /// `(input logic a, output b)`, `(a, b)`, `(.a(x))`.
    pub(crate) fn port_list(&mut self) {
        self.start(PORT_LIST);
        self.bump();
        if !self.at(RPAREN) {
            loop {
                if self.at(RPAREN) {
                    // A trailing comma.
                    self.error_expected("port");
                    break;
                }
                self.port();
                if !self.eat(COMMA) {
                    break;
                }
            }
        }
        self.expect(RPAREN);
        self.finish_node();
    }

    fn port(&mut self) {
        self.start(PORT);
        if self.at(DOT) {
            // `.name(expr)`.
            self.bump();
            self.name();
            if self.expect(LPAREN) {
                if !self.at(RPAREN) {
                    self.expr();
                }
                self.expect(RPAREN);
            }
            self.finish_node();
            return;
        }
        if self.at(LBRACE) {
            // `{a, b}`: a port made of several nets.
            self.expr();
            self.finish_node();
            return;
        }
        if self.at(CONST_KW) && self.nth_at(1, REF_KW) {
            self.bump();
        }
        if self.at_set(DIRECTIONS) {
            self.bump();
        }
        if self.at(INTERFACE_KW) {
            self.start(DATA_TYPE);
            self.bump();
            if self.at(DOT) {
                self.bump();
                self.name();
            }
            self.finish_node();
        } else if self.at_set(NET_TYPES) || self.at(VAR_KW) || self.at_data_type() {
            self.data_type();
        }
        self.name();
        while self.at(LBRACKET) {
            self.dimension();
        }
        if self.eat(EQ) {
            self.expr_or_type();
        }
        self.finish_node();
    }

    // ---- Instantiations ----------------------------------------------------

    /// At a name in an item list: a declaration with a named type, or an
    /// instantiation.
    fn declaration_or_instantiation(&mut self) {
        let mut n = self.type_name_end(0).unwrap_or(1);
        // `mod #5 inst (...)`: a delay-like parameter value.
        if self.nth_at(n, HASH) && !self.nth_at(n + 1, LPAREN) {
            n += 2;
        }
        while self.nth_at(n, LBRACKET) {
            n = self.skip_group_ahead(n);
        }
        if !self.nth_is_name(n) {
            // `mod (...)` without an instance name is not valid Verilog;
            // parse it as an instantiation anyway when it looks like one.
            if self.nth_at(n, LPAREN) {
                self.instantiation();
            } else {
                self.recover("item", ITEM_START);
            }
            return;
        }
        let mut m = n + 1;
        if self.nth_at(n, MACRO_CALL) && self.nth_at(m, LPAREN) {
            m = self.skip_group_ahead(m);
        }
        while self.nth_at(m, LBRACKET) {
            m = self.skip_group_ahead(m);
        }
        if self.nth_at(m, LPAREN) {
            self.instantiation();
        } else {
            self.data_decl(DATA_DECL);
        }
    }

    /// `mod #(.W(8)) u0 (.a(x)), u1 (...);`
    fn instantiation(&mut self) {
        self.start(INSTANTIATION);
        self.start(NAME_REF);
        self.bump_name_token();
        while self.at(COLON2) && matches!(self.nth(1), IDENT | ESC_IDENT) {
            self.bump();
            self.bump();
        }
        self.finish_node();
        if self.at(HASH) {
            self.param_values();
        }
        self.instances();
        self.expect(SEMI);
        self.finish_node();
    }

    /// `and #1 g (y, a, b);`, `pullup (strong1) p (n);`
    fn gate_instantiation(&mut self) {
        self.start(INSTANTIATION);
        self.bump();
        if self.at(LPAREN)
            && matches!(
                self.nth(1),
                SUPPLY0_KW
                    | SUPPLY1_KW
                    | STRONG0_KW
                    | STRONG1_KW
                    | PULL0_KW
                    | PULL1_KW
                    | WEAK0_KW
                    | WEAK1_KW
                    | HIGHZ0_KW
                    | HIGHZ1_KW
            )
        {
            self.strength();
        }
        if self.at(HASH) {
            self.delay();
        }
        self.instances();
        self.expect(SEMI);
        self.finish_node();
    }

    fn instances(&mut self) {
        loop {
            self.start(INSTANCE);
            if self.at_set(NAME_TOKENS) {
                self.name();
                while self.at(LBRACKET) {
                    self.dimension();
                }
            }
            if self.at(LPAREN) {
                self.connections();
            } else {
                self.error_expected("`(`");
            }
            self.finish_node();
            if !self.eat(COMMA) {
                break;
            }
        }
    }

    /// `#(8, .W(4))`, `#8`.
    pub(crate) fn param_values(&mut self) {
        self.start(PARAM_VALUES);
        self.bump();
        if self.at(LPAREN) {
            self.connections();
        } else {
            self.start(CONNECTION);
            self.primary_expr();
            self.finish_node();
        }
        self.finish_node();
    }

    /// `(.a(x), .b, .*)` or `(x, , y)`.
    fn connections(&mut self) {
        self.bump();
        if self.at(RPAREN) {
            self.bump();
            return;
        }
        loop {
            self.start(CONNECTION);
            match self.current() {
                DOT_STAR => self.bump(),
                DOT => {
                    self.bump();
                    self.name();
                    if self.eat(LPAREN) {
                        if !self.at(RPAREN) {
                            self.expr_or_type();
                        }
                        self.expect(RPAREN);
                    }
                }
                // An unconnected positional port.
                COMMA | RPAREN => {}
                _ => self.expr_or_type(),
            }
            self.finish_node();
            if !self.eat(COMMA) {
                break;
            }
        }
        self.expect(RPAREN);
    }

    // ---- Module items --------------------------------------------------------

    /// `assign #1 a = b, c = d;`
    fn continuous_assign(&mut self) {
        self.start(CONTINUOUS_ASSIGN);
        self.bump();
        if self.at(LPAREN) {
            self.strength();
        }
        if self.at(HASH) {
            self.delay();
        }
        loop {
            self.assign_expr();
            if !self.eat(COMMA) {
                break;
            }
        }
        self.expect(SEMI);
        self.finish_node();
    }

    /// `always_ff @(posedge clk) ...`
    fn procedure(&mut self) {
        self.start(PROCEDURE);
        self.bump();
        self.statement();
        self.finish_node();
    }

    /// `defparam u0.W = 8;`
    fn defparam(&mut self) {
        self.start(DEFPARAM_DECL);
        self.bump();
        loop {
            self.assign_expr();
            if !self.eat(COMMA) {
                break;
            }
        }
        self.expect(SEMI);
        self.finish_node();
    }

    /// `modport mp (input a, output b, import f), mp2 (...);`
    fn modport_decl(&mut self) {
        self.start(MODPORT_DECL);
        self.bump();
        loop {
            self.start(MODPORT_ITEM);
            self.name();
            if self.expect(LPAREN) {
                while !self.at_eof() && !self.at(RPAREN) {
                    let progress = self.progress();
                    self.modport_port();
                    if !self.eat(COMMA) && self.progress() == progress {
                        break;
                    }
                }
                self.expect(RPAREN);
            }
            self.finish_node();
            if !self.eat(COMMA) {
                break;
            }
        }
        self.expect(SEMI);
        self.finish_node();
    }

    fn modport_port(&mut self) {
        self.start(PORT);
        match self.current() {
            INPUT_KW | OUTPUT_KW | INOUT_KW | REF_KW | CLOCKING_KW => self.bump(),
            IMPORT_KW | EXPORT_KW => {
                self.bump();
                if self.at(TASK_KW) || self.at(FUNCTION_KW) {
                    // A prototype: up to the next `,` or `)` outside brackets.
                    while !self.at_eof() && !self.at(COMMA) && !self.at(RPAREN) {
                        if self.at(LPAREN) {
                            self.balanced();
                        } else {
                            self.bump();
                        }
                    }
                    self.finish_node();
                    return;
                }
            }
            _ => {}
        }
        if self.at(DOT) {
            self.bump();
            self.name();
            if self.expect(LPAREN) {
                if !self.at(RPAREN) {
                    self.expr();
                }
                self.expect(RPAREN);
            }
        } else {
            self.name();
        }
        self.finish_node();
    }

    /// `clocking cb @(posedge clk); ... endclocking`, `default clocking cb;`.
    fn clocking(&mut self) {
        let mut n = 0;
        if matches!(self.current(), DEFAULT_KW | GLOBAL_KW) {
            n += 1;
        }
        n += 1;
        if self.nth_is_name(n) {
            n += 1;
        }
        if self.nth_at(n, SEMI) {
            self.opaque_to_semi(OPAQUE_DECL);
            return;
        }
        self.start(OPAQUE_DECL);
        while !self.at(CLOCKING_KW) && !self.at_eof() {
            self.bump();
        }
        while !self.at_eof() && !self.at(ENDCLOCKING_KW) && !self.at_set(UNIT_END) {
            self.bump();
        }
        self.expect(ENDCLOCKING_KW);
        self.end_label();
        self.finish_node();
    }

    /// `constraint c { ... }`, or a prototype `constraint c;`.
    fn constraint(&mut self) {
        self.start(OPAQUE_DECL);
        while !self.at_eof() && !self.at(LBRACE) && !self.at(SEMI) {
            self.bump();
        }
        if self.at(LBRACE) {
            self.balanced();
        } else {
            self.expect(SEMI);
        }
        self.finish_node();
    }

    /// A concurrent or deferred assertion in a module body.
    fn assertion(&mut self) {
        self.start(ASSERTION_ITEM);
        self.assertion_body();
        self.finish_node();
    }

    /// `assert property (...) else ...;`, `cover sequence (...)`,
    /// `assert final (...)`, `assert #0 (...)`.
    pub(crate) fn assertion_body(&mut self) {
        let expect = self.at(EXPECT_KW);
        self.bump();
        if expect || self.at(PROPERTY_KW) || self.at(SEQUENCE_KW) {
            if !expect {
                self.bump();
            }
            self.start(PROPERTY_EXPR);
            if self.at(LPAREN) {
                self.balanced();
            } else {
                self.error_expected("`(`");
            }
            self.finish_node();
        } else {
            if self.at(FINAL_KW) {
                self.bump();
            } else if self.at(HASH) {
                self.bump();
                self.expect(INT_NUMBER);
            }
            if self.expect(LPAREN) {
                self.expr();
                self.expect(RPAREN);
            }
        }
        self.action_block();
    }

    /// The statement after an assertion, and its `else` branch.
    pub(crate) fn action_block(&mut self) {
        if self.eat(SEMI) {
            return;
        }
        if !self.at(ELSE_KW) {
            self.statement();
        }
        if self.at(ELSE_KW) {
            self.start(ELSE_CLAUSE);
            self.bump();
            self.statement();
            self.finish_node();
        }
    }

    // ---- Generate constructs -------------------------------------------------

    fn generate_region(&mut self) {
        self.start(GENERATE_REGION);
        self.bump();
        while !self.at_eof() && !self.at(ENDGENERATE_KW) && !self.at_set(UNIT_END) {
            let progress = self.progress();
            if self.at_set(ANY_END) {
                self.error_here(format!("unexpected `{}`", self.nth_text(0)));
                self.bump_error();
            } else {
                self.item();
            }
            if self.progress() == progress {
                self.bump_error();
            }
        }
        self.expect(ENDGENERATE_KW);
        self.finish_node();
    }

    fn generate_for(&mut self) {
        self.start(GENERATE_FOR);
        self.generate_for_body();
        self.finish_node();
    }

    /// `for (genvar i = 0; i < N; i++) item`.
    fn generate_for_body(&mut self) {
        self.bump();
        if self.expect(LPAREN) {
            if self.at(GENVAR_KW) {
                self.bump();
            }
            self.assign_expr();
            self.expect(SEMI);
            self.expr();
            self.expect(SEMI);
            self.expr_with_assign();
            self.expect(RPAREN);
        }
        self.item();
    }

    fn generate_if(&mut self) {
        self.start(GENERATE_IF);
        self.generate_if_body();
        self.finish_node();
    }

    fn generate_if_body(&mut self) {
        self.bump();
        if self.expect(LPAREN) {
            self.expr();
            self.expect(RPAREN);
        }
        self.generate_item_or_null();
        if self.at(ELSE_KW) {
            self.start(ELSE_CLAUSE);
            self.bump();
            self.generate_item_or_null();
            self.finish_node();
        }
    }

    fn generate_item_or_null(&mut self) {
        if self.at(SEMI) {
            self.start(NULL_STMT);
            self.bump();
            self.finish_node();
        } else if self.at_set(ANY_END) {
            self.error_expected("item");
        } else {
            self.item();
        }
    }

    fn generate_case(&mut self) {
        self.start(GENERATE_CASE);
        self.generate_case_body();
        self.finish_node();
    }

    fn generate_case_body(&mut self) {
        self.bump();
        if self.expect(LPAREN) {
            self.expr();
            self.expect(RPAREN);
        }
        while !self.at_eof() && !self.at_set(ANY_END) {
            let progress = self.progress();
            self.start(CASE_ITEM);
            self.case_labels();
            self.generate_item_or_null();
            self.finish_node();
            if self.progress() == progress {
                self.bump_error();
            }
        }
        self.expect(ENDCASE_KW);
    }

    /// `expr, expr :` or `default [:]` before a case item.
    pub(crate) fn case_labels(&mut self) {
        if self.eat(DEFAULT_KW) {
            self.eat(COLON);
            return;
        }
        loop {
            if self.at(LBRACKET) {
                self.value_range();
            } else {
                self.expr();
            }
            if !self.eat(COMMA) {
                break;
            }
        }
        self.expect(COLON);
    }

    fn generate_block(&mut self) {
        self.start(GENERATE_BLOCK);
        self.generate_block_body();
        self.finish_node();
    }

    fn generate_block_body(&mut self) {
        self.bump();
        self.end_label();
        self.generate_items();
        self.expect(END_KW);
        self.end_label();
    }

    // ---- Functions and tasks -------------------------------------------------

    /// `function automatic logic [7:0] f(input int a); ... endfunction : f`
    fn function_decl(&mut self) {
        self.start(FUNCTION_DECL);
        self.bump();
        if self.at(STATIC_KW) || self.at(AUTOMATIC_KW) {
            self.bump();
        }
        if self.function_has_return_type() {
            self.data_type();
        }
        self.subroutine_name();
        self.subroutine_rest(ENDFUNCTION_KW);
        self.finish_node();
    }

    fn function_has_return_type(&self) -> bool {
        match self.current() {
            NEW_KW => false,
            IDENT | ESC_IDENT | MACRO_REF | MACRO_CALL | SYSTEM_IDENT => self.at_named_type_decl(),
            _ => true,
        }
    }

    fn task_decl(&mut self) {
        self.start(TASK_DECL);
        self.bump();
        if self.at(STATIC_KW) || self.at(AUTOMATIC_KW) {
            self.bump();
        }
        self.subroutine_name();
        self.subroutine_rest(ENDTASK_KW);
        self.finish_node();
    }

    /// `f`, or `c::f` outside its class.
    fn subroutine_name(&mut self) {
        self.start(NAME);
        if self.at_set(NAME_TOKENS) || self.at(NEW_KW) {
            self.bump_name_token();
            while self.at(COLON2) && matches!(self.nth(1), IDENT | ESC_IDENT | NEW_KW) {
                self.bump();
                self.bump();
            }
        } else {
            self.error_expected("identifier");
        }
        self.finish_node();
    }

    fn subroutine_rest(&mut self, end: SyntaxKind) {
        if self.at(LPAREN) {
            self.port_list();
        }
        self.expect(SEMI);
        while !self.at_eof() && !self.at_set(ANY_END) {
            let progress = self.progress();
            if self.at_set(DIRECTIONS) || (self.at(CONST_KW) && self.nth_at(1, REF_KW)) {
                self.port_decl();
            } else {
                self.block_item();
            }
            if self.progress() == progress {
                self.bump_error();
            }
        }
        self.expect(end);
        self.end_label();
    }
}

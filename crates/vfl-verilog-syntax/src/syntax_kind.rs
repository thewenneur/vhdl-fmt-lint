//! Every kind of token and node in the concrete syntax tree.

use crate::Standard;

macro_rules! syntax_kinds {
    (
        tokens { $($(#[$tok_attr:meta])* $tok:ident),* $(,)? }
        keywords { $($kw:ident = $text:literal @ $since:ident),* $(,)? }
        nodes { $($(#[$node_attr:meta])* $node:ident),* $(,)? }
    ) => {
        /// The kind of a token or node.
        ///
        /// Token kinds come first and all fit below 512, which lets
        /// [`TokenSet`](crate::parser::TokenSet) be a small bit set.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        #[repr(u16)]
        #[allow(non_camel_case_types, clippy::upper_case_acronyms)]
        pub enum SyntaxKind {
            $($(#[$tok_attr])* $tok,)*
            $($kw,)*
            $($(#[$node_attr])* $node,)*
        }

        impl SyntaxKind {
            const ALL: &[SyntaxKind] = &[
                $(SyntaxKind::$tok,)*
                $(SyntaxKind::$kw,)*
                $(SyntaxKind::$node,)*
            ];

            const KEYWORDS: &[(&str, SyntaxKind, Standard)] = &[
                $(($text, SyntaxKind::$kw, Standard::$since),)*
            ];

            fn keyword_entry(text: &str) -> Option<(SyntaxKind, Standard)> {
                match text {
                    $($text => Some((SyntaxKind::$kw, Standard::$since)),)*
                    _ => None,
                }
            }

            /// Inverse of `kind as u16`.
            pub fn from_raw(raw: u16) -> SyntaxKind {
                Self::ALL[raw as usize]
            }

            /// Whether this kind is a reserved word (in some standard).
            pub fn is_keyword(self) -> bool {
                matches!(self, $(SyntaxKind::$kw)|*)
            }

            /// Whether this kind is a node rather than a token.
            pub fn is_node(self) -> bool {
                matches!(self, $(SyntaxKind::$node)|*)
            }
        }
    };
}

syntax_kinds! {
    tokens {
        // Trivia.
        WHITESPACE,
        LINE_COMMENT,
        BLOCK_COMMENT,
        /// An attribute instance, `(* full_case *)`. Kept as trivia: it can
        /// appear almost anywhere and never changes how the code parses.
        ATTRIBUTE,
        /// A compiler directive that is not a macro use: `` `define ``,
        /// `` `include "f" ``, `` `ifdef X ``, `` `timescale 1ns/1ps ``, ...
        DIRECTIVE,
        /// Source text in a conditional branch that is not compiled
        /// (`` `ifdef `` of an undefined macro, ...).
        INACTIVE,

        // Identifiers and literals.
        IDENT,
        /// `\bus[0] `: an escaped identifier, up to the next white space.
        ESC_IDENT,
        /// `$display`, `$clog2`, ...
        SYSTEM_IDENT,
        /// `` `name `` of a macro that takes arguments, or that the file
        /// does not define.
        MACRO_CALL,
        /// `` `name `` of a macro defined without arguments, or
        /// `` `__FILE__ `` and `` `__LINE__ ``.
        MACRO_REF,
        /// `42`, `1_000`.
        INT_NUMBER,
        /// `8'hFF`, `'b1010`, `4 'sd 3`.
        BASED_NUMBER,
        /// `1.5`, `2e-3`.
        REAL_NUMBER,
        /// `10ns`, `1.5ps`, `1step`.
        TIME_LITERAL,
        /// `'0`, `'1`, `'x`, `'z`.
        UNBASED_UNSIZED,
        STRING_LITERAL,

        // Delimiters and operators.
        LPAREN,
        RPAREN,
        LBRACKET,
        RBRACKET,
        LBRACE,
        RBRACE,
        SEMI,
        COMMA,
        DOT,
        /// `.*` in port connections.
        DOT_STAR,
        COLON,
        COLON2,
        COLON_EQ,
        COLON_SLASH,
        QUESTION,
        HASH,
        HASH2,
        HASH_MINUS_HASH,
        HASH_EQ_HASH,
        AT,
        AT2,
        EQ,
        PLUS,
        MINUS,
        STAR,
        SLASH,
        PERCENT,
        STAR2,
        AMP,
        AMP2,
        AMP3,
        PIPE,
        PIPE2,
        CARET,
        TILDE,
        BANG,
        TILDE_AMP,
        TILDE_PIPE,
        /// `~^` or `^~`.
        TILDE_CARET,
        LT,
        GT,
        LTE,
        GTE,
        EQ2,
        NEQ,
        EQ3,
        NEQ2,
        /// `==?`
        EQ2_Q,
        /// `!=?`
        NEQ_Q,
        SHL,
        SHR,
        ASHL,
        ASHR,
        PLUS2,
        MINUS2,
        PLUS_EQ,
        MINUS_EQ,
        STAR_EQ,
        SLASH_EQ,
        PERCENT_EQ,
        AMP_EQ,
        PIPE_EQ,
        CARET_EQ,
        SHL_EQ,
        SHR_EQ,
        ASHL_EQ,
        ASHR_EQ,
        /// `->`
        ARROW,
        /// `->>`
        DARROW,
        /// `<->`
        EQUIV,
        /// `|->`
        PIPE_ARROW,
        /// `|=>`
        PIPE_DARROW,
        PLUS_COLON,
        MINUS_COLON,
        TICK,
        DOLLAR,
        /// A character that cannot start any token.
        ERROR_TOKEN,
        EOF,
    }
    keywords {
        // IEEE 1364-1995.
        ALWAYS_KW = "always" @ Verilog1995,
        AND_KW = "and" @ Verilog1995,
        ASSIGN_KW = "assign" @ Verilog1995,
        BEGIN_KW = "begin" @ Verilog1995,
        BUF_KW = "buf" @ Verilog1995,
        BUFIF0_KW = "bufif0" @ Verilog1995,
        BUFIF1_KW = "bufif1" @ Verilog1995,
        CASE_KW = "case" @ Verilog1995,
        CASEX_KW = "casex" @ Verilog1995,
        CASEZ_KW = "casez" @ Verilog1995,
        CMOS_KW = "cmos" @ Verilog1995,
        DEASSIGN_KW = "deassign" @ Verilog1995,
        DEFAULT_KW = "default" @ Verilog1995,
        DEFPARAM_KW = "defparam" @ Verilog1995,
        DISABLE_KW = "disable" @ Verilog1995,
        EDGE_KW = "edge" @ Verilog1995,
        ELSE_KW = "else" @ Verilog1995,
        END_KW = "end" @ Verilog1995,
        ENDCASE_KW = "endcase" @ Verilog1995,
        ENDFUNCTION_KW = "endfunction" @ Verilog1995,
        ENDMODULE_KW = "endmodule" @ Verilog1995,
        ENDPRIMITIVE_KW = "endprimitive" @ Verilog1995,
        ENDSPECIFY_KW = "endspecify" @ Verilog1995,
        ENDTABLE_KW = "endtable" @ Verilog1995,
        ENDTASK_KW = "endtask" @ Verilog1995,
        EVENT_KW = "event" @ Verilog1995,
        FOR_KW = "for" @ Verilog1995,
        FORCE_KW = "force" @ Verilog1995,
        FOREVER_KW = "forever" @ Verilog1995,
        FORK_KW = "fork" @ Verilog1995,
        FUNCTION_KW = "function" @ Verilog1995,
        HIGHZ0_KW = "highz0" @ Verilog1995,
        HIGHZ1_KW = "highz1" @ Verilog1995,
        IF_KW = "if" @ Verilog1995,
        IFNONE_KW = "ifnone" @ Verilog1995,
        INITIAL_KW = "initial" @ Verilog1995,
        INOUT_KW = "inout" @ Verilog1995,
        INPUT_KW = "input" @ Verilog1995,
        INTEGER_KW = "integer" @ Verilog1995,
        JOIN_KW = "join" @ Verilog1995,
        LARGE_KW = "large" @ Verilog1995,
        MACROMODULE_KW = "macromodule" @ Verilog1995,
        MEDIUM_KW = "medium" @ Verilog1995,
        MODULE_KW = "module" @ Verilog1995,
        NAND_KW = "nand" @ Verilog1995,
        NEGEDGE_KW = "negedge" @ Verilog1995,
        NMOS_KW = "nmos" @ Verilog1995,
        NOR_KW = "nor" @ Verilog1995,
        NOT_KW = "not" @ Verilog1995,
        NOTIF0_KW = "notif0" @ Verilog1995,
        NOTIF1_KW = "notif1" @ Verilog1995,
        OR_KW = "or" @ Verilog1995,
        OUTPUT_KW = "output" @ Verilog1995,
        PARAMETER_KW = "parameter" @ Verilog1995,
        PMOS_KW = "pmos" @ Verilog1995,
        POSEDGE_KW = "posedge" @ Verilog1995,
        PRIMITIVE_KW = "primitive" @ Verilog1995,
        PULL0_KW = "pull0" @ Verilog1995,
        PULL1_KW = "pull1" @ Verilog1995,
        PULLDOWN_KW = "pulldown" @ Verilog1995,
        PULLUP_KW = "pullup" @ Verilog1995,
        RCMOS_KW = "rcmos" @ Verilog1995,
        REAL_KW = "real" @ Verilog1995,
        REALTIME_KW = "realtime" @ Verilog1995,
        REG_KW = "reg" @ Verilog1995,
        RELEASE_KW = "release" @ Verilog1995,
        REPEAT_KW = "repeat" @ Verilog1995,
        RNMOS_KW = "rnmos" @ Verilog1995,
        RPMOS_KW = "rpmos" @ Verilog1995,
        RTRAN_KW = "rtran" @ Verilog1995,
        RTRANIF0_KW = "rtranif0" @ Verilog1995,
        RTRANIF1_KW = "rtranif1" @ Verilog1995,
        SCALARED_KW = "scalared" @ Verilog1995,
        SMALL_KW = "small" @ Verilog1995,
        SPECIFY_KW = "specify" @ Verilog1995,
        SPECPARAM_KW = "specparam" @ Verilog1995,
        STRONG0_KW = "strong0" @ Verilog1995,
        STRONG1_KW = "strong1" @ Verilog1995,
        SUPPLY0_KW = "supply0" @ Verilog1995,
        SUPPLY1_KW = "supply1" @ Verilog1995,
        TABLE_KW = "table" @ Verilog1995,
        TASK_KW = "task" @ Verilog1995,
        TIME_KW = "time" @ Verilog1995,
        TRAN_KW = "tran" @ Verilog1995,
        TRANIF0_KW = "tranif0" @ Verilog1995,
        TRANIF1_KW = "tranif1" @ Verilog1995,
        TRI_KW = "tri" @ Verilog1995,
        TRI0_KW = "tri0" @ Verilog1995,
        TRI1_KW = "tri1" @ Verilog1995,
        TRIAND_KW = "triand" @ Verilog1995,
        TRIOR_KW = "trior" @ Verilog1995,
        TRIREG_KW = "trireg" @ Verilog1995,
        VECTORED_KW = "vectored" @ Verilog1995,
        WAIT_KW = "wait" @ Verilog1995,
        WAND_KW = "wand" @ Verilog1995,
        WEAK0_KW = "weak0" @ Verilog1995,
        WEAK1_KW = "weak1" @ Verilog1995,
        WHILE_KW = "while" @ Verilog1995,
        WIRE_KW = "wire" @ Verilog1995,
        WOR_KW = "wor" @ Verilog1995,
        XNOR_KW = "xnor" @ Verilog1995,
        XOR_KW = "xor" @ Verilog1995,
        // IEEE 1364-2001.
        AUTOMATIC_KW = "automatic" @ Verilog2001,
        CELL_KW = "cell" @ Verilog2001,
        CONFIG_KW = "config" @ Verilog2001,
        DESIGN_KW = "design" @ Verilog2001,
        ENDCONFIG_KW = "endconfig" @ Verilog2001,
        ENDGENERATE_KW = "endgenerate" @ Verilog2001,
        GENERATE_KW = "generate" @ Verilog2001,
        GENVAR_KW = "genvar" @ Verilog2001,
        INCDIR_KW = "incdir" @ Verilog2001,
        INCLUDE_KW = "include" @ Verilog2001,
        INSTANCE_KW = "instance" @ Verilog2001,
        LIBLIST_KW = "liblist" @ Verilog2001,
        LIBRARY_KW = "library" @ Verilog2001,
        LOCALPARAM_KW = "localparam" @ Verilog2001,
        NOSHOWCANCELLED_KW = "noshowcancelled" @ Verilog2001,
        PULSESTYLE_ONDETECT_KW = "pulsestyle_ondetect" @ Verilog2001,
        PULSESTYLE_ONEVENT_KW = "pulsestyle_onevent" @ Verilog2001,
        SHOWCANCELLED_KW = "showcancelled" @ Verilog2001,
        SIGNED_KW = "signed" @ Verilog2001,
        UNSIGNED_KW = "unsigned" @ Verilog2001,
        USE_KW = "use" @ Verilog2001,
        // IEEE 1364-2005.
        UWIRE_KW = "uwire" @ Verilog2005,
        // IEEE 1800-2005.
        ALIAS_KW = "alias" @ Sv2005,
        ALWAYS_COMB_KW = "always_comb" @ Sv2005,
        ALWAYS_FF_KW = "always_ff" @ Sv2005,
        ALWAYS_LATCH_KW = "always_latch" @ Sv2005,
        ASSERT_KW = "assert" @ Sv2005,
        ASSUME_KW = "assume" @ Sv2005,
        BEFORE_KW = "before" @ Sv2005,
        BIND_KW = "bind" @ Sv2005,
        BINS_KW = "bins" @ Sv2005,
        BINSOF_KW = "binsof" @ Sv2005,
        BIT_KW = "bit" @ Sv2005,
        BREAK_KW = "break" @ Sv2005,
        BYTE_KW = "byte" @ Sv2005,
        CHANDLE_KW = "chandle" @ Sv2005,
        CLASS_KW = "class" @ Sv2005,
        CLOCKING_KW = "clocking" @ Sv2005,
        CONST_KW = "const" @ Sv2005,
        CONSTRAINT_KW = "constraint" @ Sv2005,
        CONTEXT_KW = "context" @ Sv2005,
        CONTINUE_KW = "continue" @ Sv2005,
        COVER_KW = "cover" @ Sv2005,
        COVERGROUP_KW = "covergroup" @ Sv2005,
        COVERPOINT_KW = "coverpoint" @ Sv2005,
        CROSS_KW = "cross" @ Sv2005,
        DIST_KW = "dist" @ Sv2005,
        DO_KW = "do" @ Sv2005,
        ENDCLASS_KW = "endclass" @ Sv2005,
        ENDCLOCKING_KW = "endclocking" @ Sv2005,
        ENDGROUP_KW = "endgroup" @ Sv2005,
        ENDINTERFACE_KW = "endinterface" @ Sv2005,
        ENDPACKAGE_KW = "endpackage" @ Sv2005,
        ENDPROGRAM_KW = "endprogram" @ Sv2005,
        ENDPROPERTY_KW = "endproperty" @ Sv2005,
        ENDSEQUENCE_KW = "endsequence" @ Sv2005,
        ENUM_KW = "enum" @ Sv2005,
        EXPECT_KW = "expect" @ Sv2005,
        EXPORT_KW = "export" @ Sv2005,
        EXTENDS_KW = "extends" @ Sv2005,
        EXTERN_KW = "extern" @ Sv2005,
        FINAL_KW = "final" @ Sv2005,
        FIRST_MATCH_KW = "first_match" @ Sv2005,
        FOREACH_KW = "foreach" @ Sv2005,
        FORKJOIN_KW = "forkjoin" @ Sv2005,
        IFF_KW = "iff" @ Sv2005,
        IGNORE_BINS_KW = "ignore_bins" @ Sv2005,
        ILLEGAL_BINS_KW = "illegal_bins" @ Sv2005,
        IMPORT_KW = "import" @ Sv2005,
        INSIDE_KW = "inside" @ Sv2005,
        INT_KW = "int" @ Sv2005,
        INTERFACE_KW = "interface" @ Sv2005,
        INTERSECT_KW = "intersect" @ Sv2005,
        JOIN_ANY_KW = "join_any" @ Sv2005,
        JOIN_NONE_KW = "join_none" @ Sv2005,
        LOCAL_KW = "local" @ Sv2005,
        LOGIC_KW = "logic" @ Sv2005,
        LONGINT_KW = "longint" @ Sv2005,
        MATCHES_KW = "matches" @ Sv2005,
        MODPORT_KW = "modport" @ Sv2005,
        NEW_KW = "new" @ Sv2005,
        NULL_KW = "null" @ Sv2005,
        PACKAGE_KW = "package" @ Sv2005,
        PACKED_KW = "packed" @ Sv2005,
        PRIORITY_KW = "priority" @ Sv2005,
        PROGRAM_KW = "program" @ Sv2005,
        PROPERTY_KW = "property" @ Sv2005,
        PROTECTED_KW = "protected" @ Sv2005,
        PURE_KW = "pure" @ Sv2005,
        RAND_KW = "rand" @ Sv2005,
        RANDC_KW = "randc" @ Sv2005,
        RANDCASE_KW = "randcase" @ Sv2005,
        RANDSEQUENCE_KW = "randsequence" @ Sv2005,
        REF_KW = "ref" @ Sv2005,
        RETURN_KW = "return" @ Sv2005,
        SEQUENCE_KW = "sequence" @ Sv2005,
        SHORTINT_KW = "shortint" @ Sv2005,
        SHORTREAL_KW = "shortreal" @ Sv2005,
        SOLVE_KW = "solve" @ Sv2005,
        STATIC_KW = "static" @ Sv2005,
        STRING_KW = "string" @ Sv2005,
        STRUCT_KW = "struct" @ Sv2005,
        SUPER_KW = "super" @ Sv2005,
        TAGGED_KW = "tagged" @ Sv2005,
        THIS_KW = "this" @ Sv2005,
        THROUGHOUT_KW = "throughout" @ Sv2005,
        TIMEPRECISION_KW = "timeprecision" @ Sv2005,
        TIMEUNIT_KW = "timeunit" @ Sv2005,
        TYPE_KW = "type" @ Sv2005,
        TYPEDEF_KW = "typedef" @ Sv2005,
        UNION_KW = "union" @ Sv2005,
        UNIQUE_KW = "unique" @ Sv2005,
        VAR_KW = "var" @ Sv2005,
        VIRTUAL_KW = "virtual" @ Sv2005,
        VOID_KW = "void" @ Sv2005,
        WAIT_ORDER_KW = "wait_order" @ Sv2005,
        WILDCARD_KW = "wildcard" @ Sv2005,
        WITH_KW = "with" @ Sv2005,
        WITHIN_KW = "within" @ Sv2005,
        // IEEE 1800-2009.
        ACCEPT_ON_KW = "accept_on" @ Sv2009,
        CHECKER_KW = "checker" @ Sv2009,
        ENDCHECKER_KW = "endchecker" @ Sv2009,
        EVENTUALLY_KW = "eventually" @ Sv2009,
        GLOBAL_KW = "global" @ Sv2009,
        IMPLIES_KW = "implies" @ Sv2009,
        LET_KW = "let" @ Sv2009,
        NEXTTIME_KW = "nexttime" @ Sv2009,
        REJECT_ON_KW = "reject_on" @ Sv2009,
        RESTRICT_KW = "restrict" @ Sv2009,
        S_ALWAYS_KW = "s_always" @ Sv2009,
        S_EVENTUALLY_KW = "s_eventually" @ Sv2009,
        S_NEXTTIME_KW = "s_nexttime" @ Sv2009,
        S_UNTIL_KW = "s_until" @ Sv2009,
        S_UNTIL_WITH_KW = "s_until_with" @ Sv2009,
        STRONG_KW = "strong" @ Sv2009,
        SYNC_ACCEPT_ON_KW = "sync_accept_on" @ Sv2009,
        SYNC_REJECT_ON_KW = "sync_reject_on" @ Sv2009,
        UNIQUE0_KW = "unique0" @ Sv2009,
        UNTIL_KW = "until" @ Sv2009,
        UNTIL_WITH_KW = "until_with" @ Sv2009,
        UNTYPED_KW = "untyped" @ Sv2009,
        WEAK_KW = "weak" @ Sv2009,
        // IEEE 1800-2012.
        IMPLEMENTS_KW = "implements" @ Sv2012,
        INTERCONNECT_KW = "interconnect" @ Sv2012,
        NETTYPE_KW = "nettype" @ Sv2012,
        SOFT_KW = "soft" @ Sv2012,
    }
    nodes {
        SOURCE_FILE,
        /// Tokens that could not be parsed.
        ERROR,

        // Design elements.
        /// `module` or `macromodule`.
        MODULE_DECL,
        INTERFACE_DECL,
        PROGRAM_DECL,
        PACKAGE_DECL,
        /// A user-defined primitive. Its table is kept as an opaque run of tokens.
        PRIMITIVE_DECL,
        /// A library configuration, kept as an opaque run of tokens.
        CONFIG_DECL,
        /// A class, kept as an opaque run of tokens.
        CLASS_DECL,
        /// A checker, kept as an opaque run of tokens.
        CHECKER_DECL,
        /// `extern module ...;` and other prototypes.
        EXTERN_DECL,
        /// A declared name.
        NAME,
        /// `: name` after an `end` keyword.
        END_LABEL,
        /// `name :` before a statement, a block or an item.
        LABEL,
        /// `import p::*, q::x;`
        PACKAGE_IMPORT,
        IMPORT_ITEM,
        /// `export p::*;`
        PACKAGE_EXPORT,
        /// `import "DPI-C" function ...;` and `export "DPI-C" ...;`
        DPI_DECL,
        /// `timeunit 1ns;`, `timeprecision 1ps;`
        TIMEUNIT_DECL,

        // Parameters and ports.
        /// `#(...)` after a module name.
        PARAM_PORT_LIST,
        /// `parameter`, `localparam` or `specparam` declaration, or a
        /// parameter in a `#(...)` list.
        PARAM_DECL,
        /// The port list of a module, interface, program, function or task.
        PORT_LIST,
        /// A port of a port list: a declaration (`input logic [7:0] a`),
        /// a bare name, or `.name(expr)`.
        PORT,
        /// A port declaration in a module body: `input [7:0] a, b;`
        PORT_DECL,

        // Types.
        /// A data type, a net type or both, or only a signing and
        /// dimensions (an implicit type).
        DATA_TYPE,
        /// `struct packed { ... }` or `union { ... }`.
        STRUCT_TYPE,
        STRUCT_MEMBER,
        ENUM_TYPE,
        ENUM_MEMBER,
        /// `type(expr)`.
        TYPE_REFERENCE,
        /// `[msb:lsb]`, `[N]`, `[]`, `[$]`, `[*]`, `[int]`.
        DIMENSION,
        /// `#delay` in a declaration, assignment or instance.
        DELAY,
        /// `(strong0, weak1)` and the like.
        STRENGTH,

        // Declarations.
        NET_DECL,
        /// A variable declaration.
        DATA_DECL,
        /// A name being declared, with its unpacked dimensions and initial value.
        DECLARATOR,
        TYPEDEF_DECL,
        GENVAR_DECL,
        FUNCTION_DECL,
        TASK_DECL,
        MODPORT_DECL,
        MODPORT_ITEM,
        DEFPARAM_DECL,
        /// `let`, `nettype`, `alias`, `bind`, covergroups, properties,
        /// sequences, clocking blocks, constraints: kept as opaque runs of tokens.
        OPAQUE_DECL,
        /// `specify ... endspecify`, kept as an opaque run of tokens.
        SPECIFY_BLOCK,
        /// A macro used where an item or a statement goes.
        MACRO_ITEM,
        /// `(...)` after a macro name: the arguments, unparsed.
        MACRO_ARGS,

        // Module items.
        CONTINUOUS_ASSIGN,
        /// `always`, `always_comb`, `always_ff`, `always_latch`, `initial`, `final`.
        PROCEDURE,
        /// A module, interface, program or primitive instantiation.
        INSTANTIATION,
        /// `#(...)` of an instantiation.
        PARAM_VALUES,
        INSTANCE,
        /// A port or parameter connection: `.name(expr)`, `.name`, `.*` or `expr`.
        CONNECTION,
        /// `generate ... endgenerate`.
        GENERATE_REGION,
        GENERATE_FOR,
        GENERATE_IF,
        GENERATE_CASE,
        /// `begin [: name] ... end` around module items.
        GENERATE_BLOCK,
        /// A concurrent assertion: `assert property (...)`, `cover property`, ...
        ASSERTION_ITEM,

        // Statements.
        /// `begin ... end`.
        BLOCK_STMT,
        /// `fork ... join`.
        FORK_STMT,
        IF_STMT,
        ELSE_CLAUSE,
        CASE_STMT,
        CASE_ITEM,
        FOR_STMT,
        FOREACH_STMT,
        WHILE_STMT,
        DO_WHILE_STMT,
        REPEAT_STMT,
        FOREVER_STMT,
        /// `a = b;`
        BLOCKING_ASSIGN,
        /// `a <= b;`
        NONBLOCKING_ASSIGN,
        /// `a += b;`
        COMPOUND_ASSIGN,
        /// A call, `i++` or another expression used as a statement.
        EXPR_STMT,
        /// `@(...) stmt`, `#10 stmt`, `##1 stmt`.
        TIMED_STMT,
        EVENT_CONTROL,
        /// `posedge clk iff en`, one alternative of an event control.
        EVENT_EXPR,
        WAIT_STMT,
        DISABLE_STMT,
        /// `-> ev;`
        EVENT_TRIGGER,
        RETURN_STMT,
        BREAK_STMT,
        CONTINUE_STMT,
        NULL_STMT,
        /// `assign`, `deassign`, `force` or `release` in procedural code.
        PROCEDURAL_CONTINUOUS,
        /// `assert (expr) ... else ...`.
        IMMEDIATE_ASSERT,
        /// A data declaration inside a block.
        LOCAL_DECL,
        /// `randcase`, `randsequence`, `expect`, `wait_order`: opaque.
        OPAQUE_STMT,

        // Expressions.
        BINARY_EXPR,
        UNARY_EXPR,
        /// `i++`, `i--`.
        POSTFIX_EXPR,
        CONDITIONAL_EXPR,
        PAREN_EXPR,
        /// `a:b:c`, a minimum, typical and maximum value.
        MINTYPMAX_EXPR,
        /// A literal: number, string, `null`, `$`.
        LITERAL,
        /// A name, scoped (`pkg::x`, `$unit::x`) or not.
        NAME_REF,
        /// `a.b`.
        FIELD_EXPR,
        /// `a[i]`, `a[h:l]`, `a[i+:w]`.
        INDEX_EXPR,
        CALL_EXPR,
        ARG_LIST,
        /// `.name(expr)` in a call.
        NAMED_ARG,
        /// `{a, b}`.
        CONCAT_EXPR,
        /// `{n{a}}`.
        REPLICATION_EXPR,
        /// `{<< 8 {a}}`.
        STREAMING_EXPR,
        /// `'{a, b}`, `'{default: 0}`, `t'{...}`.
        ASSIGNMENT_PATTERN,
        PATTERN_ITEM,
        /// `t'(x)`, `8'(x)`, `signed'(x)`.
        CAST_EXPR,
        /// `a inside {...}`.
        INSIDE_EXPR,
        /// `{...}` after `inside`, or a `case ... inside` item.
        RANGE_LIST,
        /// `[a:b]` in a range list.
        VALUE_RANGE,
        /// A macro used in an expression.
        MACRO_EXPR,
        /// `with (...)` after an array method call.
        WITH_CLAUSE,
        /// An assignment in an expression: `(a = b)`, `i = 0` in a `for` header.
        ASSIGN_EXPR,
        /// `new`, `new(...)`, `new [n]`.
        NEW_EXPR,
        /// `tagged name [expr]`.
        TAGGED_EXPR,
        /// A property or sequence expression, kept as an opaque run of tokens.
        PROPERTY_EXPR,
        /// `x matches pattern`.
        MATCHES_EXPR,
        /// The pattern after `matches`, kept as an opaque run of tokens.
        PATTERN,
        /// A type where an expression goes: `$bits(logic [7:0])`, a type
        /// parameter value.
        TYPE_EXPR,
    }
}

impl SyntaxKind {
    /// White space, comments, attributes and directives.
    pub fn is_trivia(self) -> bool {
        matches!(
            self,
            SyntaxKind::WHITESPACE
                | SyntaxKind::LINE_COMMENT
                | SyntaxKind::BLOCK_COMMENT
                | SyntaxKind::ATTRIBUTE
                | SyntaxKind::DIRECTIVE
                | SyntaxKind::INACTIVE
        )
    }

    /// The reserved word `text` as of `std`, if it is one. Verilog keywords
    /// are case-sensitive.
    pub fn keyword(text: &str, std: Standard) -> Option<SyntaxKind> {
        Self::keyword_entry(text)
            .filter(|&(_, since)| since <= std)
            .map(|(kind, _)| kind)
    }

    /// The source spelling of a keyword or delimiter, for messages.
    pub fn text(self) -> Option<&'static str> {
        use SyntaxKind::*;
        if let Some((text, _, _)) = Self::KEYWORDS.iter().find(|(_, kind, _)| *kind == self) {
            return Some(text);
        }
        Some(match self {
            LPAREN => "(",
            RPAREN => ")",
            LBRACKET => "[",
            RBRACKET => "]",
            LBRACE => "{",
            RBRACE => "}",
            SEMI => ";",
            COMMA => ",",
            DOT => ".",
            DOT_STAR => ".*",
            COLON => ":",
            COLON2 => "::",
            COLON_EQ => ":=",
            COLON_SLASH => ":/",
            QUESTION => "?",
            HASH => "#",
            HASH2 => "##",
            HASH_MINUS_HASH => "#-#",
            HASH_EQ_HASH => "#=#",
            AT => "@",
            AT2 => "@@",
            EQ => "=",
            PLUS => "+",
            MINUS => "-",
            STAR => "*",
            SLASH => "/",
            PERCENT => "%",
            STAR2 => "**",
            AMP => "&",
            AMP2 => "&&",
            AMP3 => "&&&",
            PIPE => "|",
            PIPE2 => "||",
            CARET => "^",
            TILDE => "~",
            BANG => "!",
            TILDE_AMP => "~&",
            TILDE_PIPE => "~|",
            TILDE_CARET => "~^",
            LT => "<",
            GT => ">",
            LTE => "<=",
            GTE => ">=",
            EQ2 => "==",
            NEQ => "!=",
            EQ3 => "===",
            NEQ2 => "!==",
            EQ2_Q => "==?",
            NEQ_Q => "!=?",
            SHL => "<<",
            SHR => ">>",
            ASHL => "<<<",
            ASHR => ">>>",
            PLUS2 => "++",
            MINUS2 => "--",
            PLUS_EQ => "+=",
            MINUS_EQ => "-=",
            STAR_EQ => "*=",
            SLASH_EQ => "/=",
            PERCENT_EQ => "%=",
            AMP_EQ => "&=",
            PIPE_EQ => "|=",
            CARET_EQ => "^=",
            SHL_EQ => "<<=",
            SHR_EQ => ">>=",
            ASHL_EQ => "<<<=",
            ASHR_EQ => ">>>=",
            ARROW => "->",
            DARROW => "->>",
            EQUIV => "<->",
            PIPE_ARROW => "|->",
            PIPE_DARROW => "|=>",
            PLUS_COLON => "+:",
            MINUS_COLON => "-:",
            TICK => "'",
            DOLLAR => "$",
            _ => return None,
        })
    }

    /// A human-readable description, for "expected ..." messages.
    pub fn describe(self) -> String {
        use SyntaxKind::*;
        match self {
            IDENT | ESC_IDENT => "identifier".into(),
            STRING_LITERAL => "string literal".into(),
            EOF => "end of file".into(),
            _ => match self.text() {
                Some(text) => format!("`{text}`"),
                None => format!("{self:?}").to_lowercase().replace('_', " "),
            },
        }
    }
}

impl From<SyntaxKind> for rowan::SyntaxKind {
    fn from(kind: SyntaxKind) -> Self {
        rowan::SyntaxKind(kind as u16)
    }
}

/// The rowan language tag for Verilog and SystemVerilog.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Verilog {}

impl rowan::Language for Verilog {
    type Kind = SyntaxKind;

    fn kind_from_raw(raw: rowan::SyntaxKind) -> SyntaxKind {
        SyntaxKind::from_raw(raw.0)
    }

    fn kind_to_raw(kind: SyntaxKind) -> rowan::SyntaxKind {
        kind.into()
    }
}

pub type SyntaxNode = rowan::SyntaxNode<Verilog>;
pub type SyntaxToken = rowan::SyntaxToken<Verilog>;
pub type SyntaxElement = rowan::SyntaxElement<Verilog>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_kinds_fit_in_a_token_set() {
        assert!((SyntaxKind::SOFT_KW as u16) < 512);
    }

    #[test]
    fn raw_round_trip() {
        for &kind in SyntaxKind::ALL {
            assert_eq!(SyntaxKind::from_raw(kind as u16), kind);
        }
    }

    #[test]
    fn keywords_depend_on_standard() {
        assert_eq!(SyntaxKind::keyword("logic", Standard::Verilog2005), None);
        assert_eq!(
            SyntaxKind::keyword("logic", Standard::Sv2017),
            Some(SyntaxKind::LOGIC_KW)
        );
        assert_eq!(SyntaxKind::keyword("generate", Standard::Verilog1995), None);
        assert_eq!(SyntaxKind::keyword("Module", Standard::Sv2017), None);
    }
}

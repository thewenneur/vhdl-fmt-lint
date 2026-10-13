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
        /// Token kinds come first and all fit below 256, which lets
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
        /// A VHDL-2019 tool directive, such as `` `warning "x" ``, up to the end of its line.
        TOOL_DIRECTIVE,

        // Identifiers and literals.
        IDENT,
        EXT_IDENT,
        DECIMAL_LITERAL,
        BASED_LITERAL,
        CHAR_LITERAL,
        STRING_LITERAL,
        BIT_STRING_LITERAL,

        // Delimiters.
        AMP,
        TICK,
        LPAREN,
        RPAREN,
        STAR,
        STAR2,
        PLUS,
        COMMA,
        MINUS,
        DOT,
        SLASH,
        COLON,
        SEMI,
        LT,
        LTE,
        BOX,
        EQ,
        NEQ,
        GT,
        GTE,
        ARROW,
        COLON_EQ,
        BAR,
        LBRACKET,
        RBRACKET,
        LBRACE,
        RBRACE,
        QQ,
        QEQ,
        QNEQ,
        QLT,
        QLTE,
        QGT,
        QGTE,
        /// A lone `?`, as in `case?` and `select?`.
        QUESTION,
        LTLT,
        GTGT,
        CARET,
        AT,
        /// A character that cannot start any token.
        ERROR_TOKEN,
        EOF,
    }
    keywords {
        ABS_KW = "abs" @ Vhdl1993,
        ACCESS_KW = "access" @ Vhdl1993,
        AFTER_KW = "after" @ Vhdl1993,
        ALIAS_KW = "alias" @ Vhdl1993,
        ALL_KW = "all" @ Vhdl1993,
        AND_KW = "and" @ Vhdl1993,
        ARCHITECTURE_KW = "architecture" @ Vhdl1993,
        ARRAY_KW = "array" @ Vhdl1993,
        ASSERT_KW = "assert" @ Vhdl1993,
        ATTRIBUTE_KW = "attribute" @ Vhdl1993,
        BEGIN_KW = "begin" @ Vhdl1993,
        BLOCK_KW = "block" @ Vhdl1993,
        BODY_KW = "body" @ Vhdl1993,
        BUFFER_KW = "buffer" @ Vhdl1993,
        BUS_KW = "bus" @ Vhdl1993,
        CASE_KW = "case" @ Vhdl1993,
        COMPONENT_KW = "component" @ Vhdl1993,
        CONFIGURATION_KW = "configuration" @ Vhdl1993,
        CONSTANT_KW = "constant" @ Vhdl1993,
        CONTEXT_KW = "context" @ Vhdl2008,
        DISCONNECT_KW = "disconnect" @ Vhdl1993,
        DOWNTO_KW = "downto" @ Vhdl1993,
        ELSE_KW = "else" @ Vhdl1993,
        ELSIF_KW = "elsif" @ Vhdl1993,
        END_KW = "end" @ Vhdl1993,
        ENTITY_KW = "entity" @ Vhdl1993,
        EXIT_KW = "exit" @ Vhdl1993,
        FILE_KW = "file" @ Vhdl1993,
        FOR_KW = "for" @ Vhdl1993,
        FORCE_KW = "force" @ Vhdl2008,
        FUNCTION_KW = "function" @ Vhdl1993,
        GENERATE_KW = "generate" @ Vhdl1993,
        GENERIC_KW = "generic" @ Vhdl1993,
        GROUP_KW = "group" @ Vhdl1993,
        GUARDED_KW = "guarded" @ Vhdl1993,
        IF_KW = "if" @ Vhdl1993,
        IMPURE_KW = "impure" @ Vhdl1993,
        IN_KW = "in" @ Vhdl1993,
        INERTIAL_KW = "inertial" @ Vhdl1993,
        INOUT_KW = "inout" @ Vhdl1993,
        IS_KW = "is" @ Vhdl1993,
        LABEL_KW = "label" @ Vhdl1993,
        LIBRARY_KW = "library" @ Vhdl1993,
        LINKAGE_KW = "linkage" @ Vhdl1993,
        LITERAL_KW = "literal" @ Vhdl1993,
        LOOP_KW = "loop" @ Vhdl1993,
        MAP_KW = "map" @ Vhdl1993,
        MOD_KW = "mod" @ Vhdl1993,
        NAND_KW = "nand" @ Vhdl1993,
        NEW_KW = "new" @ Vhdl1993,
        NEXT_KW = "next" @ Vhdl1993,
        NOR_KW = "nor" @ Vhdl1993,
        NOT_KW = "not" @ Vhdl1993,
        NULL_KW = "null" @ Vhdl1993,
        OF_KW = "of" @ Vhdl1993,
        ON_KW = "on" @ Vhdl1993,
        OPEN_KW = "open" @ Vhdl1993,
        OR_KW = "or" @ Vhdl1993,
        OTHERS_KW = "others" @ Vhdl1993,
        OUT_KW = "out" @ Vhdl1993,
        PACKAGE_KW = "package" @ Vhdl1993,
        PARAMETER_KW = "parameter" @ Vhdl2008,
        PORT_KW = "port" @ Vhdl1993,
        POSTPONED_KW = "postponed" @ Vhdl1993,
        PRIVATE_KW = "private" @ Vhdl2019,
        PROCEDURE_KW = "procedure" @ Vhdl1993,
        PROCESS_KW = "process" @ Vhdl1993,
        // Reserved since VHDL-2002, which is folded into "93" here.
        PROTECTED_KW = "protected" @ Vhdl1993,
        PURE_KW = "pure" @ Vhdl1993,
        RANGE_KW = "range" @ Vhdl1993,
        RECORD_KW = "record" @ Vhdl1993,
        REGISTER_KW = "register" @ Vhdl1993,
        REJECT_KW = "reject" @ Vhdl1993,
        RELEASE_KW = "release" @ Vhdl2008,
        REM_KW = "rem" @ Vhdl1993,
        REPORT_KW = "report" @ Vhdl1993,
        RETURN_KW = "return" @ Vhdl1993,
        ROL_KW = "rol" @ Vhdl1993,
        ROR_KW = "ror" @ Vhdl1993,
        SELECT_KW = "select" @ Vhdl1993,
        SEVERITY_KW = "severity" @ Vhdl1993,
        SHARED_KW = "shared" @ Vhdl1993,
        SIGNAL_KW = "signal" @ Vhdl1993,
        SLA_KW = "sla" @ Vhdl1993,
        SLL_KW = "sll" @ Vhdl1993,
        SRA_KW = "sra" @ Vhdl1993,
        SRL_KW = "srl" @ Vhdl1993,
        SUBTYPE_KW = "subtype" @ Vhdl1993,
        THEN_KW = "then" @ Vhdl1993,
        TO_KW = "to" @ Vhdl1993,
        TRANSPORT_KW = "transport" @ Vhdl1993,
        TYPE_KW = "type" @ Vhdl1993,
        UNAFFECTED_KW = "unaffected" @ Vhdl1993,
        UNITS_KW = "units" @ Vhdl1993,
        UNTIL_KW = "until" @ Vhdl1993,
        USE_KW = "use" @ Vhdl1993,
        VARIABLE_KW = "variable" @ Vhdl1993,
        VIEW_KW = "view" @ Vhdl2019,
        WAIT_KW = "wait" @ Vhdl1993,
        WHEN_KW = "when" @ Vhdl1993,
        WHILE_KW = "while" @ Vhdl1993,
        WITH_KW = "with" @ Vhdl1993,
        XNOR_KW = "xnor" @ Vhdl1993,
        XOR_KW = "xor" @ Vhdl1993,
    }
    nodes {
        /// A token sequence the parser could not make sense of.
        ERROR,
        SOURCE_FILE,

        // Design units and context items.
        DESIGN_UNIT,
        CONTEXT_CLAUSE,
        LIBRARY_CLAUSE,
        USE_CLAUSE,
        CONTEXT_REFERENCE,
        ENTITY_DECL,
        ARCHITECTURE_BODY,
        PACKAGE_DECL,
        PACKAGE_BODY,
        PACKAGE_INSTANTIATION_DECL,
        CONTEXT_DECL,
        CONFIGURATION_DECL,
        BLOCK_CONFIGURATION,
        COMPONENT_CONFIGURATION,
        BINDING_INDICATION,
        CONFIGURATION_SPEC,

        // Interfaces.
        GENERIC_CLAUSE,
        PORT_CLAUSE,
        INTERFACE_LIST,
        INTERFACE_OBJECT_DECL,
        INTERFACE_TYPE_DECL,
        INTERFACE_SUBPROGRAM_DECL,
        INTERFACE_PACKAGE_DECL,
        MODE_VIEW_INDICATION,
        GENERIC_MAP_ASPECT,
        PORT_MAP_ASPECT,
        PARAMETER_MAP_ASPECT,
        ASSOCIATION_LIST,
        ASSOCIATION_ELEMENT,

        // Declarations.
        IDENT_LIST,
        TYPE_DECL,
        SUBTYPE_DECL,
        CONSTANT_DECL,
        SIGNAL_DECL,
        VARIABLE_DECL,
        FILE_DECL,
        FILE_OPEN_INFO,
        ALIAS_DECL,
        ATTRIBUTE_DECL,
        ATTRIBUTE_SPEC,
        ENTITY_SPEC,
        COMPONENT_DECL,
        SUBPROGRAM_DECL,
        SUBPROGRAM_BODY,
        SUBPROGRAM_INSTANTIATION_DECL,
        FUNCTION_SPEC,
        PROCEDURE_SPEC,
        GROUP_TEMPLATE_DECL,
        GROUP_DECL,
        DISCONNECTION_SPEC,
        MODE_VIEW_DECL,
        MODE_VIEW_ELEMENT,
        PSL_DIRECTIVE,

        // Type definitions.
        ENUM_TYPE_DEF,
        RANGE_TYPE_DEF,
        PHYSICAL_TYPE_DEF,
        UNIT_DECL,
        ARRAY_TYPE_DEF,
        INDEX_SUBTYPE_DEF,
        RECORD_TYPE_DEF,
        ELEMENT_DECL,
        ACCESS_TYPE_DEF,
        FILE_TYPE_DEF,
        PROTECTED_TYPE_DEF,
        PROTECTED_TYPE_BODY,
        PRIVATE_TYPE_DEF,
        SUBTYPE_INDICATION,
        RESOLUTION_INDICATION,
        RANGE_CONSTRAINT,

        // Concurrent statements.
        LABEL,
        PROCESS_STMT,
        SENSITIVITY_LIST,
        BLOCK_STMT,
        COMPONENT_INSTANTIATION_STMT,
        FOR_GENERATE_STMT,
        IF_GENERATE_STMT,
        CASE_GENERATE_STMT,
        GENERATE_BODY,

        // Statements valid in both regions.
        SIGNAL_ASSIGN_STMT,
        SELECTED_ASSIGN_STMT,
        VARIABLE_ASSIGN_STMT,
        PROCEDURE_CALL_STMT,
        ASSERT_STMT,

        // Sequential statements.
        WAIT_STMT,
        REPORT_STMT,
        IF_STMT,
        CASE_STMT,
        CASE_ALTERNATIVE,
        LOOP_STMT,
        WHILE_SCHEME,
        FOR_SCHEME,
        NEXT_STMT,
        EXIT_STMT,
        RETURN_STMT,
        NULL_STMT,
        SEQUENTIAL_BLOCK_STMT,

        // Assignment parts.
        DELAY_MECHANISM,
        WAVEFORM,
        WAVEFORM_ELEMENT,
        CONDITIONAL_WAVEFORMS,
        SELECTED_WAVEFORMS,
        CONDITIONAL_EXPR,
        CHOICES,

        // Expressions and names.
        BINARY_EXPR,
        UNARY_EXPR,
        PAREN_EXPR,
        AGGREGATE,
        ELEMENT_ASSOCIATION,
        LITERAL,
        PHYSICAL_LITERAL,
        ALLOCATOR,
        QUALIFIED_EXPR,
        RANGE_EXPR,
        NAME,
        SELECTED_NAME,
        /// `prefix(...)`: a function call, an indexed name, a slice or a
        /// constrained type mark. Only semantic analysis can tell them apart.
        CALL_EXPR,
        ATTRIBUTE_NAME,
        SIGNATURE,
        EXTERNAL_NAME,
        EXTERNAL_PATH,
    }
}

impl SyntaxKind {
    /// Whitespace and comments.
    pub fn is_trivia(self) -> bool {
        matches!(
            self,
            SyntaxKind::WHITESPACE
                | SyntaxKind::LINE_COMMENT
                | SyntaxKind::BLOCK_COMMENT
                | SyntaxKind::TOOL_DIRECTIVE
        )
    }

    /// The reserved word `text` (in lower case) as of `std`, if it is one.
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
            AMP => "&",
            TICK => "'",
            LPAREN => "(",
            RPAREN => ")",
            STAR => "*",
            STAR2 => "**",
            PLUS => "+",
            COMMA => ",",
            MINUS => "-",
            DOT => ".",
            SLASH => "/",
            COLON => ":",
            SEMI => ";",
            LT => "<",
            LTE => "<=",
            BOX => "<>",
            EQ => "=",
            NEQ => "/=",
            GT => ">",
            GTE => ">=",
            ARROW => "=>",
            COLON_EQ => ":=",
            BAR => "|",
            LBRACKET => "[",
            RBRACKET => "]",
            LBRACE => "{",
            RBRACE => "}",
            QQ => "??",
            QEQ => "?=",
            QNEQ => "?/=",
            QLT => "?<",
            QLTE => "?<=",
            QGT => "?>",
            QGTE => "?>=",
            QUESTION => "?",
            LTLT => "<<",
            GTGT => ">>",
            CARET => "^",
            AT => "@",
            _ => return None,
        })
    }

    /// A human-readable description, for "expected ..." messages.
    pub fn describe(self) -> String {
        use SyntaxKind::*;
        match self {
            IDENT | EXT_IDENT => "identifier".into(),
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

/// The rowan language tag for VHDL.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Vhdl {}

impl rowan::Language for Vhdl {
    type Kind = SyntaxKind;

    fn kind_from_raw(raw: rowan::SyntaxKind) -> SyntaxKind {
        SyntaxKind::from_raw(raw.0)
    }

    fn kind_to_raw(kind: SyntaxKind) -> rowan::SyntaxKind {
        kind.into()
    }
}

pub type SyntaxNode = rowan::SyntaxNode<Vhdl>;
pub type SyntaxToken = rowan::SyntaxToken<Vhdl>;
pub type SyntaxElement = rowan::SyntaxElement<Vhdl>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_kinds_fit_in_a_token_set() {
        assert!((SyntaxKind::EOF as u16) < 256);
        assert!((SyntaxKind::XOR_KW as u16) < 256);
    }

    #[test]
    fn raw_round_trip() {
        for &kind in SyntaxKind::ALL {
            assert_eq!(SyntaxKind::from_raw(kind as u16), kind);
        }
    }

    #[test]
    fn keywords_depend_on_standard() {
        assert_eq!(SyntaxKind::keyword("view", Standard::Vhdl2008), None);
        assert_eq!(
            SyntaxKind::keyword("view", Standard::Vhdl2019),
            Some(SyntaxKind::VIEW_KW)
        );
        assert_eq!(SyntaxKind::keyword("context", Standard::Vhdl1993), None);
        assert_eq!(
            SyntaxKind::keyword("entity", Standard::Vhdl1993),
            Some(SyntaxKind::ENTITY_KW)
        );
    }
}

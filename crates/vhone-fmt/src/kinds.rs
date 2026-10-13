//! How the formatter classifies syntax tree nodes.

use vhone_syntax::SyntaxKind::{self, *};
use vhone_syntax::SyntaxNode;

/// Nodes that start their own line: statements, declarations, clauses,
/// design units. They are only items inside a [`container`](is_container).
pub(crate) fn is_item_kind(kind: SyntaxKind) -> bool {
    matches!(
        kind,
        DESIGN_UNIT
            | LIBRARY_CLAUSE
            | USE_CLAUSE
            | CONTEXT_REFERENCE
            | ENTITY_DECL
            | ARCHITECTURE_BODY
            | PACKAGE_DECL
            | PACKAGE_BODY
            | PACKAGE_INSTANTIATION_DECL
            | CONTEXT_DECL
            | CONFIGURATION_DECL
            | BLOCK_CONFIGURATION
            | COMPONENT_CONFIGURATION
            | BINDING_INDICATION
            | CONFIGURATION_SPEC
            | GENERIC_CLAUSE
            | PORT_CLAUSE
            | GENERIC_MAP_ASPECT
            | PORT_MAP_ASPECT
            | TYPE_DECL
            | SUBTYPE_DECL
            | CONSTANT_DECL
            | SIGNAL_DECL
            | VARIABLE_DECL
            | FILE_DECL
            | ALIAS_DECL
            | ATTRIBUTE_DECL
            | ATTRIBUTE_SPEC
            | COMPONENT_DECL
            | SUBPROGRAM_DECL
            | SUBPROGRAM_BODY
            | SUBPROGRAM_INSTANTIATION_DECL
            | GROUP_TEMPLATE_DECL
            | GROUP_DECL
            | DISCONNECTION_SPEC
            | MODE_VIEW_DECL
            | MODE_VIEW_ELEMENT
            | PSL_DIRECTIVE
            | ELEMENT_DECL
            | UNIT_DECL
            | PROCESS_STMT
            | BLOCK_STMT
            | COMPONENT_INSTANTIATION_STMT
            | FOR_GENERATE_STMT
            | IF_GENERATE_STMT
            | CASE_GENERATE_STMT
            | SIGNAL_ASSIGN_STMT
            | SELECTED_ASSIGN_STMT
            | VARIABLE_ASSIGN_STMT
            | PROCEDURE_CALL_STMT
            | ASSERT_STMT
            | WAIT_STMT
            | REPORT_STMT
            | IF_STMT
            | CASE_STMT
            | LOOP_STMT
            | NEXT_STMT
            | EXIT_STMT
            | RETURN_STMT
            | NULL_STMT
            | SEQUENTIAL_BLOCK_STMT
            | CASE_ALTERNATIVE
    )
}

/// Nodes whose items are indented one level deeper than the node itself.
pub(crate) fn is_container(kind: SyntaxKind) -> bool {
    matches!(
        kind,
        ENTITY_DECL
            | ARCHITECTURE_BODY
            | PACKAGE_DECL
            | PACKAGE_BODY
            | PACKAGE_INSTANTIATION_DECL
            | CONTEXT_DECL
            | CONFIGURATION_DECL
            | BLOCK_CONFIGURATION
            | COMPONENT_CONFIGURATION
            | BINDING_INDICATION
            | COMPONENT_DECL
            | SUBPROGRAM_BODY
            | SUBPROGRAM_INSTANTIATION_DECL
            | PROCESS_STMT
            | BLOCK_STMT
            | SEQUENTIAL_BLOCK_STMT
            | COMPONENT_INSTANTIATION_STMT
            | IF_STMT
            | CASE_STMT
            | CASE_ALTERNATIVE
            | LOOP_STMT
            | FOR_GENERATE_STMT
            | IF_GENERATE_STMT
            | CASE_GENERATE_STMT
            | GENERATE_BODY
            | RECORD_TYPE_DEF
            | PHYSICAL_TYPE_DEF
            | PROTECTED_TYPE_DEF
            | PROTECTED_TYPE_BODY
            | MODE_VIEW_DECL
    )
}

/// Nodes whose items stay at the node's own indentation.
pub(crate) fn is_flat_container(kind: SyntaxKind) -> bool {
    matches!(kind, SOURCE_FILE | DESIGN_UNIT | CONTEXT_CLAUSE)
}

/// Whether `node` starts its own line.
pub(crate) fn is_item(node: &SyntaxNode) -> bool {
    is_item_kind(node.kind())
        && node
            .parent()
            .is_some_and(|p| is_container(p.kind()) || is_flat_container(p.kind()))
}

/// Keywords that separate the sections of a container (`begin`, `end`,
/// `else`, `elsif`) when they are its direct children. They sit at the
/// container's own indentation, on their own line.
pub(crate) fn is_section_keyword(kind: SyntaxKind, parent: SyntaxKind) -> bool {
    matches!(kind, BEGIN_KW | END_KW | ELSE_KW | ELSIF_KW) && is_container(parent)
}

/// Parenthesized lists the formatter can lay out one element per line.
pub(crate) fn is_list(kind: SyntaxKind) -> bool {
    matches!(
        kind,
        INTERFACE_LIST | ASSOCIATION_LIST | AGGREGATE | ENUM_TYPE_DEF
    )
}

/// Line and block comments, and VHDL-2019 tool directives.
pub(crate) fn is_comment(kind: SyntaxKind) -> bool {
    matches!(kind, LINE_COMMENT | BLOCK_COMMENT | TOOL_DIRECTIVE)
}

//! Small helpers over the untyped syntax tree.

use vhone_syntax::{SyntaxKind, SyntaxNode, SyntaxToken, identifier_key};

/// Tokens that can name something: identifiers, operator symbols and
/// character literals.
pub fn is_designator(kind: SyntaxKind) -> bool {
    matches!(
        kind,
        SyntaxKind::IDENT
            | SyntaxKind::EXT_IDENT
            | SyntaxKind::STRING_LITERAL
            | SyntaxKind::CHAR_LITERAL
    )
}

/// The first identifier or operator symbol directly under `node`: the name
/// a declaration introduces.
pub fn designator(node: &SyntaxNode) -> Option<SyntaxToken> {
    node.children_with_tokens()
        .filter_map(|e| e.into_token())
        .find(|t| {
            matches!(
                t.kind(),
                SyntaxKind::IDENT | SyntaxKind::EXT_IDENT | SyntaxKind::STRING_LITERAL
            )
        })
}

/// The lookup key of a designator. Operator symbols keep their quotes and
/// are folded like identifiers (`"AND"` is `"and"`).
pub fn designator_key(token: &SyntaxToken) -> String {
    match token.kind() {
        SyntaxKind::STRING_LITERAL => token.text().to_ascii_lowercase(),
        SyntaxKind::CHAR_LITERAL => token.text().to_owned(),
        _ => identifier_key(token.text()),
    }
}

/// The identifier tokens directly under `node` (an `IDENT_LIST`, say).
pub fn idents(node: &SyntaxNode) -> Vec<SyntaxToken> {
    node.children_with_tokens()
        .filter_map(|e| e.into_token())
        .filter(|t| matches!(t.kind(), SyntaxKind::IDENT | SyntaxKind::EXT_IDENT))
        .collect()
}

/// The identifier of a `NAME` node.
pub fn simple_name_token(node: &SyntaxNode) -> Option<SyntaxToken> {
    if node.kind() != SyntaxKind::NAME {
        return None;
    }
    node.children_with_tokens()
        .filter_map(|e| e.into_token())
        .find(|t| matches!(t.kind(), SyntaxKind::IDENT | SyntaxKind::EXT_IDENT))
}

pub fn has_token(node: &SyntaxNode, kind: SyntaxKind) -> bool {
    node.children_with_tokens().any(|e| e.kind() == kind)
}

pub fn first_token(node: &SyntaxNode, kind: SyntaxKind) -> Option<SyntaxToken> {
    node.children_with_tokens()
        .filter_map(|e| e.into_token())
        .find(|t| t.kind() == kind)
}

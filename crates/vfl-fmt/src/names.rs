//! `[format.declaration_case]`: the case of an identifier follows the kind
//! of its declaration.
//!
//! VHDL is case-insensitive, so any identifier can be written in any case.
//! The declarations of the file are gathered into a table from name to
//! case, and every identifier with that name in the file, declaration or
//! reference, takes the case. A name declared as kinds that want different
//! cases (a constant `width` and a port `width`) keeps `identifier_case`.
//! Names declared in other files are not known and follow `identifier_case`
//! too.

use std::collections::HashMap;

use rowan::NodeOrToken;
use vfl_config::{Case, DeclarationCase, FormatConfig};
use vfl_syntax::SyntaxKind::*;
use vfl_syntax::{SyntaxNode, SyntaxToken, Vhdl};

use crate::elems::{Elem, apply_case};

pub(crate) fn apply_declaration_case(
    elems: &mut [Elem<Vhdl>],
    root: &SyntaxNode,
    config: &FormatConfig,
) {
    let table = declared_cases(root, &config.declaration_case, config.identifier_case);
    if table.is_empty() {
        return;
    }
    for elem in elems.iter_mut().filter(|e| e.kind == IDENT) {
        let Some(&case) = table.get(&elem.text.to_ascii_lowercase()) else {
            continue;
        };
        elem.text = match (case, &elem.syntax) {
            (Case::Preserve, Some(NodeOrToken::Token(token))) => token.text().to_owned(),
            _ => apply_case(&elem.text, case),
        };
    }
}

/// The case of each name (lower case) declared in `root`. Names whose
/// declarations want different cases are left out.
fn declared_cases(
    root: &SyntaxNode,
    cases: &DeclarationCase,
    default: Case,
) -> HashMap<String, Case> {
    let mut table: HashMap<String, Option<Case>> = HashMap::new();
    for node in root.descendants() {
        let Some((case, names)) = declared(&node, cases) else {
            continue;
        };
        let case = case.unwrap_or(default);
        for name in names {
            table
                .entry(name.text().to_ascii_lowercase())
                .and_modify(|c| {
                    if *c != Some(case) {
                        *c = None;
                    }
                })
                .or_insert(Some(case));
        }
    }
    table
        .into_iter()
        .filter_map(|(name, case)| Some((name, case?)))
        .collect()
}

/// The case `node` asks for the names it declares, and those names.
fn declared(
    node: &SyntaxNode,
    cases: &DeclarationCase,
) -> Option<(Option<Case>, Vec<SyntaxToken>)> {
    let own = |node: &SyntaxNode| -> Vec<SyntaxToken> {
        node.children_with_tokens()
            .filter_map(NodeOrToken::into_token)
            .filter(|t| t.kind() == IDENT)
            .collect()
    };
    let first = |node: &SyntaxNode| own(node).into_iter().take(1).collect::<Vec<_>>();
    let list = |node: &SyntaxNode| {
        node.children()
            .find(|c| c.kind() == IDENT_LIST)
            .map_or_else(Vec::new, |l| own(&l))
    };
    Some(match node.kind() {
        ENTITY_DECL => (cases.entity, first(node)),
        ARCHITECTURE_BODY => (cases.architecture, first(node)),
        PACKAGE_DECL | PACKAGE_INSTANTIATION_DECL => (cases.package, first(node)),
        COMPONENT_DECL => (cases.component, first(node)),
        INTERFACE_OBJECT_DECL => {
            let clause = node.parent().and_then(|l| l.parent()).map(|c| c.kind());
            let case = match clause {
                Some(GENERIC_CLAUSE) => cases.generic,
                Some(PORT_CLAUSE) => cases.port,
                _ => return None,
            };
            (case, list(node))
        }
        CONSTANT_DECL => (cases.constant, list(node)),
        SIGNAL_DECL => (cases.signal, list(node)),
        VARIABLE_DECL => (cases.variable, list(node)),
        TYPE_DECL => (cases.type_, first(node)),
        SUBTYPE_DECL => (cases.subtype, first(node)),
        ENUM_TYPE_DEF => (cases.enum_literal, own(node)),
        FUNCTION_SPEC => (cases.function, first(node)),
        PROCEDURE_SPEC => (cases.procedure, first(node)),
        ALIAS_DECL => (cases.alias, first(node)),
        LABEL => (cases.label, first(node)),
        _ => return None,
    })
}

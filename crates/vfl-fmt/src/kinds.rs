//! How the formatter classifies syntax tree nodes.

use crate::lang::{Lang, Node};

/// Whether `node` starts its own line: a statement, declaration, clause or
/// design unit inside a container.
#[inline]
pub(crate) fn is_item<L: Lang>(node: &Node<L>) -> bool {
    L::is_item_kind(node.kind())
        && node
            .parent()
            .is_some_and(|p| L::is_container(p.kind()) || L::is_flat_container(p.kind()))
        && !L::in_header(node)
}

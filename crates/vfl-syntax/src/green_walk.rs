//! Walks over a syntax tree that build no syntax nodes.
//!
//! Each rowan `SyntaxNode` or `SyntaxToken` a traversal steps on is a heap
//! allocation, freed again when the traversal moves on: a whole-file
//! `descendants()` allocates once per node. These walks read the green tree
//! underneath instead, which is already in memory, and only build syntax
//! nodes for what the caller asks for. They keep their own stack rather than
//! recurse: expressions can nest deeply.

use std::cell::Cell;

use rowan::{Children, Language, NodeOrToken, SyntaxElement, SyntaxNode, SyntaxToken, TextRange};

/// Calls `f` with the kind, range and text of each token under `node`, in
/// source order, building no syntax tokens.
pub fn for_each_token<L: Language>(
    node: &SyntaxNode<L>,
    mut f: impl FnMut(L::Kind, TextRange, &str),
) {
    let green = node.green();
    let mut offset = node.text_range().start();
    let mut stack: Vec<Children<'_>> = vec![green.children()];
    while let Some(children) = stack.last_mut() {
        match children.next() {
            Some(NodeOrToken::Node(child)) => stack.push(child.children()),
            Some(NodeOrToken::Token(token)) => {
                let range = TextRange::at(offset, token.text_len());
                f(L::kind_from_raw(token.kind()), range, token.text());
                offset = range.end();
            }
            None => {
                stack.pop();
            }
        }
    }
}

/// The nodes under `node`, itself included, whose kind `wanted` accepts,
/// in preorder: what `node.descendants().filter(...)` gives, but building
/// syntax nodes only for the matches and the nodes above them. Use it when
/// matches are few.
pub fn descendants_where<L: Language>(
    node: &SyntaxNode<L>,
    wanted: impl Fn(L::Kind) -> bool,
) -> Vec<SyntaxNode<L>> {
    let mut out = Vec::new();
    if wanted(node.kind()) {
        out.push(node.clone());
    }
    let green = node.green();
    // The children of each node on the current path (`node` at depth 0),
    // with how many of them were taken so far.
    let mut stack: Vec<(Children<'_>, u32)> = vec![(green.children(), 0)];
    // The syntax nodes built so far on the current path, from `node` down.
    let mut built: Vec<SyntaxNode<L>> = vec![node.clone()];
    while let Some((children, taken)) = stack.last_mut() {
        let Some(child) = children.next() else {
            stack.pop();
            built.truncate(stack.len());
            continue;
        };
        *taken += 1;
        let NodeOrToken::Node(child) = child else {
            continue;
        };
        if wanted(L::kind_from_raw(child.kind())) {
            // Build the path down to this child, from the deepest node
            // already built on it.
            for depth in built.len()..=stack.len() {
                let parent = &built[depth - 1];
                built.push(nth_child(parent, stack[depth - 1].1 - 1));
            }
            out.push(built[stack.len()].clone());
        }
        stack.push((child.children(), 0));
    }
    out
}

/// The child node at this index among all children, tokens included.
/// `first_child_by_kind` skips the children it is not asked for without
/// building them; it asks about each child once, in order.
fn nth_child<L: Language>(node: &SyntaxNode<L>, index: u32) -> SyntaxNode<L> {
    let seen = Cell::new(0);
    node.first_child_by_kind(&|_| {
        let i = seen.get();
        seen.set(i + 1);
        i == index
    })
    .expect("the green tree has a node child at this index")
}

/// Child token lookups that build no syntax for the children they skip:
/// what `children_with_tokens().filter_map(|e| e.into_token()).find(...)`
/// gives, without a heap allocation per child stepped over.
pub trait ChildTokens<L: Language> {
    /// The first token directly under this node whose kind `wanted`
    /// accepts.
    fn child_token(&self, wanted: impl Fn(L::Kind) -> bool) -> Option<SyntaxToken<L>>;
    /// The last token directly under this node whose kind `wanted` accepts.
    fn last_child_token(&self, wanted: impl Fn(L::Kind) -> bool) -> Option<SyntaxToken<L>>;
    /// Whether a child, token or node, has a kind `wanted` accepts.
    fn has_child(&self, wanted: impl Fn(L::Kind) -> bool) -> bool;
}

impl<L: Language> ChildTokens<L> for SyntaxNode<L> {
    fn child_token(&self, wanted: impl Fn(L::Kind) -> bool) -> Option<SyntaxToken<L>> {
        let index = self.green().children().position(|c| {
            c.as_token()
                .is_some_and(|t| wanted(L::kind_from_raw(t.kind())))
        })?;
        nth_child_or_token(self, index as u32).into_token()
    }

    fn last_child_token(&self, wanted: impl Fn(L::Kind) -> bool) -> Option<SyntaxToken<L>> {
        let index = self.green().children().rposition(|c| {
            c.as_token()
                .is_some_and(|t| wanted(L::kind_from_raw(t.kind())))
        })?;
        nth_child_or_token(self, index as u32).into_token()
    }

    fn has_child(&self, wanted: impl Fn(L::Kind) -> bool) -> bool {
        self.green()
            .children()
            .any(|c| wanted(L::kind_from_raw(c.kind())))
    }
}

/// The child, token or node, at this index. Like [`nth_child`].
fn nth_child_or_token<L: Language>(node: &SyntaxNode<L>, index: u32) -> SyntaxElement<L> {
    let seen = Cell::new(0);
    node.first_child_or_token_by_kind(&|_| {
        let i = seen.get();
        seen.set(i + 1);
        i == index
    })
    .expect("the green tree has a child at this index")
}

//! Walks over a syntax tree that build no syntax nodes.
//!
//! Each rowan `SyntaxNode` or `SyntaxToken` a traversal steps on is a heap
//! allocation, freed again when the traversal moves on: a whole-file
//! `descendants()` allocates once per node. These walks read the green tree
//! underneath instead, which is already in memory, and only build syntax
//! nodes for what the caller asks for. They keep their own stack rather than
//! recurse: expressions can nest deeply.

use std::cell::Cell;
use std::marker::PhantomData;

use rowan::{
    Children, GreenNodeData, Language, NodeOrToken, SyntaxElement, SyntaxNode, SyntaxToken,
    TextRange, TextSize,
};

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
    descendants_where_green(node, |green| wanted(green.kind()))
}

/// A green node with where it starts in the file: lets a walk look inside a
/// node before deciding to build a syntax node for it.
pub struct GreenAt<'a, L> {
    green: &'a GreenNodeData,
    start: TextSize,
    language: PhantomData<L>,
}

impl<L> Clone for GreenAt<'_, L> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<L> Copy for GreenAt<'_, L> {}

impl<'a, L: Language> GreenAt<'a, L> {
    pub fn kind(self) -> L::Kind {
        L::kind_from_raw(self.green.kind())
    }

    pub fn range(self) -> TextRange {
        TextRange::at(self.start, self.green.text_len())
    }

    /// The child nodes, in order.
    pub fn nodes(self) -> impl Iterator<Item = GreenAt<'a, L>> {
        self.elements().filter_map(|e| e.left())
    }

    /// The kind and range of each child token, in order.
    pub fn tokens(self) -> impl Iterator<Item = (L::Kind, TextRange)> {
        self.elements().filter_map(|e| e.right())
    }

    fn elements(self) -> impl Iterator<Item = Either<GreenAt<'a, L>, (L::Kind, TextRange)>> {
        let mut offset = self.start;
        self.green.children().map(move |child| {
            let start = offset;
            offset += child.text_len();
            match child {
                NodeOrToken::Node(green) => Either::Left(GreenAt {
                    green,
                    start,
                    language: PhantomData,
                }),
                NodeOrToken::Token(token) => Either::Right((
                    L::kind_from_raw(token.kind()),
                    TextRange::at(start, token.text_len()),
                )),
            }
        })
    }
}

enum Either<A, B> {
    Left(A),
    Right(B),
}

impl<A, B> Either<A, B> {
    fn left(self) -> Option<A> {
        match self {
            Either::Left(a) => Some(a),
            Either::Right(_) => None,
        }
    }

    fn right(self) -> Option<B> {
        match self {
            Either::Left(_) => None,
            Either::Right(b) => Some(b),
        }
    }
}

/// Like [`descendants_where`], but `wanted` sees each node's green node and
/// where it starts, so it can rule a node out by its children (an operator
/// token, a reference the analysis resolved) without a syntax node built.
pub fn descendants_where_green<L: Language>(
    node: &SyntaxNode<L>,
    mut wanted: impl FnMut(GreenAt<'_, L>) -> bool,
) -> Vec<SyntaxNode<L>> {
    let mut out = Vec::new();
    let green = node.green();
    let start = node.text_range().start();
    if wanted(GreenAt {
        green: &green,
        start,
        language: PhantomData,
    }) {
        out.push(node.clone());
    }
    // The children of each node on the current path (`node` at depth 0),
    // with how many of them were taken so far and the range of the last
    // one taken.
    let mut stack: Vec<(Children<'_>, u32, TextRange)> =
        vec![(green.children(), 0, TextRange::empty(start))];
    // The syntax nodes built so far on the current path, from `node` down.
    let mut built: Vec<SyntaxNode<L>> = vec![node.clone()];
    while let Some((children, taken, last)) = stack.last_mut() {
        let Some(child) = children.next() else {
            stack.pop();
            built.truncate(stack.len());
            continue;
        };
        *taken += 1;
        *last = TextRange::at(last.end(), child.text_len());
        let range = *last;
        let NodeOrToken::Node(child) = child else {
            continue;
        };
        if wanted(GreenAt {
            green: child,
            start: range.start(),
            language: PhantomData,
        }) {
            // Build the path down to this child, from the deepest node
            // already built on it.
            for depth in built.len()..=stack.len() {
                let parent = &built[depth - 1];
                let (_, taken, range) = stack[depth - 1];
                built.push(child_at(parent, taken - 1, range));
            }
            out.push(built[stack.len()].clone());
        }
        stack.push((child.children(), 0, TextRange::empty(range.start())));
    }
    out
}

/// The child node at this index among all children, whose range is
/// `range`: found by its range, as the index alone would take a walk over
/// the children before it (thousands, in a generated netlist's
/// architecture).
fn child_at<L: Language>(node: &SyntaxNode<L>, index: u32, range: TextRange) -> SyntaxNode<L> {
    match node.child_or_token_at_range(range) {
        Some(NodeOrToken::Node(child)) if child.index() == index as usize => child,
        // An empty child shares its range with its neighbours.
        _ => nth_child(node, index),
    }
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

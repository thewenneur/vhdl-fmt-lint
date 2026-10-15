//! Column alignment across consecutive lines.
//!
//! Each line can carry *anchors*: tokens that should line up with the same
//! token of neighbouring lines in the same construct (the `:` of port
//! declarations, the `<=` of assignments, trailing comments, ...). Anchors
//! of the same kind in the same block (`scope = "block"`: a process, an
//! architecture, an interface list, ...) or construct on consecutive lines
//! (`scope = "contiguous"`) form a run, and every anchor of a run is padded
//! to the run's rightmost column. Anchors further left on a line are
//! aligned first.

use std::collections::BTreeMap;

use rowan::NodeOrToken;
use vfl_config::{AlignConfig, AlignScope, BlockBoundary};

use crate::elems::Elem;
use crate::lang::{Lang, Node};
use crate::layout::{Analysis, Line, column};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Kind {
    /// A Verilog port's type, after its direction.
    PortType,
    InterfaceColon,
    DeclColon,
    DeclIs,
    Assign,
    Arrow,
    CaseArrow,
    InterfaceType,
    AlternativeAssign,
    /// The name declared by a Verilog port, parameter or declaration.
    Name,
    InterfaceDefault,
    DeclDefault,
    When,
    /// The `;` ending a line, by the class of the line's first anchor:
    /// interface elements, declarations and statements each align among
    /// themselves.
    Semicolon(u8),
    Comment,
}

impl Kind {
    /// The `;` ending a line whose first anchor is of this kind.
    fn semicolon(self) -> Kind {
        Kind::Semicolon(match self {
            Kind::PortType
            | Kind::InterfaceColon
            | Kind::InterfaceType
            | Kind::Name
            | Kind::InterfaceDefault => 0,
            Kind::DeclColon | Kind::DeclIs | Kind::DeclDefault => 1,
            _ => 2,
        })
    }

    /// Anchors of a lower rank sit further left on their lines.
    fn rank(self) -> u8 {
        match self {
            Kind::PortType
            | Kind::InterfaceColon
            | Kind::DeclColon
            | Kind::DeclIs
            | Kind::Assign
            | Kind::Arrow
            | Kind::CaseArrow => 0,
            Kind::InterfaceType | Kind::AlternativeAssign | Kind::Name => 1,
            Kind::InterfaceDefault | Kind::DeclDefault | Kind::When => 2,
            Kind::Semicolon(_) => 3,
            Kind::Comment => 4,
        }
    }
}

/// Identifies the construct an anchor belongs to.
pub(crate) type Context = (u32, u16);

pub(crate) fn context<L: Lang>(node: &Node<L>) -> Context {
    (
        node.text_range().start().into(),
        L::kind_to_raw(node.kind()).0,
    )
}

/// The context of anchors in `node`: `node` itself, or with
/// `scope = "block"` the block around it.
pub(crate) fn context_in<L: Lang>(node: &Node<L>, config: &AlignConfig) -> Context {
    match config.scope {
        AlignScope::Contiguous => context(node),
        AlignScope::Block => context(&block(node)),
    }
}

/// The innermost block (process, architecture, interface list, ...) that
/// holds `node`, or `node` itself if it is one.
pub(crate) fn block<L: Lang>(node: &Node<L>) -> Node<L> {
    let mut last = node.clone();
    for n in node.ancestors() {
        if L::is_align_block(n.kind()) {
            return n;
        }
        last = n;
    }
    last
}

/// A run of anchors aligned together: its first and last line, and with
/// `scope = "block"` the lines holding its anchors (other lines in between
/// do not depend on it).
#[derive(Debug, Clone)]
pub(crate) struct Run {
    pub first: usize,
    pub last: usize,
    /// Empty when every line from `first` to `last` counts.
    pub lines: Vec<usize>,
}

impl Run {
    /// Whether a line of `l0..=l1` is part of the run.
    pub fn touches(&self, l0: usize, l1: usize) -> bool {
        if self.first > l1 || self.last < l0 {
            return false;
        }
        if self.lines.is_empty() {
            return true;
        }
        let i = self.lines.partition_point(|&l| l < l0);
        self.lines.get(i).is_some_and(|&l| l <= l1)
    }
}

/// The padding to add before each element, and the first and last line of
/// each run of lines aligned together.
pub(crate) fn align<L: Lang>(
    elems: &[Elem<L>],
    an: &Analysis<L>,
    lines: &mut [Line],
    line_of: &[usize],
    config: &AlignConfig,
) -> (Vec<u32>, Vec<Run>) {
    let mut groups: BTreeMap<(Kind, Context), Vec<(usize, usize)>> = BTreeMap::new();
    for (li, line) in lines.iter().enumerate() {
        let mut add = |kind: Kind, ctx: Context, elem: usize| {
            if line_of[elem] != li {
                return;
            }
            let group = groups.entry((kind, ctx)).or_default();
            if group.last().is_none_or(|&(l, _)| l != li) {
                group.push((li, elem));
            }
        };
        if line.comment_only || line.verbatim {
            continue;
        }
        line_anchors(elems, an, line, config, &mut add);
        for (i, elem) in (line.start + 1..line.end).zip(&elems[line.start + 1..line.end]) {
            if let Some((kind, node)) = L::inline_anchor(elem, config) {
                add(kind, context_in(&node, config), i);
            }
        }
        let last = line.end - 1;
        if config.trailing_comment && last > line.start && elems[last].is_comment() {
            let ctx = match (config.scope, &elems[line.start].parent) {
                (AlignScope::Block, Some(parent)) => context_in(parent, config),
                _ => (0, 0),
            };
            add(Kind::Comment, ctx, last);
        }
    }

    let mut pads = vec![0u32; elems.len()];
    let mut spans = Vec::new();
    for rank in 0..=4 {
        if rank == 1 {
            // Continuation lines of conditional values start under the first
            // value, which may have moved with the `<=` just aligned.
            hang(elems, an, lines, line_of, &pads);
        }
        for ((kind, _), anchors) in &groups {
            if kind.rank() != rank {
                continue;
            }
            for run in runs(anchors, lines, config) {
                if run.len() < 2 {
                    continue;
                }
                spans.push(Run {
                    first: run[0].0,
                    last: run[run.len() - 1].0,
                    lines: match config.scope {
                        AlignScope::Block => run.iter().map(|&(l, _)| l).collect(),
                        AlignScope::Contiguous => Vec::new(),
                    },
                });
                let target = run
                    .iter()
                    .map(|&(l, e)| column(&lines[l], e, elems, an, &pads))
                    .max()
                    .unwrap_or(0);
                for &(l, e) in run {
                    let col = column(&lines[l], e, elems, an, &pads);
                    pads[e] += (target - col) as u32;
                }
            }
        }
    }
    (pads, spans)
}

/// Re-indents the lines that hang under a conditional assignment's first
/// value, and the lines that follow their indentation.
fn hang<L: Lang>(
    elems: &[Elem<L>],
    an: &Analysis<L>,
    lines: &mut [Line],
    line_of: &[usize],
    pads: &[u32],
) {
    let mut delta = vec![0i64; lines.len()];
    for li in 0..lines.len() {
        if lines[li].comment_only {
            continue;
        }
        let d = match (lines[li].hang, lines[li].follows) {
            (Some(f), _) => {
                column(&lines[line_of[f]], f, elems, an, pads) as i64 - i64::from(lines[li].indent)
            }
            (None, Some(p)) => delta[p],
            (None, None) => 0,
        };
        delta[li] = d;
        lines[li].indent = (i64::from(lines[li].indent) + d).max(0) as u32;
    }
    for line in lines.iter_mut() {
        if L::keeps_column(elems[line.start].kind) {
            continue;
        }
        if let (true, Some(p)) = (line.comment_only, line.follows) {
            line.indent = (i64::from(line.indent) + delta[p]).max(0) as u32;
        }
    }
}

/// Splits a group's anchors into runs of lines that align together.
fn runs<'a>(
    anchors: &'a [(usize, usize)],
    lines: &[Line],
    config: &AlignConfig,
) -> Vec<&'a [(usize, usize)]> {
    let boundary = config.block_boundary;
    let joinable = |a: usize, b: usize| {
        (a + 1..=b).all(|m| {
            if lines[m].verbatim {
                return false;
            }
            if config.scope == AlignScope::Block {
                return true;
            }
            let blank_ok = boundary == BlockBoundary::Never || lines[m].blank_before == 0;
            let between_ok =
                m == b || (lines[m].comment_only && boundary != BlockBoundary::Comment);
            blank_ok && between_ok
        })
    };
    let mut out = Vec::new();
    let mut start = 0;
    for i in 1..=anchors.len() {
        if i == anchors.len() || !joinable(anchors[i - 1].0, anchors[i].0) {
            out.push(&anchors[start..i]);
            start = i;
        }
    }
    out
}

/// The anchors of the construct that begins `line`.
fn line_anchors<L: Lang>(
    elems: &[Elem<L>],
    an: &Analysis<L>,
    line: &Line,
    config: &AlignConfig,
    add: &mut impl FnMut(Kind, Context, usize),
) {
    // A leading separator is not part of the element after it.
    let skip = an.leading_list(line.start).is_some() && line.start + 1 < line.end;
    let first = &elems[line.start + usize::from(skip)];
    let Some(syntax) = &first.syntax else {
        return;
    };
    if first.is_comment() {
        return;
    }
    let start = syntax.text_range().start();
    let mut node = match syntax {
        NodeOrToken::Node(n) => Some(n.clone()),
        NodeOrToken::Token(t) => t.parent(),
    };
    while let Some(n) = node.filter(|n| n.text_range().start() == start) {
        // The context of the first anchor on the line, for its `;`, and
        // the last anchor, which the `;` must follow.
        let mut first = None;
        let mut last = 0;
        let mut on_line = |kind: Kind, ctx: Context, elem: usize| {
            if (line.start..line.end).contains(&elem) {
                first.get_or_insert((kind.semicolon(), ctx));
                last = last.max(elem);
            }
            add(kind, ctx, elem);
        };
        if L::anchors_of(&n, elems, an, line, config, &mut on_line) {
            if config.semicolon
                && let Some((kind, ctx)) = first
                && let Some(semi) = line_semicolon(&n, elems, line).filter(|&s| s > last)
            {
                add(kind, ctx, semi);
            }
            return;
        }
        node = n.parent();
    }
}

/// The `;` that ends `line` (before any trailing comment), when it ends
/// the construct `node` that begins the line or separates it from the next
/// element of a list.
fn line_semicolon<L: Lang>(node: &Node<L>, elems: &[Elem<L>], line: &Line) -> Option<usize> {
    let mut last = line.end - 1;
    while last > line.start && elems[last].is_comment() {
        last -= 1;
    }
    if last == line.start || !elems[last].is_token(L::SEMI) {
        return None;
    }
    let token = elems[last].syntax.as_ref()?.as_token()?;
    let inside = node.text_range().contains_range(token.text_range());
    let separator = token.parent() == node.parent() && L::is_separator(token.kind());
    (inside || separator).then_some(last)
}

/// The element index of `node`'s first direct child token of a kind in
/// `kinds`.
pub(crate) fn child_token<L: Lang>(
    node: &Node<L>,
    an: &Analysis<L>,
    kinds: &[L::Kind],
) -> Option<usize> {
    node.children_with_tokens()
        .filter_map(NodeOrToken::into_token)
        .find(|t| kinds.contains(&t.kind()))
        .and_then(|t| an.index.get(&t.text_range().start()).copied())
}

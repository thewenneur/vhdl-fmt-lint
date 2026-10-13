//! Column alignment across consecutive lines.
//!
//! Each line can carry *anchors*: tokens that should line up with the same
//! token of neighbouring lines in the same construct (the `:` of port
//! declarations, the `<=` of assignments, trailing comments, ...). Anchors
//! with the same construct and kind on consecutive lines form a run, and
//! every anchor of a run is padded to the run's rightmost column. Anchors
//! further left on a line are aligned first.

use std::collections::BTreeMap;

use rowan::NodeOrToken;
use vfl_config::{AlignConfig, BlockBoundary};
use vfl_syntax::SyntaxKind::{self, *};
use vfl_syntax::SyntaxNode;

use crate::elems::Elem;
use crate::layout::{Analysis, Line, column};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Kind {
    InterfaceColon,
    DeclColon,
    DeclIs,
    Assign,
    Arrow,
    CaseArrow,
    InterfaceType,
    AlternativeAssign,
    InterfaceDefault,
    DeclDefault,
    When,
    Comment,
}

impl Kind {
    /// Anchors of a lower rank sit further left on their lines.
    fn rank(self) -> u8 {
        match self {
            Kind::InterfaceColon
            | Kind::DeclColon
            | Kind::DeclIs
            | Kind::Assign
            | Kind::Arrow
            | Kind::CaseArrow => 0,
            Kind::InterfaceType | Kind::AlternativeAssign => 1,
            Kind::InterfaceDefault | Kind::DeclDefault | Kind::When => 2,
            Kind::Comment => 3,
        }
    }
}

/// Identifies the construct an anchor belongs to.
type Context = (u32, u16);

fn context(node: &SyntaxNode) -> Context {
    (node.text_range().start().into(), node.kind() as u16)
}

/// The padding to add before each element.
pub(crate) fn align(
    elems: &[Elem],
    an: &Analysis,
    lines: &mut [Line],
    line_of: &[usize],
    config: &AlignConfig,
) -> Vec<u32> {
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
            if config.when_clause && elem.is_token(WHEN_KW) {
                if let Some(parent) = elem.parent.as_ref().filter(|p| {
                    matches!(
                        p.kind(),
                        CONDITIONAL_WAVEFORMS | SELECTED_WAVEFORMS | CONDITIONAL_EXPR
                    )
                }) {
                    add(Kind::When, context(parent), i);
                }
            }
        }
        let last = line.end - 1;
        if config.trailing_comment && last > line.start && elems[last].is_comment() {
            add(Kind::Comment, (0, 0), last);
        }
    }

    let mut pads = vec![0u32; elems.len()];
    for rank in 0..=3 {
        if rank == 1 {
            // Continuation lines of conditional values start under the first
            // value, which may have moved with the `<=` just aligned.
            hang(elems, an, lines, line_of, &pads);
        }
        for ((kind, _), anchors) in &groups {
            if kind.rank() != rank {
                continue;
            }
            for run in runs(anchors, lines, config.block_boundary) {
                if run.len() < 2 {
                    continue;
                }
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
    pads
}

/// Re-indents the lines that hang under a conditional assignment's first
/// value, and the lines that follow their indentation.
fn hang(elems: &[Elem], an: &Analysis, lines: &mut [Line], line_of: &[usize], pads: &[u32]) {
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
        if let (true, Some(p)) = (line.comment_only, line.follows) {
            line.indent = (i64::from(line.indent) + delta[p]).max(0) as u32;
        }
    }
}

/// Splits a group's anchors into runs of lines that align together.
fn runs<'a>(
    anchors: &'a [(usize, usize)],
    lines: &[Line],
    boundary: BlockBoundary,
) -> Vec<&'a [(usize, usize)]> {
    let joinable = |a: usize, b: usize| {
        (a + 1..=b).all(|m| {
            if lines[m].verbatim {
                return false;
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
fn line_anchors(
    elems: &[Elem],
    an: &Analysis,
    line: &Line,
    config: &AlignConfig,
    add: &mut impl FnMut(Kind, Context, usize),
) {
    let first = &elems[line.start];
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
        if anchors_of(&n, elems, an, line, config, add) {
            return;
        }
        node = n.parent();
    }
}

/// The element index of `node`'s first direct child token of kind `kind`.
fn child_token(node: &SyntaxNode, an: &Analysis, kinds: &[SyntaxKind]) -> Option<usize> {
    node.children_with_tokens()
        .filter_map(NodeOrToken::into_token)
        .find(|t| kinds.contains(&t.kind()))
        .and_then(|t| an.index.get(&t.text_range().start()).copied())
}

/// Adds the anchors of `node` if it is a construct that has any. Returns
/// whether it was one.
fn anchors_of(
    node: &SyntaxNode,
    elems: &[Elem],
    an: &Analysis,
    line: &Line,
    config: &AlignConfig,
    add: &mut impl FnMut(Kind, Context, usize),
) -> bool {
    let parent = node.parent();
    let Some(parent) = parent else {
        return false;
    };
    let ctx = context(&parent);
    match node.kind() {
        INTERFACE_OBJECT_DECL => {
            let generic = parent
                .parent()
                .is_some_and(|clause| clause.kind() == GENERIC_CLAUSE);
            let colon = child_token(node, an, &[COLON]);
            if let Some(colon) = colon {
                if (generic && config.generic_colon) || (!generic && config.port_colon) {
                    add(Kind::InterfaceColon, ctx, colon);
                }
                if !generic && config.port_mode {
                    let mode =
                        child_token(node, an, &[IN_KW, OUT_KW, INOUT_KW, BUFFER_KW, LINKAGE_KW]);
                    let before_type = mode.unwrap_or(colon);
                    if before_type + 1 < line.end && !elems[before_type + 1].is_comment() {
                        add(Kind::InterfaceType, ctx, before_type + 1);
                    }
                }
            }
            if config.interface_default {
                if let Some(default) = child_token(node, an, &[COLON_EQ]) {
                    add(Kind::InterfaceDefault, ctx, default);
                }
            }
            true
        }
        SIGNAL_DECL | CONSTANT_DECL | VARIABLE_DECL | FILE_DECL | ELEMENT_DECL | ATTRIBUTE_DECL
        | MODE_VIEW_ELEMENT => {
            if config.declaration {
                if let Some(colon) = child_token(node, an, &[COLON]) {
                    add(Kind::DeclColon, ctx, colon);
                }
                if let Some(default) = child_token(node, an, &[COLON_EQ]) {
                    add(Kind::DeclDefault, ctx, default);
                }
            }
            true
        }
        ALIAS_DECL => {
            if config.declaration {
                match child_token(node, an, &[COLON]) {
                    Some(colon) => add(Kind::DeclColon, ctx, colon),
                    None => {
                        if let Some(is) = child_token(node, an, &[IS_KW]) {
                            add(Kind::DeclIs, ctx, is);
                        }
                    }
                }
            }
            true
        }
        TYPE_DECL | SUBTYPE_DECL => {
            if config.declaration {
                if let Some(is) = child_token(node, an, &[IS_KW]) {
                    add(Kind::DeclIs, ctx, is);
                }
            }
            true
        }
        SIGNAL_ASSIGN_STMT | VARIABLE_ASSIGN_STMT => {
            if config.assignment {
                if let Some(op) = child_token(node, an, &[LTE, COLON_EQ]) {
                    add(Kind::Assign, ctx, op);
                }
            }
            true
        }
        ASSOCIATION_ELEMENT | ELEMENT_ASSOCIATION => {
            if config.association {
                if let Some(arrow) = child_token(node, an, &[ARROW]) {
                    add(Kind::Arrow, ctx, arrow);
                }
            }
            true
        }
        CASE_ALTERNATIVE => {
            // Only when a statement follows on the same line.
            let Some(arrow) = child_token(node, an, &[ARROW])
                .filter(|&a| a + 1 < line.end && !elems[a + 1].is_comment())
            else {
                return true;
            };
            if config.case_choice {
                add(Kind::CaseArrow, ctx, arrow);
            }
            // Aligning the statements only makes sense when the choices are.
            if config.case_choice && config.assignment {
                if let Some(op) = statement_assignment(&elems[arrow + 1], an) {
                    add(Kind::AlternativeAssign, ctx, op);
                }
            }
            true
        }
        _ => false,
    }
}

/// The `<=` or `:=` of the assignment statement starting at `elem`.
fn statement_assignment(elem: &Elem, an: &Analysis) -> Option<usize> {
    let token = elem.syntax.as_ref()?.as_token()?.clone();
    let start = token.text_range().start();
    token
        .parent_ancestors()
        .take_while(|n| n.text_range().start() == start)
        .find(|n| matches!(n.kind(), SIGNAL_ASSIGN_STMT | VARIABLE_ASSIGN_STMT))
        .and_then(|stmt| child_token(&stmt, an, &[LTE, COLON_EQ]))
}

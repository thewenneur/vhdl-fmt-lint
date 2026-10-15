//! Flattens the syntax tree into the sequence of things the formatter
//! places: tokens, comments, and opaque chunks kept verbatim.

use std::ops::Range;

use rowan::{NodeOrToken, TextRange, TextSize, WalkEvent};
use vfl_config::{Case, FormatConfig};

use crate::lang::{Element, Lang, Node};

/// One placed piece of output.
#[derive(Debug, Clone)]
pub(crate) struct Elem<L: Lang> {
    /// The token kind, or the node kind of a chunk kept as written.
    pub kind: L::Kind,
    /// The output text (case-converted where the config says so).
    pub text: String,
    /// The tree element it comes from: a token, or a node for opaque
    /// chunks. `None` for tokens the formatter adds.
    pub syntax: Option<Element<L>>,
    /// The node the element sits in, for context.
    pub parent: Option<Node<L>>,
    /// Where it was in the (normalized) source.
    pub src: Option<TextRange>,
    /// Line breaks in the whitespace before it, in the source.
    pub src_newlines: u32,
    /// Its column in the source, with tabs expanded.
    pub src_col: u32,
}

impl<L: Lang> Elem<L> {
    pub fn is_comment(&self) -> bool {
        L::is_comment(self.kind)
    }

    /// Whether this is the real token `kind` (not one the formatter added).
    pub fn is_token(&self, kind: L::Kind) -> bool {
        self.kind == kind && matches!(self.syntax, Some(NodeOrToken::Token(_)))
    }

    pub fn parent_kind(&self) -> Option<L::Kind> {
        self.parent.as_ref().map(Node::kind)
    }

    /// The columns it takes on screen: CJK and other wide characters
    /// count twice. Multi-line chunks count with their first line only.
    pub fn width(&self) -> usize {
        self.text.split('\n').next().map_or(0, display_width)
    }
}

/// The columns `text` takes on screen (Unicode East Asian width). Control
/// characters such as a tab inside a comment count as one column, as they
/// did before display widths were used.
pub(crate) fn display_width(text: &str) -> usize {
    if text.is_ascii() {
        return text.len();
    }
    text.chars()
        .map(|c| unicode_width::UnicodeWidthChar::width(c).unwrap_or(1))
        .sum()
}

#[inline(always)]
pub(crate) fn apply_case(text: &str, case: Case) -> String {
    match case {
        Case::Lower => text.to_ascii_lowercase(),
        Case::Upper => text.to_ascii_uppercase(),
        Case::Preserve => text.to_owned(),
    }
}

/// Collects the elements of `root`, applying keyword and identifier case.
/// With `window`, only those in these bytes of the source, which start and
/// end between tokens at the start and end of lines.
pub(crate) fn collect<L: Lang>(
    root: &Node<L>,
    config: &FormatConfig,
    window: Option<Range<usize>>,
) -> Vec<Elem<L>> {
    let window = window.map(|w| TextSize::from(w.start as u32)..TextSize::from(w.end as u32));
    let outside = |r: TextRange| {
        window
            .as_ref()
            .is_some_and(|w| r.end() <= w.start || r.start() >= w.end)
    };
    let tab = config.indent.max(1);
    let mut elems = Vec::new();
    let mut newlines = 0u32;
    let mut col = 0u32;
    let advance = |col: &mut u32, text: &str| {
        for c in text.chars() {
            match c {
                '\n' | '\r' => *col = 0,
                '\t' => *col = (*col / tab + 1) * tab,
                c if c.is_ascii() => *col += 1,
                c => *col += unicode_width::UnicodeWidthChar::width(c).unwrap_or(1) as u32,
            }
        }
    };
    let mut walk = root.preorder_with_tokens();
    while let Some(event) = walk.next() {
        let WalkEvent::Enter(element) = event else {
            continue;
        };
        if outside(element.text_range()) {
            if let NodeOrToken::Node(_) = element {
                walk.skip_subtree();
            }
            continue;
        }
        match element {
            NodeOrToken::Node(node) => {
                if L::is_chunk(node.kind()) {
                    walk.skip_subtree();
                    let text = node.to_string();
                    elems.push(Elem {
                        kind: node.kind(),
                        text: text.clone(),
                        parent: node.parent(),
                        src: Some(node.text_range()),
                        src_newlines: std::mem::take(&mut newlines),
                        src_col: col,
                        syntax: Some(NodeOrToken::Node(node)),
                    });
                    advance(&mut col, &text);
                }
            }
            NodeOrToken::Token(token) => {
                let kind = token.kind();
                let mut raw = token.text();
                if let Some(w) = &window {
                    // Whitespace may run over the window's edges.
                    let r = token.text_range();
                    let from = usize::from(w.start.max(r.start()) - r.start());
                    let to = usize::from(w.end.min(r.end()) - r.start());
                    raw = &raw[from..to];
                }
                if kind == L::WHITESPACE {
                    newlines += raw.chars().filter(|&c| c == '\n' || c == '\r').count() as u32;
                    advance(&mut col, raw);
                    continue;
                }
                // Blanks after a directive's `\` are kept: without them it
                // would continue on the next line.
                let text = if L::is_comment(kind)
                    && !raw.contains('\n')
                    && !(L::keeps_column(kind) && raw.trim_end().ends_with('\\'))
                {
                    raw.trim_end().to_owned()
                } else {
                    apply_case(raw, L::case_of(kind, config))
                };
                elems.push(Elem {
                    kind,
                    text,
                    parent: token.parent(),
                    src: Some(token.text_range()),
                    src_newlines: std::mem::take(&mut newlines),
                    src_col: col,
                    syntax: Some(NodeOrToken::Token(token.clone())),
                });
                advance(&mut col, raw);
            }
        }
    }
    elems
}

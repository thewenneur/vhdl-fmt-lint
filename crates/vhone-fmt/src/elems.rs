//! Flattens the syntax tree into the sequence of things the formatter
//! places: tokens, comments, and opaque chunks kept verbatim.

use std::ops::Range;

use rowan::{NodeOrToken, WalkEvent};
use vhone_config::{Case, EndKeyword, EndLabel, FormatConfig};
use vhone_syntax::SyntaxKind::{self, *};
use vhone_syntax::{SyntaxElement, SyntaxNode, SyntaxToken, TextRange, TextSize};

use crate::kinds::{is_comment, is_container};

/// One placed piece of output.
#[derive(Debug, Clone)]
pub(crate) struct Elem {
    /// The token kind, or `PSL_DIRECTIVE` for an opaque PSL chunk.
    pub kind: SyntaxKind,
    /// The output text (case-converted where the config says so).
    pub text: String,
    /// The tree element it comes from: a token, or a node for opaque
    /// chunks. `None` for tokens the formatter adds.
    pub syntax: Option<SyntaxElement>,
    /// The node the element sits in, for context.
    pub parent: Option<SyntaxNode>,
    /// Where it was in the (normalized) source.
    pub src: Option<TextRange>,
    /// Line breaks in the whitespace before it, in the source.
    pub src_newlines: u32,
    /// Its column in the source, with tabs expanded.
    pub src_col: u32,
}

impl Elem {
    pub fn is_comment(&self) -> bool {
        is_comment(self.kind)
    }

    /// Whether this is the real token `kind` (not one the formatter added).
    pub fn is_token(&self, kind: SyntaxKind) -> bool {
        self.kind == kind && matches!(self.syntax, Some(NodeOrToken::Token(_)))
    }

    pub fn parent_kind(&self) -> Option<SyntaxKind> {
        self.parent.as_ref().map(SyntaxNode::kind)
    }

    pub fn width(&self) -> usize {
        // Multi-line chunks count with their first line only.
        self.text
            .split('\n')
            .next()
            .map_or(0, |line| line.chars().count())
    }
}

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
pub(crate) fn collect(
    root: &SyntaxNode,
    config: &FormatConfig,
    window: Option<Range<usize>>,
) -> Vec<Elem> {
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
                _ => *col += 1,
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
                if node.kind() == PSL_DIRECTIVE {
                    walk.skip_subtree();
                    let text = node.to_string();
                    elems.push(Elem {
                        kind: PSL_DIRECTIVE,
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
                if kind == WHITESPACE {
                    newlines += raw.chars().filter(|&c| c == '\n' || c == '\r').count() as u32;
                    advance(&mut col, raw);
                    continue;
                }
                let text = if kind.is_keyword() {
                    apply_case(raw, config.keyword_case)
                } else if kind == IDENT {
                    apply_case(raw, config.identifier_case)
                } else if is_comment(kind) && !raw.contains('\n') {
                    raw.trim_end().to_owned()
                } else {
                    raw.to_owned()
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

/// What may follow `end` for a construct: the optional keywords that can be
/// added or dropped, and the name that can be repeated.
fn end_info(container: &SyntaxNode) -> Option<(Vec<SyntaxKind>, Option<SyntaxToken>)> {
    let first_ident = |node: &SyntaxNode| {
        node.children_with_tokens()
            .filter_map(NodeOrToken::into_token)
            .find(|t| matches!(t.kind(), IDENT | EXT_IDENT))
    };
    let label = || {
        container
            .children()
            .find(|n| n.kind() == LABEL)
            .and_then(|l| l.first_token())
    };
    Some(match container.kind() {
        ENTITY_DECL => (vec![ENTITY_KW], first_ident(container)),
        ARCHITECTURE_BODY => (vec![ARCHITECTURE_KW], first_ident(container)),
        PACKAGE_DECL => (vec![PACKAGE_KW], first_ident(container)),
        PACKAGE_BODY => (vec![PACKAGE_KW, BODY_KW], first_ident(container)),
        CONTEXT_DECL => (vec![CONTEXT_KW], first_ident(container)),
        CONFIGURATION_DECL => (vec![CONFIGURATION_KW], first_ident(container)),
        COMPONENT_DECL | MODE_VIEW_DECL => (vec![], first_ident(container)),
        SUBPROGRAM_BODY => {
            let spec = container
                .children()
                .find(|n| matches!(n.kind(), FUNCTION_SPEC | PROCEDURE_SPEC))?;
            let keyword = if spec.kind() == FUNCTION_SPEC {
                FUNCTION_KW
            } else {
                PROCEDURE_KW
            };
            let name = spec
                .children_with_tokens()
                .filter_map(NodeOrToken::into_token)
                .find(|t| matches!(t.kind(), IDENT | EXT_IDENT | STRING_LITERAL));
            (vec![keyword], name)
        }
        PROCESS_STMT
        | BLOCK_STMT
        | SEQUENTIAL_BLOCK_STMT
        | IF_STMT
        | CASE_STMT
        | LOOP_STMT
        | FOR_GENERATE_STMT
        | IF_GENERATE_STMT
        | CASE_GENERATE_STMT => (vec![], label()),
        RECORD_TYPE_DEF | PHYSICAL_TYPE_DEF | PROTECTED_TYPE_DEF | PROTECTED_TYPE_BODY => {
            let decl = container.parent().filter(|p| p.kind() == TYPE_DECL)?;
            (vec![], first_ident(&decl))
        }
        _ => return None,
    })
}

/// Adds or removes the optional keywords and names after `end`, as configured.
pub(crate) fn edit_end_clauses(elems: Vec<Elem>, config: &FormatConfig) -> Vec<Elem> {
    if config.end_label == EndLabel::Preserve && config.end_keyword == EndKeyword::Preserve {
        return elems;
    }
    let mut out = Vec::with_capacity(elems.len());
    let mut i = 0;
    while i < elems.len() {
        let elem = &elems[i];
        out.push(elem.clone());
        i += 1;
        let container = match &elem.parent {
            Some(p) if elem.is_token(END_KW) && is_container(p.kind()) => p.clone(),
            _ => continue,
        };
        let Some((keywords, name)) = end_info(&container) else {
            continue;
        };
        // The tail: everything up to the `;`.
        let tail_end = (i..elems.len())
            .find(|&j| elems[j].kind == SEMI)
            .unwrap_or(elems.len());
        let tail = &elems[i..tail_end];
        if tail.iter().any(|e| e.is_comment() || e.syntax.is_none()) {
            continue;
        }
        let designator = tail
            .last()
            .filter(|e| matches!(e.kind, IDENT | EXT_IDENT | STRING_LITERAL | CHAR_LITERAL));
        let tail_keywords: Vec<SyntaxKind> = tail
            .iter()
            .take(tail.len() - usize::from(designator.is_some()))
            .map(|e| e.kind)
            .collect();
        let synthetic = |kind: SyntaxKind, text: String| Elem {
            kind,
            text,
            syntax: None,
            parent: Some(container.clone()),
            src: None,
            src_newlines: 0,
            src_col: 0,
        };
        let keyword_case = match config.keyword_case {
            Case::Preserve if elem.text.chars().any(|c| c.is_ascii_uppercase()) => Case::Upper,
            Case::Preserve => Case::Lower,
            case => case,
        };

        let mut new_tail: Vec<Elem> = tail.to_vec();
        let keywords_present = !keywords.is_empty() && tail_keywords.starts_with(&keywords);
        match config.end_keyword {
            EndKeyword::Add if !keywords.is_empty() && tail_keywords.is_empty() => {
                let added = keywords.iter().map(|&kw| {
                    synthetic(kw, apply_case(kw.text().unwrap_or_default(), keyword_case))
                });
                new_tail.splice(0..0, added);
            }
            EndKeyword::Remove if keywords_present && tail_keywords.len() == keywords.len() => {
                new_tail.drain(0..keywords.len());
            }
            _ => {}
        }
        match config.end_label {
            EndLabel::Add if designator.is_none() => {
                if let Some(name) = &name {
                    let text = if name.kind() == IDENT {
                        apply_case(name.text(), config.identifier_case)
                    } else {
                        name.text().to_owned()
                    };
                    new_tail.push(synthetic(name.kind(), text));
                }
            }
            EndLabel::Remove if designator.is_some() => {
                new_tail.pop();
            }
            _ => {}
        }
        out.extend(new_tail);
        i = tail_end;
    }
    out
}

//! Checks that formatting changed nothing but layout.

use vhone_syntax::SyntaxKind::{self, *};
use vhone_syntax::{Standard, SyntaxNode};

use crate::kinds::{is_comment, is_container};

/// What formatting must preserve: the significant tokens (ignoring case
/// where VHDL does, and the optional words after `end`, which the formatter
/// may add or drop) and the comments.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Fingerprint {
    tokens: Vec<(SyntaxKind, String)>,
    comments: Vec<String>,
}

pub(crate) fn fingerprint(root: &SyntaxNode) -> Fingerprint {
    let mut tokens = Vec::new();
    let mut comments = Vec::new();
    let mut in_end_tail = false;
    for token in root
        .descendants_with_tokens()
        .filter_map(rowan::NodeOrToken::into_token)
    {
        let kind = token.kind();
        if kind == WHITESPACE {
            continue;
        }
        if is_comment(kind) {
            let text = token.text();
            comments.push(if text.contains('\n') {
                text.to_owned()
            } else {
                text.trim_end().to_owned()
            });
            continue;
        }
        if in_end_tail && kind != SEMI {
            continue;
        }
        in_end_tail = kind == END_KW && token.parent().is_some_and(|p| is_container(p.kind()));
        let text = if kind.is_keyword() || kind == IDENT {
            token.text().to_ascii_lowercase()
        } else {
            token.text().to_owned()
        };
        tokens.push((kind, text));
    }
    Fingerprint { tokens, comments }
}

/// The fingerprint of a run of whole lines, from its tokens alone: cheaper
/// than parsing the file again. Every `end` is taken to start an end
/// clause whose optional words may change.
pub(crate) fn lexical(text: &str, std: Standard) -> Fingerprint {
    let mut tokens = Vec::new();
    let mut comments = Vec::new();
    let mut in_end_tail = false;
    let mut pos = 0usize;
    for token in vhone_syntax::lex(text, std).0 {
        let len = usize::from(token.len);
        let text = &text[pos..pos + len];
        pos += len;
        let kind = token.kind;
        if kind == WHITESPACE {
            continue;
        }
        if is_comment(kind) {
            comments.push(if text.contains('\n') {
                text.to_owned()
            } else {
                text.trim_end().to_owned()
            });
            continue;
        }
        if in_end_tail && kind != SEMI {
            continue;
        }
        in_end_tail = kind == END_KW;
        tokens.push((
            kind,
            if kind.is_keyword() || kind == IDENT {
                text.to_ascii_lowercase()
            } else {
                text.to_owned()
            },
        ));
    }
    Fingerprint { tokens, comments }
}

/// Describes the first difference between two fingerprints.
pub(crate) fn first_difference(before: &Fingerprint, after: &Fingerprint) -> String {
    if before.comments != after.comments {
        let i = before
            .comments
            .iter()
            .zip(&after.comments)
            .position(|(a, b)| a != b)
            .unwrap_or(before.comments.len().min(after.comments.len()));
        return format!(
            "comment {} changed: {:?} became {:?}",
            i + 1,
            before.comments.get(i),
            after.comments.get(i)
        );
    }
    let i = before
        .tokens
        .iter()
        .zip(&after.tokens)
        .position(|(a, b)| a != b)
        .unwrap_or(before.tokens.len().min(after.tokens.len()));
    format!(
        "token {} changed: {:?} became {:?}",
        i + 1,
        before.tokens.get(i),
        after.tokens.get(i)
    )
}

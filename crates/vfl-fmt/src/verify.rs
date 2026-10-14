//! Checks that formatting changed nothing but layout.

use crate::lang::{Lang, Node};

/// What formatting must preserve: the significant tokens (ignoring case
/// where the language does, and the optional words after `end`, which the
/// formatter may add or drop) and the comments.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Fingerprint<L: Lang> {
    tokens: Vec<(L::Kind, String)>,
    comments: Vec<String>,
}

#[inline(always)]
fn comment_text(text: &str) -> String {
    if text.contains('\n') {
        text.to_owned()
    } else {
        text.trim_end().to_owned()
    }
}

pub(crate) fn fingerprint<L: Lang>(root: &Node<L>) -> Fingerprint<L> {
    let mut tokens = Vec::new();
    let mut comments = Vec::new();
    let mut in_end_tail = false;
    for token in root
        .descendants_with_tokens()
        .filter_map(rowan::NodeOrToken::into_token)
    {
        let kind = token.kind();
        if kind == L::WHITESPACE {
            continue;
        }
        if L::is_comment(kind) {
            comments.push(comment_text(token.text()));
            continue;
        }
        if in_end_tail && kind != L::SEMI {
            continue;
        }
        in_end_tail = L::starts_end_tail(&token);
        tokens.push((kind, L::normalize(kind, token.text())));
    }
    Fingerprint { tokens, comments }
}

/// The fingerprint of a run of whole lines, from its tokens alone: cheaper
/// than parsing the file again. Every `end` is taken to start an end
/// clause whose optional words may change.
pub(crate) fn lexical<L: Lang>(text: &str, std: L::Std<'_>) -> Fingerprint<L> {
    let mut tokens = Vec::new();
    let mut comments = Vec::new();
    let mut in_end_tail = false;
    let mut pos = 0usize;
    for (kind, len) in L::lex(text, std) {
        let text = &text[pos..pos + len];
        pos += len;
        if kind == L::WHITESPACE {
            continue;
        }
        if L::is_comment(kind) {
            comments.push(comment_text(text));
            continue;
        }
        if in_end_tail && kind != L::SEMI {
            continue;
        }
        in_end_tail = L::lexical_end_tail(kind);
        tokens.push((kind, L::normalize(kind, text)));
    }
    Fingerprint { tokens, comments }
}

/// Describes the first difference between two fingerprints.
pub(crate) fn first_difference<L: Lang>(before: &Fingerprint<L>, after: &Fingerprint<L>) -> String {
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

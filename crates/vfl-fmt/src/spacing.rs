//! Whether two VHDL elements on the same line are separated by a space.

use vfl_config::FormatConfig;
use vfl_syntax::SyntaxKind::{self, *};
use vfl_syntax::{Standard, Vhdl};

use crate::elems::Elem;

fn grandparent_kind(elem: &Elem<Vhdl>) -> Option<SyntaxKind> {
    elem.parent.as_ref()?.parent().map(|n| n.kind())
}

/// `+`, `-` used as a sign: no space after it.
fn is_sign(elem: &Elem<Vhdl>) -> bool {
    matches!(elem.kind, PLUS | MINUS) && elem.parent_kind() == Some(UNARY_EXPR)
}

#[inline]
fn in_external_path(elem: &Elem<Vhdl>) -> bool {
    elem.parent_kind() == Some(EXTERNAL_PATH)
}

/// Whether there was white space between two elements in the source.
#[inline]
fn spaced_in_source(prev: &Elem<Vhdl>, cur: &Elem<Vhdl>) -> bool {
    match (prev.src, cur.src) {
        (Some(p), Some(c)) => p.end() < c.start(),
        _ => false,
    }
}

/// Whether two tokens lex differently once written together: `s foo` in
/// a malformed external path, `- -a`.
#[cold]
fn would_join(prev: &Elem<Vhdl>, cur: &Elem<Vhdl>) -> bool {
    if !prev.is_token(prev.kind) || !cur.is_token(cur.kind) {
        return false;
    }
    let joined = format!("{}{}", prev.text, cur.text);
    let (tokens, _) = vfl_syntax::lex(&joined, Standard::Vhdl2019);
    !matches!(&tokens[..], [a, b] if a.kind == prev.kind && b.kind == cur.kind)
}

/// Whether a space goes between `prev` and `cur` when they share a line.
#[inline]
pub(crate) fn space_between(prev: &Elem<Vhdl>, cur: &Elem<Vhdl>, config: &FormatConfig) -> bool {
    // Tokens written apart stay apart where they would read as one.
    space_by_rule(prev, cur, config) || (spaced_in_source(prev, cur) && would_join(prev, cur))
}

#[inline]
fn space_by_rule(prev: &Elem<Vhdl>, cur: &Elem<Vhdl>, config: &FormatConfig) -> bool {
    if prev.is_comment() || cur.is_comment() {
        return true;
    }
    if prev.kind == PSL_DIRECTIVE || cur.kind == PSL_DIRECTIVE {
        return true;
    }
    if matches!(cur.kind, SEMI | COMMA | RPAREN | RBRACKET | TICK | QUESTION) {
        return false;
    }
    if matches!(prev.kind, LPAREN | LBRACKET | TICK | DOT) {
        return false;
    }
    if in_external_path(cur) {
        // `<< signal .top.dut.x : t >>`: one space before the path, none inside.
        return !in_external_path(prev);
    }
    if cur.is_token(COLON) && cur.parent_kind() == Some(LABEL) {
        return config.space_before_label_colon;
    }
    match cur.kind {
        DOT | LBRACKET => false,
        LPAREN => {
            if is_sign(prev) {
                return false;
            }
            match (cur.parent_kind(), grandparent_kind(cur)) {
                // Calls, indexing, slices, constraints: `f(x)`, `v(7 downto 0)`.
                (Some(ASSOCIATION_LIST), Some(CALL_EXPR)) => false,
                // `function f(x : t)`, but `procedure p parameter (x : t)`.
                (Some(INTERFACE_LIST), Some(FUNCTION_SPEC | PROCEDURE_SPEC))
                    if prev.kind != PARAMETER_KW =>
                {
                    false
                }
                _ if matches!(
                    prev.kind,
                    PROCESS_KW
                        | PORT_KW
                        | GENERIC_KW
                        | MAP_KW
                        | PARAMETER_KW
                        | ARRAY_KW
                        | BLOCK_KW
                        | IF_KW
                        | ELSIF_KW
                        | WHILE_KW
                        | UNTIL_KW
                ) =>
                {
                    config.space_after_keyword
                }
                _ => true,
            }
        }
        _ => !is_sign(prev),
    }
}

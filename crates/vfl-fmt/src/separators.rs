//! Moves the separators of lists to the end of a line or the start of the
//! next one, as `interface_separator` and `association_separator` say.
//!
//! Only the order of a separator and the comments next to it changes, and
//! only where the list breaks a line between two elements: `a, b` on one
//! line stays as it is. The line break moves with the separator, so that
//!
//! ```vhdl
//! a => x, -- note
//! b => y
//! ```
//!
//! becomes, with leading separators,
//!
//! ```vhdl
//! a => x -- note
//! , b => y
//! ```
//!
//! and back. Separators never cross a `vfl:` directive, a compiler
//! directive, or a region kept as written.

use rowan::NodeOrToken;
use vfl_config::{FormatConfig, Separator};

use crate::elems::Elem;
use crate::escape::is_directive;
use crate::lang::Lang;

pub(crate) fn place<L: Lang>(elems: &mut [Elem<L>], config: &FormatConfig, protected: &[bool]) {
    let n = elems.len();
    let crossable = |e: &Elem<L>| e.is_comment() && !L::keeps_column(e.kind) && !is_directive(e);
    let styles = [config.interface_separator, config.association_separator];
    let any_leading = styles.contains(&Separator::Leading);
    let any_trailing = styles.contains(&Separator::Trailing);
    let mut i = 0;
    while i < n {
        if !L::is_separator(elems[i].kind) {
            i += 1;
            continue;
        }
        // Most separators already sit where either style puts them: right
        // after the previous element and right before the next one.
        let after_code = i > 0 && !elems[i - 1].is_comment() && elems[i].src_newlines == 0;
        let before_code = elems
            .get(i + 1)
            .is_some_and(|e| !e.is_comment() && e.src_newlines == 0);
        let may_move = (!after_code && any_trailing) || (!before_code && any_leading);
        if !may_move || protected[i] || !elems[i].is_token(elems[i].kind) {
            i += 1;
            continue;
        }
        let Some(sep) = list_separator(&elems[i], config) else {
            i += 1;
            continue;
        };
        let breaks = |range: &[Elem<L>], last: usize| {
            range.iter().any(|e| e.src_newlines > 0)
                || range[..last].iter().any(|e| L::ends_line(e.kind))
        };
        match sep {
            Separator::Leading => {
                // Before the next element, after the comments in between.
                let mut next = i + 1;
                while next < n && crossable(&elems[next]) {
                    next += 1;
                }
                // An empty last element (`f(a,\n)`) has nothing to lead.
                if next >= n
                    || elems[next].is_comment()
                    || elems[next].kind == L::RPAREN
                    || L::keeps_column(elems[next].kind)
                    || protected[i..=next].iter().any(|&p| p)
                    || !breaks(&elems[i + 1..=next], next - i - 1)
                {
                    i += 1;
                    continue;
                }
                let before = elems[i].src_newlines;
                let after = elems[next].src_newlines;
                elems[next].src_newlines = 0;
                if next == i + 1 {
                    elems[i].src_newlines = before.max(after);
                } else {
                    elems[i + 1].src_newlines = elems[i + 1].src_newlines.max(before);
                    elems[i].src_newlines = after.max(1);
                    elems[i..next].rotate_left(1);
                }
                i = next + 1;
            }
            Separator::Trailing => {
                // Right after the previous element, before the comments in
                // between.
                let mut prev = i;
                while prev > 0 && crossable(&elems[prev - 1]) {
                    prev -= 1;
                }
                // An empty first element (`#(\n,)`) has nothing to trail.
                if prev == 0
                    || elems[prev - 1].is_comment()
                    || elems[prev - 1].kind == L::LPAREN
                    || L::keeps_column(elems[prev - 1].kind)
                    || protected[prev - 1..=i].iter().any(|&p| p)
                    || !breaks(&elems[prev..=i], i - prev)
                    || i + 1 >= n
                {
                    i += 1;
                    continue;
                }
                let before = std::mem::take(&mut elems[i].src_newlines);
                elems[i + 1].src_newlines = elems[i + 1].src_newlines.max(before);
                elems[prev..=i].rotate_right(1);
                i += 1;
            }
        }
    }
}

/// Where `sep` goes, when it separates the elements of a list.
fn list_separator<L: Lang>(sep: &Elem<L>, config: &FormatConfig) -> Option<Separator> {
    let list = sep.parent.as_ref().filter(|p| L::is_list(p.kind()))?;
    let open = list
        .children_with_tokens()
        .filter_map(NodeOrToken::into_token)
        .find(|t| t.kind() == L::LPAREN)?;
    L::opens_list(&open, list, config)?;
    Some(L::separator(list, config))
}

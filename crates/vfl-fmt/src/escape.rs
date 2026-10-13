//! `-- vfl: off`, `-- vfl: on` and `-- vfl: skip`: regions left as written.

use rowan::NodeOrToken;
use vfl_syntax::SyntaxKind::*;
use vfl_syntax::TextSize;

use crate::elems::Elem;
use crate::kinds::is_item;

#[derive(Debug, PartialEq, Eq)]
enum Directive {
    Off,
    On,
    Skip,
}

fn directive(elem: &Elem) -> Option<Directive> {
    if elem.kind != LINE_COMMENT {
        return None;
    }
    let body = elem.text.strip_prefix("--")?.trim().to_ascii_lowercase();
    let word = body.strip_prefix("vfl:")?.trim();
    match word {
        "off" => Some(Directive::Off),
        "on" => Some(Directive::On),
        "skip" => Some(Directive::Skip),
        _ => None,
    }
}

/// The elements of the regions kept as written: each is the range of
/// elements after a directive comment, and the source text it spans.
pub(crate) struct Region {
    /// The directive comment.
    pub directive: usize,
    /// The last element kept as written, or `None` for "to the end of the file".
    pub last: Option<usize>,
    /// The `-- vfl: on` comment that ends the region, if any.
    pub on: Option<usize>,
    /// Where the region ends in the source.
    pub src_end: usize,
}

/// Finds the regions protected by `-- vfl: off`/`on` and `-- vfl: skip`.
pub(crate) fn regions(elems: &[Elem], src_len: usize) -> Vec<Region> {
    let mut regions = Vec::new();
    let mut i = 0;
    while i < elems.len() {
        let Some(d) = directive(&elems[i]) else {
            i += 1;
            continue;
        };
        match d {
            Directive::Off => {
                let on =
                    (i + 1..elems.len()).find(|&j| directive(&elems[j]) == Some(Directive::On));
                let (last, src_end) = match on {
                    Some(j) => (
                        Some(j - 1),
                        elems[j].src.map_or(src_len, |r| r.start().into()),
                    ),
                    None => (None, src_len),
                };
                regions.push(Region {
                    directive: i,
                    last,
                    on,
                    src_end,
                });
                i = on.map_or(elems.len(), |j| j + 1);
            }
            Directive::Skip => {
                let found = (i + 1..elems.len())
                    .find(|&k| !elems[k].is_comment())
                    .and_then(|k| {
                        let end = item_end(&elems[k])?;
                        let m = (k..elems.len())
                            .find(|&m| elems[m].src.is_some_and(|r| r.end() == end))?;
                        Some((m, end))
                    });
                match found {
                    Some((m, end)) => {
                        regions.push(Region {
                            directive: i,
                            last: Some(m),
                            on: None,
                            src_end: end.into(),
                        });
                        i = m + 1;
                    }
                    None => i += 1,
                }
            }
            Directive::On => i += 1,
        }
    }
    regions
}

/// Marks the elements inside protected regions.
pub(crate) fn protected(elems: &[Elem], regions: &[Region]) -> Vec<bool> {
    let mut marks = vec![false; elems.len()];
    for region in regions {
        let last = region.last.unwrap_or(elems.len() - 1);
        for mark in &mut marks[region.directive + 1..=last.max(region.directive)] {
            *mark = true;
        }
        marks[region.directive] = false;
    }
    marks
}

/// Puts the source text back into the protected regions.
pub(crate) fn restore_verbatim(
    src: &str,
    out: String,
    elems: &[Elem],
    positions: &[(usize, usize)],
    regions: &[Region],
) -> String {
    if regions.is_empty() {
        return out;
    }
    let mut result = String::with_capacity(out.len());
    let mut last = 0;
    for region in regions {
        let Some(range) = elems[region.directive].src else {
            continue;
        };
        let out_start = positions[region.directive].1;
        let out_end = match (region.on, region.last) {
            (Some(on), _) => positions[on].0,
            (None, Some(m)) => positions[m].1,
            (None, None) => out.len(),
        };
        result.push_str(&out[last..out_start]);
        result.push_str(&src[usize::from(range.end())..region.src_end]);
        last = out_end;
    }
    result.push_str(&out[last..]);
    let trimmed = result.trim_end_matches(['\n', ' ', '\t']).len();
    result.truncate(trimmed);
    result.push('\n');
    result
}

/// Where the statement or declaration starting at `elem` ends.
fn item_end(elem: &Elem) -> Option<TextSize> {
    let syntax = elem.syntax.as_ref()?;
    let start = syntax.text_range().start();
    let mut node = match syntax {
        NodeOrToken::Node(n) => Some(n.clone()),
        NodeOrToken::Token(t) => t.parent(),
    };
    while let Some(n) = node.filter(|n| n.text_range().start() == start) {
        if is_item(&n) {
            return Some(n.text_range().end());
        }
        node = n.parent();
    }
    None
}

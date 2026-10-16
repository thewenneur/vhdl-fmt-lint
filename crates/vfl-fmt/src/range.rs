//! Formatting part of a file: a selection, or the line just typed.
//!
//! The whole file is formatted, then only the output lines that hold the
//! selected source lines are copied back, widened until the copy is
//! consistent on its own:
//!
//! - to the whole statement or declaration the selection starts or ends in
//!   (not past the construct around it: selecting `begin` does not
//!   reformat the whole process);
//! - to whole parenthesized lists, which may be laid out one element per
//!   line;
//! - to every line of an alignment run that one of the lines is part of, so
//!   the `:` of a port list or the `<=` of a block of assignments still
//!   line up (with `scope = "block"`, from the first to the last line of
//!   the run in its block; unchanged lines are then left out of the edits);
//! - to the comment lines just above, and from a comment line to the code
//!   line it is indented after;
//! - to lines that start and end at the same tokens in the source and in
//!   the output, since the formatter may join or split lines.
//!
//! Lines kept as written (`-- vfl: off`, `-- vfl: skip`) are never touched.
//! Formatting the same range again changes nothing.

use std::ops::Range;
use std::time::{Duration, Instant};

use rowan::{NodeOrToken, TextSize};
use vfl_config::{AlignScope, BlockBoundary, FormatConfig};
use vfl_syntax::{Standard, Vhdl};
use vfl_verilog_syntax::{Options, Verilog};

use crate::kinds::is_item;
use crate::lang::{Element, Lang, Node, Token};
use crate::verify::{first_difference, lexical};
use crate::{FormatError, Laid, Source, format_with, lay_out, parse_valid};

/// A change to a file: replace the bytes `range` of the original text by
/// `text`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edit {
    pub range: Range<usize>,
    pub text: String,
}

/// Applies edits that do not overlap, in any order.
pub fn apply_edits(text: &str, edits: &[Edit]) -> String {
    let mut sorted: Vec<&Edit> = edits.iter().collect();
    sorted.sort_by_key(|e| e.range.start);
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for edit in sorted {
        out.push_str(&text[last..edit.range.start]);
        out.push_str(&edit.text);
        last = edit.range.end;
    }
    out.push_str(&text[last..]);
    out
}

/// The one edit that turns `old` into `new`: the bytes between their common
/// prefix and common suffix, both on character boundaries. Editors keep the
/// cursor in place when a whole-file change comes as this edit.
pub fn minimal_edit(old: &str, new: &str) -> Edit {
    let mut prefix = old
        .bytes()
        .zip(new.bytes())
        .take_while(|(a, b)| a == b)
        .count();
    while !old.is_char_boundary(prefix) {
        prefix -= 1;
    }
    // The suffix may not overlap the prefix in either text.
    let mut suffix = old[prefix..]
        .bytes()
        .rev()
        .zip(new[prefix..].bytes().rev())
        .take_while(|(a, b)| a == b)
        .count();
    // Equal bytes on both sides, so a boundary in `old` is one in `new`.
    while !old.is_char_boundary(old.len() - suffix) {
        suffix -= 1;
    }
    Edit {
        range: prefix..old.len() - suffix,
        text: new[prefix..new.len() - suffix].to_owned(),
    }
}

/// How long [`minimal_edits`] looks for the smallest set of changed lines
/// before it settles for larger runs.
const DIFF_DEADLINE: Duration = Duration::from_millis(20);

/// Edits turning `old` into `new`, each trimmed to the bytes that differ:
/// one for each changed line, and one for each run of lines that were
/// joined, split, added or removed. Editors keep the cursor, the selection
/// and the scroll position when a whole-file change comes as these edits:
/// text outside them, including lines added or removed above the view,
/// keeps its place. Sorted, and they do not overlap.
///
/// Lines are matched by their text without spaces and case, since
/// formatting mostly reindents, realigns and recases them. That keeps the
/// diff small, and so fast, even when every line changed.
pub fn minimal_edits(old: &str, new: &str) -> Vec<Edit> {
    if old == new {
        return Vec::new();
    }
    let (old, new) = (Lines::new(old), Lines::new(new));
    let mut runs = Runs::default();
    let Ok(()) = similar::algorithms::patience::diff_deadline(
        &mut runs,
        &old.keys,
        0..old.keys.len(),
        &new.keys,
        0..new.keys.len(),
        Instant::now().checked_add(DIFF_DEADLINE),
    );
    let mut edits = Vec::new();
    let mut edit = |o: Range<usize>, n: Range<usize>| {
        let (o, n) = (old.bytes(o), new.bytes(n));
        let edit = minimal_edit(&old.text[o.clone()], &new.text[n]);
        if !edit.range.is_empty() || !edit.text.is_empty() {
            edits.push(Edit {
                range: o.start + edit.range.start..o.start + edit.range.end,
                text: edit.text,
            });
        }
    };
    // Between the runs, lines with the same key may still differ in spaces
    // or case.
    let mut at = (0, 0);
    for (o, n) in runs
        .runs
        .into_iter()
        .chain([(old.len()..old.len(), new.len()..new.len())])
    {
        for (a, b) in (at.0..o.start).zip(at.1..n.start) {
            if old.lines[a] != new.lines[b] {
                edit(a..a + 1, b..b + 1);
            }
        }
        if o.len() == n.len() {
            for (a, b) in o.clone().zip(n.clone()) {
                edit(a..a + 1, b..b + 1);
            }
        } else if !o.is_empty() || !n.is_empty() {
            edit(o.clone(), n.clone());
        }
        at = (o.end, n.end);
    }
    edits
}

/// The lines of a text, ending with their line break, where each starts,
/// and the key each is matched by.
struct Lines<'a> {
    text: &'a str,
    lines: Vec<&'a str>,
    starts: Vec<usize>,
    keys: Vec<u64>,
}

impl<'a> Lines<'a> {
    fn new(text: &'a str) -> Lines<'a> {
        let lines: Vec<&str> = text.split_inclusive('\n').collect();
        let mut starts = Vec::with_capacity(lines.len() + 1);
        let mut at = 0;
        for line in &lines {
            starts.push(at);
            at += line.len();
        }
        starts.push(at);
        let keys = lines.iter().map(|line| key(line)).collect();
        Lines {
            text,
            lines,
            starts,
            keys,
        }
    }

    fn len(&self) -> usize {
        self.lines.len()
    }

    fn bytes(&self, lines: Range<usize>) -> Range<usize> {
        self.starts[lines.start]..self.starts[lines.end]
    }
}

/// A hash (FNV-1a) of a line without its spaces, lowercased. A collision
/// only makes the edits larger.
fn key(line: &str) -> u64 {
    line.bytes()
        .filter(|b| !b.is_ascii_whitespace())
        .fold(0xcbf2_9ce4_8422_2325, |hash, b| {
            (hash ^ u64::from(b.to_ascii_lowercase())).wrapping_mul(0x0100_0000_01b3)
        })
}

/// The runs of changed lines of a diff, consecutive changes joined.
#[derive(Default)]
struct Runs {
    runs: Vec<(Range<usize>, Range<usize>)>,
    /// Whether the last run may still grow.
    open: bool,
}

impl Runs {
    fn change(&mut self, old: Range<usize>, new: Range<usize>) {
        match self.runs.last_mut() {
            // A replacement comes as a deletion and an insertion from the
            // same start.
            Some(last) if self.open => {
                last.0.end = last.0.end.max(old.end);
                last.1.end = last.1.end.max(new.end);
            }
            _ => {
                self.runs.push((old, new));
                self.open = true;
            }
        }
    }
}

impl similar::algorithms::DiffHook for Runs {
    type Error = std::convert::Infallible;

    fn equal(&mut self, _: usize, _: usize, _: usize) -> Result<(), Self::Error> {
        self.open = false;
        Ok(())
    }

    fn delete(&mut self, old: usize, len: usize, new: usize) -> Result<(), Self::Error> {
        self.change(old..old + len, new..new);
        Ok(())
    }

    fn insert(&mut self, old: usize, new: usize, len: usize) -> Result<(), Self::Error> {
        self.change(old..old, new..new + len);
        Ok(())
    }
}

/// Formats the lines of `text` that the byte range `range` touches: see
/// [`format_ranges`].
pub fn format_range(
    text: &str,
    range: Range<usize>,
    config: &FormatConfig,
    std: Standard,
) -> Result<Vec<Edit>, FormatError> {
    format_ranges(text, std::slice::from_ref(&range), config, std)
}

/// Formats the line just finished after `ch` was typed and the cursor is at
/// byte `cursor`: the line of a `;`, or after a new line the line above it.
/// The new line, and the indentation the editor gave it, are left alone.
/// Nothing when `on_type` is off, for other characters, or while the file
/// does not parse.
pub fn format_typed(
    text: &str,
    cursor: usize,
    ch: char,
    config: &FormatConfig,
    std: Standard,
) -> Result<Vec<Edit>, FormatError> {
    typed::<Vhdl>(text, cursor, ch, config, std)
}

/// [`format_typed`] for a Verilog or SystemVerilog file.
pub fn format_typed_verilog(
    text: &str,
    cursor: usize,
    ch: char,
    config: &FormatConfig,
    options: &Options,
) -> Result<Vec<Edit>, FormatError> {
    typed::<Verilog>(text, cursor, ch, config, *options)
}

fn typed<L: Lang>(
    text: &str,
    cursor: usize,
    ch: char,
    config: &FormatConfig,
    std: L::Std<'_>,
) -> Result<Vec<Edit>, FormatError> {
    if !config.on_type || cursor > text.len() || !text.is_char_boundary(cursor) {
        return Ok(Vec::new());
    }
    let line_start = text[..cursor].rfind('\n').map_or(0, |i| i + 1);
    match ch {
        ';' => ranges_with::<L>(
            text,
            std::slice::from_ref(&(cursor..cursor)),
            config,
            std,
            Mode::Typed,
        ),
        '\n' if line_start > 0 => {
            let above = line_start - 1;
            let mut edits = ranges_with::<L>(
                text,
                std::slice::from_ref(&(above..above)),
                config,
                std,
                Mode::Typed,
            )?;
            let line_end = text[cursor..].find('\n').map_or(text.len(), |i| cursor + i);
            edits.retain(|e| e.range.end < line_start || e.range.start > line_end);
            Ok(edits)
        }
        _ => Ok(Vec::new()),
    }
}

/// Formats the lines of `text` that the byte ranges `ranges` touch, and
/// returns the edits to make, sorted and not overlapping. An empty range
/// stands for its line. Lines the selection does not touch may change too
/// when they belong to the same statement or alignment run.
///
/// The edits keep the file's line endings, whatever `line_ending` says.
pub fn format_ranges(
    text: &str,
    ranges: &[Range<usize>],
    config: &FormatConfig,
    std: Standard,
) -> Result<Vec<Edit>, FormatError> {
    format_ranges_in(text, ranges, config, std, true)
}

/// [`format_ranges`], laying out the whole file when `!incremental`. For
/// tests: the edits must be the same.
#[doc(hidden)]
pub fn format_ranges_in(
    text: &str,
    ranges: &[Range<usize>],
    config: &FormatConfig,
    std: Standard,
    incremental: bool,
) -> Result<Vec<Edit>, FormatError> {
    ranges_with::<Vhdl>(text, ranges, config, std, Mode::new(incremental))
}

/// [`format_ranges`] for a Verilog or SystemVerilog file.
pub fn format_ranges_verilog(
    text: &str,
    ranges: &[Range<usize>],
    config: &FormatConfig,
    options: &Options,
) -> Result<Vec<Edit>, FormatError> {
    format_ranges_verilog_in(text, ranges, config, options, true)
}

/// [`format_ranges_verilog`], laying out the whole file when
/// `!incremental`. For tests.
#[doc(hidden)]
pub fn format_ranges_verilog_in(
    text: &str,
    ranges: &[Range<usize>],
    config: &FormatConfig,
    options: &Options,
    incremental: bool,
) -> Result<Vec<Edit>, FormatError> {
    ranges_with::<Verilog>(text, ranges, config, *options, Mode::new(incremental))
}

/// How much of the file range formatting lays out.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// The whole file, for tests.
    Whole,
    /// The part around the selection, growing to whole alignment blocks.
    Exact,
    /// As `Exact`, but on typing a block larger than [`TYPED_BLOCK`] is not
    /// laid out whole: the line aligns with the lines near it, and the
    /// next formatting of the file aligns the rest.
    Typed,
}

impl Mode {
    fn new(incremental: bool) -> Self {
        match incremental {
            true => Mode::Exact,
            false => Mode::Whole,
        }
    }
}

/// The largest block, in bytes, laid out whole for format on type.
const TYPED_BLOCK: usize = 16 * 1024;

fn ranges_with<L: Lang>(
    text: &str,
    ranges: &[Range<usize>],
    config: &FormatConfig,
    std: L::Std<'_>,
    mode: Mode,
) -> Result<Vec<Edit>, FormatError> {
    let incremental = mode != Mode::Whole;
    if ranges.is_empty() {
        return Ok(Vec::new());
    }
    // Lines are found by `\n`: a file that also breaks lines with a lone
    // `\r` is formatted whole.
    if text
        .match_indices('\r')
        .any(|(i, _)| text.as_bytes().get(i + 1) != Some(&b'\n'))
    {
        let out = format_with::<L>(text, config, std)?;
        return Ok(match out == text {
            true => Vec::new(),
            false => vec![Edit {
                range: 0..text.len(),
                text: out,
            }],
        });
    }
    let source = Source::new(text);
    let src = source.text.as_str();
    let root = parse_valid::<L>(src, std)?;
    let offsets = Offsets::new(text, &source);
    let whole = 0..src.len();
    let norm: Vec<(usize, usize)> = ranges
        .iter()
        .map(|r| {
            let (a, b) = (offsets.to_norm(r.start), offsets.to_norm(r.end));
            (a.min(b), a.max(b))
        })
        .collect();
    // With `scope = "block"`, a run reaches across its whole block: when
    // the lines to change hold a run whose block is not all laid out, the
    // part around that block is laid out instead.
    let block_scope = incremental && config.align.scope == AlignScope::Block;
    let mut around = (
        norm.iter().map(|r| r.0).min().unwrap_or(0),
        norm.iter().map(|r| r.1).max().unwrap_or(0),
    );
    let edits = loop {
        let window = match incremental {
            false => whole.clone(),
            true if block_scope => {
                window(&root, src, around.0, around.1, config).unwrap_or(whole.clone())
            }
            true => norm
                .iter()
                .map(|&(a, b)| window(&root, src, a, b, config).unwrap_or(whole.clone()))
                .reduce(|x, y| x.start.min(y.start)..x.end.max(y.end))
                .unwrap_or(whole.clone()),
        };
        let Some(laid) = lay_out(src, &root, config, std, Some(window.clone()))? else {
            return Ok(Vec::new());
        };
        let view = View::new(&laid, src);

        let mut spans: Vec<(usize, usize)> = norm
            .iter()
            .flat_map(|&(a, b)| view.selected(a, b))
            .filter_map(|(l0, l1)| view.widen(l0, l1))
            .collect();
        spans.sort_unstable();
        let mut merged: Vec<(usize, usize)> = Vec::new();
        for (l0, l1) in spans {
            match merged.last_mut() {
                Some(last) if l0 <= last.1 => last.1 = last.1.max(l1),
                _ => merged.push((l0, l1)),
            }
        }
        if block_scope {
            let needed = merged
                .iter()
                .flat_map(|&(l0, l1)| laid.runs.iter().filter(move |r| r.touches(l0, l1)))
                .map(|r| (r.block.0 as usize, r.block.1 as usize))
                .reduce(|x, y| (x.0.min(y.0), x.1.max(y.1)));
            if let Some((a, b)) = needed
                && (a < window.start || b > window.end)
                && (mode != Mode::Typed || b - a <= TYPED_BLOCK)
            {
                around = (around.0.min(a), around.1.max(b));
                continue;
            }
        }
        break merged
            .into_iter()
            .filter_map(|(l0, l1)| view.edit(l0, l1))
            .collect::<Vec<Edit>>();
    };
    // Only layout, case and the words after `end` may change.
    for edit in &edits {
        let before = lexical::<L>(&src[edit.range.clone()], std);
        let after = lexical::<L>(&edit.text, std);
        if before != after {
            return Err(FormatError::Internal(first_difference(&before, &after)));
        }
    }
    Ok(edits
        .into_iter()
        .filter_map(|e| unchanged_lines_out(src, e))
        .map(|e| Edit {
            range: offsets.to_orig(e.range.start)..offsets.to_orig(e.range.end),
            text: if source.crlf {
                e.text.replace('\n', "\r\n")
            } else {
                e.text
            },
        })
        .collect())
}

/// `edit` without the lines at its start and end that it leaves as they
/// are. `None` when nothing is left.
fn unchanged_lines_out(src: &str, edit: Edit) -> Option<Edit> {
    let old = &src[edit.range.clone()];
    let new = edit.text.as_str();
    let mut pre = 0;
    while let (Some(i), Some(j)) = (old[pre..].find('\n'), new[pre..].find('\n')) {
        if i != j || old[pre..pre + i] != new[pre..pre + i] {
            break;
        }
        pre += i + 1;
    }
    let mut suf = 0;
    loop {
        let (o, n) = (&old[pre..old.len() - suf], &new[pre..new.len() - suf]);
        match (o.rfind('\n'), n.rfind('\n')) {
            (Some(i), Some(j)) if o[i..] == n[j..] => suf += o.len() - i,
            _ => break,
        }
    }
    let (o, n) = (&old[pre..old.len() - suf], &new[pre..new.len() - suf]);
    (o != n).then(|| Edit {
        range: edit.range.start + pre..edit.range.end - suf,
        text: n.to_owned(),
    })
}

/// The part of the file that lays out the same alone as within the file,
/// around the lines of bytes `a..b`: whole lines between blank lines, which
/// end alignment runs, cut between the items of a construct. `None` when the
/// file has to be laid out whole.
fn window<L: Lang>(
    root: &Node<L>,
    src: &str,
    a: usize,
    b: usize,
    config: &FormatConfig,
) -> Option<Range<usize>> {
    if config.max_blank_lines == 0
        || (config.align.scope == AlignScope::Contiguous
            && config.align.block_boundary == BlockBoundary::Never)
    {
        return None;
    }
    // `-- vfl: off` may start a region far above.
    if src
        .as_bytes()
        .windows(4)
        .any(|w| w.eq_ignore_ascii_case(b"vfl:"))
    {
        return None;
    }
    let line_start = |i: usize| src[..i].rfind('\n').map_or(0, |k| k + 1);
    let line_end = |i: usize| src[i..].find('\n').map_or(src.len(), |k| i + k);
    let blank = |start: usize| src[start..line_end(start)].trim().is_empty();

    // Start: the first line after a blank line.
    let mut start = line_start(a.min(src.len()));
    loop {
        if start == 0 {
            break;
        }
        let prev = line_start(start - 1);
        // Comment lines above take their indentation from the code below.
        if blank(prev)
            && !blank(start)
            && cuts_between_items(root, src, start)
            && last_token_before(root, start).is_none_or(|t| !L::is_comment(t.kind()))
        {
            break;
        }
        start = prev;
    }
    // End: the last line before a blank line, not a comment, whose
    // indentation follows the code after it.
    let mut end = line_end(b.min(src.len()));
    loop {
        if end >= src.len() {
            end = src.len();
            break;
        }
        let next = end + 1;
        if blank(next)
            && !blank(line_start(end))
            && cuts_between_items(root, src, next)
            && first_token_after(root, src, line_start(end))
                .is_some_and(|t| !L::is_comment(t.kind()))
        {
            break;
        }
        end = line_end(next);
    }
    // The line break may end a token, as a directive continued by a `\`
    // at the end of the file: the window holds all of it.
    if let Some(token) = root
        .token_at_offset(TextSize::from(end as u32))
        .right_biased()
        .filter(|t| t.kind() != L::WHITESPACE)
    {
        end = end.max(usize::from(token.text_range().end()));
    }
    Some(start..end)
}

/// Whether the source can be cut at `at`, the start of a line: between
/// tokens, and outside everything but constructs holding items (and not in
/// the body of a record, physical or protected type that is shifted in
/// because its keyword starts a line).
fn cuts_between_items<L: Lang>(root: &Node<L>, src: &str, at: usize) -> bool {
    let starts_line = |token: Option<Token<L>>| {
        token.is_some_and(|t| {
            let start = usize::from(t.text_range().start());
            src[..start]
                .rsplit('\n')
                .next()
                .is_some_and(|before| before.trim().is_empty())
        })
    };
    let Some(next) = first_token_after(root, src, at) else {
        return true;
    };
    let start = next.text_range().start();
    if usize::from(start) < at {
        return false;
    }
    next.parent_ancestors()
        .filter(|n| n.text_range().start() < start)
        .all(|n| L::cuts_inside(&n, &starts_line))
}

/// The first token that is not whitespace at or after `at`, or the one
/// running over it.
fn first_token_after<L: Lang>(root: &Node<L>, src: &str, at: usize) -> Option<Token<L>> {
    if at >= src.len() {
        return None;
    }
    let mut token = root
        .token_at_offset(TextSize::from(at as u32))
        .right_biased()?;
    while token.kind() == L::WHITESPACE {
        token = next_token(&token)?;
    }
    Some(token)
}

/// The last token that is not whitespace before `at`.
fn last_token_before<L: Lang>(root: &Node<L>, at: usize) -> Option<Token<L>> {
    let mut token = root
        .token_at_offset(TextSize::from(at as u32))
        .left_biased()?;
    while token.kind() == L::WHITESPACE || usize::from(token.text_range().end()) > at {
        token = prev_token(&token)?;
    }
    Some(token)
}

/// The token after `token`, past empty nodes (an empty Verilog connection,
/// `.a(a), )`), which `SyntaxToken::next_token` stops at.
fn next_token<L: Lang>(token: &Token<L>) -> Option<Token<L>> {
    let mut element: Element<L> = token.clone().into();
    loop {
        let mut sibling = element.next_sibling_or_token();
        while let Some(s) = sibling {
            let found = match &s {
                NodeOrToken::Token(t) => Some(t.clone()),
                NodeOrToken::Node(n) => n
                    .descendants_with_tokens()
                    .find_map(NodeOrToken::into_token),
            };
            if found.is_some() {
                return found;
            }
            sibling = s.next_sibling_or_token();
        }
        element = element.parent()?.into();
    }
}

/// The token before `token`, past empty nodes.
fn prev_token<L: Lang>(token: &Token<L>) -> Option<Token<L>> {
    let mut element: Element<L> = token.clone().into();
    loop {
        let mut sibling = element.prev_sibling_or_token();
        while let Some(s) = sibling {
            let found = match &s {
                NodeOrToken::Token(t) => Some(t.clone()),
                NodeOrToken::Node(n) => n
                    .descendants_with_tokens()
                    .filter_map(NodeOrToken::into_token)
                    .last(),
            };
            if found.is_some() {
                return found;
            }
            sibling = s.prev_sibling_or_token();
        }
        element = element.parent()?.into();
    }
}

/// Converts between offsets in the original text and in the normalized one
/// (no byte order mark, `\n` line ends).
struct Offsets {
    bom: usize,
    /// Where each `\r` dropped from the original was, in the original text
    /// after the byte order mark.
    dropped: Vec<usize>,
    len: usize,
}

impl Offsets {
    fn new(text: &str, source: &Source) -> Self {
        let bom = source.bom.len();
        let dropped = if source.crlf {
            text[bom..].match_indices("\r\n").map(|(i, _)| i).collect()
        } else {
            Vec::new()
        };
        Offsets {
            bom,
            dropped,
            len: source.text.len(),
        }
    }

    fn to_norm(&self, orig: usize) -> usize {
        let o = orig.saturating_sub(self.bom);
        (o - self.dropped.partition_point(|&d| d < o)).min(self.len)
    }

    fn to_orig(&self, norm: usize) -> usize {
        // The i-th dropped `\r` sits before normalized offset `d - i`.
        let before = self
            .dropped
            .iter()
            .enumerate()
            .take_while(|&(i, &d)| d - i < norm)
            .count();
        self.bom + norm + before
    }
}

struct View<'a, L: Lang> {
    laid: &'a Laid<L>,
    src: &'a str,
    /// For each parenthesis, the element of the one matching it.
    partner: Vec<Option<usize>>,
    /// Lines to replace together because a separator moved between them:
    /// from the line of the element before it in the source to the line of
    /// the one after it.
    moved: Vec<(usize, usize)>,
}

impl<'a, L: Lang> View<'a, L> {
    fn new(laid: &'a Laid<L>, src: &'a str) -> Self {
        let mut partner = vec![None; laid.elems.len()];
        let mut open = Vec::new();
        for (i, elem) in laid.elems.iter().enumerate() {
            if elem.is_token(L::LPAREN) {
                open.push(i);
            } else if elem.is_token(L::RPAREN)
                && let Some(o) = open.pop()
            {
                partner[o] = Some(i);
                partner[i] = Some(o);
            }
        }
        let src_start = |k: usize| laid.elems.get(k)?.src.map(|r| usize::from(r.start()));
        let mut moved = Vec::new();
        for (e, elem) in laid.elems.iter().enumerate() {
            let Some(r) = elem.src else {
                continue;
            };
            let (start, end) = (usize::from(r.start()), usize::from(r.end()));
            let out_of_order = (e > 0 && src_start(e - 1).is_some_and(|s| s > start))
                || src_start(e + 1).is_some_and(|s| s < start);
            if !out_of_order {
                continue;
            }
            let before = (0..e).rev().find(|&k| {
                laid.elems[k]
                    .src
                    .is_some_and(|q| usize::from(q.end()) <= start)
            });
            let after = (e + 1..laid.elems.len()).find(|&k| src_start(k).is_some_and(|s| s >= end));
            moved.push((
                before.map_or(0, |k| laid.line_of[k]),
                after.map_or(laid.lines.len() - 1, |k| laid.line_of[k]),
            ));
        }
        View {
            laid,
            src,
            partner,
            moved,
        }
    }

    fn src_range(&self, e: usize) -> Option<Range<usize>> {
        self.laid.elems[e]
            .src
            .map(|r| usize::from(r.start())..usize::from(r.end()))
    }

    /// The chunks of output lines holding the source lines of `a..b`,
    /// split around lines kept as written.
    fn selected(&self, a: usize, b: usize) -> Vec<(usize, usize)> {
        let src = self.src;
        let (mut a, mut b) = (a.min(b), a.max(b));
        while !src.is_char_boundary(a) {
            a -= 1;
        }
        while !src.is_char_boundary(b) {
            b -= 1;
        }
        // A selection of whole lines ends at the start of the next one.
        let b = if b > a && src.as_bytes()[b - 1] == b'\n' {
            b - 1
        } else {
            b
        };
        let start = src[..a].rfind('\n').map_or(0, |i| i + 1);
        let end = src[b..].find('\n').map_or(src.len(), |i| b + i);
        let laid = self.laid;
        let mut chosen = (0..laid.elems.len()).filter(|&e| {
            self.src_range(e)
                .is_some_and(|r| r.start < end && r.end > start)
        });
        let Some(first) = chosen.next() else {
            return Vec::new();
        };
        let last = chosen.next_back().unwrap_or(first);
        let (l0, l1) = (laid.line_of[first], laid.line_of[last]);
        let mut chunks: Vec<(usize, usize)> = Vec::new();
        for li in l0..=l1 {
            if laid.lines[li].verbatim {
                continue;
            }
            match chunks.last_mut() {
                Some(chunk) if chunk.1 + 1 == li => chunk.1 = li,
                _ => chunks.push((li, li)),
            }
        }
        chunks
    }

    /// Widens output lines `l0..=l1` until they can replace their source
    /// lines on their own. `None` when that would take in a line kept as
    /// written.
    fn widen(&self, mut l0: usize, mut l1: usize) -> Option<(usize, usize)> {
        let laid = self.laid;
        let lines = &laid.lines;
        loop {
            let (old0, old1) = (l0, l1);
            if let Some((a, b)) = self.item_lines(lines[l0].start) {
                l0 = l0.min(a);
                l1 = l1.max(b);
            }
            if let Some((a, b)) = self.item_lines(lines[l1].end - 1) {
                l0 = l0.min(a);
                l1 = l1.max(b);
            }
            for run in &laid.runs {
                if run.touches(l0, l1) {
                    l0 = l0.min(run.first);
                    l1 = l1.max(run.last);
                }
            }
            for &(a, b) in &self.moved {
                if a <= l1 && b >= l0 {
                    l0 = l0.min(a);
                    l1 = l1.max(b);
                }
            }
            // Lists are laid out whole.
            for e in lines[l0].start..lines[l1].end {
                if let Some(p) = self.partner[e] {
                    l0 = l0.min(laid.line_of[p]);
                    l1 = l1.max(laid.line_of[p]);
                }
            }
            // A comment line is indented after the code below it, and where
            // that code was in the source.
            while l0 > 0 && lines[l0 - 1].comment_only && !lines[l0 - 1].verbatim {
                l0 -= 1;
            }
            if let Some(next) = lines[l1]
                .follows
                .filter(|&next| lines[l1].comment_only && !lines[next].verbatim)
            {
                l1 = l1.max(next);
            }
            while l0 > 0 && !self.starts_src_line(lines[l0].start) {
                l0 -= 1;
            }
            while l1 + 1 < lines.len() && !self.ends_src_line(lines[l1].end - 1) {
                l1 += 1;
            }
            if lines[l0..=l1].iter().any(|l| l.verbatim) {
                return None;
            }
            if (l0, l1) == (old0, old1) {
                return Some((l0, l1));
            }
        }
    }

    /// The output lines of the statement or declaration around element `e`,
    /// if it is not itself a construct holding others.
    fn item_lines(&self, e: usize) -> Option<(usize, usize)> {
        let laid = self.laid;
        // A comment between items belongs to none of them.
        if laid.elems[e].is_comment() {
            return None;
        }
        let syntax = laid.elems[e].syntax.as_ref()?;
        let mut node: Option<Node<L>> = match syntax {
            NodeOrToken::Node(n) => Some(n.clone()),
            NodeOrToken::Token(t) => t.parent(),
        };
        while let Some(n) = node {
            if L::is_container(n.kind()) {
                return None;
            }
            if is_item(&n) {
                // A type declaration holding a record or protected body is
                // a construct too.
                if n.children().any(|c| L::is_container(c.kind())) {
                    return None;
                }
                let range = n.text_range();
                let (start, end) = (usize::from(range.start()), usize::from(range.end()));
                let in_node = |k: &usize| {
                    self.src_range(*k)
                        .is_some_and(|r| r.start >= start && r.end <= end)
                };
                let first = (0..=e)
                    .rev()
                    .take_while(|k| in_node(k) || self.src_range(*k).is_none())
                    .last()?;
                let last = (e..laid.elems.len())
                    .take_while(|k| in_node(k) || self.src_range(*k).is_none())
                    .last()?;
                return Some((laid.line_of[first], laid.line_of[last]));
            }
            node = n.parent();
        }
        None
    }

    /// Whether element `e` is the first one on its source line.
    fn starts_src_line(&self, e: usize) -> bool {
        let Some(range) = self.src_range(e) else {
            return false;
        };
        match (0..e).rev().find_map(|k| self.src_range(k)) {
            // A separator moved past comments is out of source order.
            Some(prev) => self
                .src
                .get(prev.end..range.start)
                .is_some_and(|between| between.contains('\n')),
            None => true,
        }
    }

    /// Whether element `e` is the last one on its source line.
    fn ends_src_line(&self, e: usize) -> bool {
        let Some(end) = (0..=e).rev().find_map(|k| self.src_range(k)).map(|r| r.end) else {
            return false;
        };
        match (e + 1..self.laid.elems.len()).find_map(|k| self.src_range(k)) {
            Some(next) => self
                .src
                .get(end..next.start)
                .is_some_and(|between| between.contains('\n')),
            None => true,
        }
    }

    /// The edit replacing the source lines of output lines `l0..=l1`, in
    /// normalized offsets. `None` when nothing changes.
    fn edit(&self, l0: usize, l1: usize) -> Option<Edit> {
        let laid = self.laid;
        let (first, last) = (laid.lines[l0].start, laid.lines[l1].end - 1);
        let src_first = (first..=last).find_map(|k| self.src_range(k))?;
        let src_last = (first..=last).rev().find_map(|k| self.src_range(k))?;
        let start = self.src[..src_first.start].rfind('\n').map_or(0, |i| i + 1);
        let end = src_last.end
            + self.src[src_last.end..]
                .find(|c| c != ' ' && c != '\t')
                .unwrap_or(self.src.len() - src_last.end);
        let pos = laid.positions[first].0;
        let out_start = laid.raw[..pos].rfind('\n').map_or(0, |i| i + 1);
        let out_end = laid.positions[last].1;
        let text = &laid.raw[out_start..out_end];
        (text != &self.src[start..end]).then(|| Edit {
            range: start..end,
            text: text.to_owned(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(old: &str, new: &str) -> Edit {
        let edit = minimal_edit(old, new);
        assert_eq!(apply_edits(old, std::slice::from_ref(&edit)), new);
        edit
    }

    #[test]
    fn minimal_edit_covers_only_the_change() {
        let edit = check("a := 1;\nb := 2;\n", "a := 1;\nb  := 2;\n");
        assert_eq!(edit.range, 10..10);
        assert_eq!(edit.text, " ");
        let edit = check("abc", "abc");
        assert_eq!((edit.range, edit.text.as_str()), (3..3, ""));
        assert_eq!(check("", "x").range, 0..0);
        assert_eq!(check("xyz", "").range, 0..3);
    }

    fn check_all(old: &str, new: &str) -> Vec<Edit> {
        let edits = minimal_edits(old, new);
        assert_eq!(apply_edits(old, &edits), new);
        assert!(edits.windows(2).all(|w| w[0].range.end <= w[1].range.start));
        edits
    }

    #[test]
    fn minimal_edits_leave_unchanged_lines_alone() {
        assert!(check_all("a\nb\n", "a\nb\n").is_empty());
        // A line removed at the top and an indent changed at the bottom are
        // two edits; the lines between are not replaced.
        let old = "x\n\n\na;\nb;\nc;\n  d;\n";
        let new = "x\n\na;\nb;\nc;\nd;\n";
        let edits = check_all(old, new);
        assert_eq!(edits.len(), 2, "{edits:?}");
        assert_eq!((edits[0].range.len(), edits[0].text.as_str()), (1, ""));
        assert_eq!(edits[1].range, 13..15);
        // Each changed line of a run of the same length is its own edit.
        let edits = check_all("a  := 1;\nbb :=2;\n", "a := 1;\nbb := 2;\n");
        assert_eq!(edits.len(), 2, "{edits:?}");
        assert_eq!(edits[0].range, 2..3);
        assert_eq!(
            (edits[1].range.clone(), edits[1].text.as_str()),
            (14..14, " ")
        );
    }

    #[test]
    fn minimal_edits_join_split_and_end_lines() {
        check_all("a(\n  b,\n  c);\nz;\n", "a(b, c);\nz;\n");
        check_all("a(b, c);\nz;\n", "a(\n  b,\n  c);\nz;\n");
        check_all("end;", "end;\n");
        check_all("a\r\nb\r\n", "a\nb\n");
        check_all("é\nè\n", "è\né\n");
        check_all("", "x\n");
        check_all("x\n", "");
    }

    #[test]
    fn minimal_edit_prefix_and_suffix_do_not_overlap() {
        // "aa" -> "aaa": the common prefix and suffix are 2 bytes each.
        let edit = check("aa", "aaa");
        assert_eq!((edit.range, edit.text.as_str()), (2..2, "a"));
        let edit = check("abab", "ab");
        assert_eq!((edit.range, edit.text.as_str()), (2..4, ""));
    }

    #[test]
    fn minimal_edit_stays_on_char_boundaries() {
        // "é" (c3 a9) and "è" (c3 a8) share their first byte.
        let edit = check("x é y", "x è y");
        assert_eq!((edit.range, edit.text.as_str()), (2..4, "è"));
        // "ä" (c3 a4) and "Ĥ" (c4 a4) share their last byte.
        let edit = check("ä", "Ĥ");
        assert_eq!((edit.range, edit.text.as_str()), (0..2, "Ĥ"));
        check("\u{3000}a\u{a0}", "\u{3000}\u{a0}\u{a0}");
        check("--  é", "-- é");
    }
}

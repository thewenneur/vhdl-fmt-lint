//! Line breaks, indentation and list layout.

use std::collections::HashMap;

use rowan::{NodeOrToken, TextSize};
use vfl_config::{
    CaseAlternative, CloseParen, FormatConfig, ListLayout, OperatorPosition, Separator,
};

use crate::elems::Elem;
use crate::kinds::is_item;
use crate::lang::{Lang, Node};

/// A parenthesized list that can be laid out one element per line.
#[derive(Debug)]
pub(crate) struct List {
    pub open: usize,
    pub close: Option<usize>,
    /// The `,` or `;` between elements.
    pub seps: Vec<usize>,
    pub layout: ListLayout,
    /// Whether the source already spreads it over several lines.
    pub src_multiline: bool,
    /// Whether its separators start lines rather than end them.
    pub leading: bool,
    /// How many parentheses its `(` is in.
    pub depth: i32,
}

/// What [`Lang::breakable_operator`] tells of an operator.
pub(crate) struct Operator<L: Lang> {
    /// How loosely it binds: the chains of the lowest class break first.
    pub class: u8,
    /// Whether the line always breaks after it, whatever
    /// `operator_position` says (`else` of a conditional assignment).
    pub after_only: bool,
    /// The node of its chain: the operators of the same class in a row
    /// (`a and b and c`), which break together.
    pub chain: Node<L>,
}

/// The operators of an expression that break together when its line is
/// too long.
#[derive(Debug)]
pub(crate) struct Chain {
    pub ops: Vec<usize>,
    pub class: u8,
    pub after_only: bool,
    /// How many parentheses its operators are in.
    pub depth: i32,
    /// The last element of the expression.
    pub end: usize,
    /// The list whose parentheses it is right inside (`f(a + b)`).
    pub list: Option<usize>,
}

/// The lists and the chains of operators laid out over several lines.
pub(crate) struct Expanded {
    pub lists: Vec<bool>,
    pub chains: Vec<bool>,
}

/// Facts about each element that do not depend on the line breaks.
pub(crate) struct Analysis<L: Lang> {
    /// First token of a statement, declaration, clause or design unit.
    pub item_start: Vec<bool>,
    /// `begin`, `end`, `else`, `elsif` of a construct.
    pub section: Vec<bool>,
    /// Keywords at their construct's indentation when they start a line.
    pub flush: Vec<bool>,
    /// The `when` starting a case alternative.
    pub alternative_start: Vec<bool>,
    /// First token of a case alternative's statement, when it is the only
    /// one and a simple one: it may stay on the `when` line.
    pub alternative_body: Vec<bool>,
    /// First token of a statement after its label (`lbl :` on the line before).
    pub after_label: Vec<bool>,
    /// For `record`, `units` and `protected`: the last element of the type
    /// definition they open. When they start a line, its body moves in too.
    pub shift_end: Vec<Option<usize>>,
    /// For elements inside a conditional or selected assignment's values:
    /// the first element of those values.
    pub waveform_start: Vec<Option<usize>>,
    /// Indentation level of the element's construct.
    pub level: Vec<u32>,
    /// Whether a space separates the element from the previous one.
    pub space: Vec<bool>,
    /// The list each `(` opens.
    pub opens: Vec<Option<usize>>,
    /// For the separators of lists whose separators start lines: the list.
    /// Empty when no list has them.
    leading_sep: Vec<Option<usize>>,
    pub lists: Vec<List>,
    /// The first element of each element of a list, and the leading
    /// separators: the lines inside a list starting elsewhere continue an
    /// element.
    pub list_item_start: Vec<bool>,
    pub chains: Vec<Chain>,
    /// The chain of each operator.
    pub chain_of: Vec<Option<usize>>,
    /// Source offset of each element to its index.
    pub index: HashMap<TextSize, usize>,
    _lang: std::marker::PhantomData<L>,
}

impl<L: Lang> Analysis<L> {
    /// For a separator that starts lines: its list.
    #[inline]
    pub fn leading_list(&self, i: usize) -> Option<usize> {
        self.leading_sep.get(i).copied().flatten()
    }
}

pub(crate) fn analyze<L: Lang>(elems: &[Elem<L>], config: &FormatConfig) -> Analysis<L> {
    let n = elems.len();
    let mut an = Analysis {
        item_start: vec![false; n],
        section: vec![false; n],
        flush: vec![false; n],
        alternative_start: vec![false; n],
        alternative_body: vec![false; n],
        after_label: vec![false; n],
        shift_end: vec![None; n],
        waveform_start: vec![None; n],
        level: vec![0; n],
        space: vec![false; n],
        opens: vec![None; n],
        leading_sep: if config.interface_separator == Separator::Leading
            || config.association_separator == Separator::Leading
        {
            vec![None; n]
        } else {
            Vec::new()
        },
        lists: Vec::new(),
        list_item_start: vec![false; n],
        chains: Vec::new(),
        chain_of: vec![None; n],
        index: HashMap::with_capacity(n),
        _lang: std::marker::PhantomData,
    };
    let mut list_of_node: HashMap<Node<L>, usize> = HashMap::new();
    let mut chain_nodes: Vec<Node<L>> = Vec::new();
    let mut chain_of_node: HashMap<Node<L>, usize> = HashMap::new();
    // How many parentheses each element is in, and the innermost `(`.
    let mut depth = Vec::with_capacity(n);
    let mut inside = Vec::with_capacity(n);
    let mut open: Vec<usize> = Vec::new();
    for (i, elem) in elems.iter().enumerate() {
        if elem.is_token(L::RPAREN) {
            open.pop();
        }
        depth.push(open.len() as i32);
        inside.push(open.last().copied());
        if elem.is_token(L::LPAREN) {
            open.push(i);
        }
    }
    let mut shifted: Vec<(usize, Node<L>)> = Vec::new();
    let mut prev_code: Option<usize> = None;
    for (i, elem) in elems.iter().enumerate() {
        if i > 0 {
            an.space[i] = L::space_between(&elems[i - 1], elem, config);
        }
        if let Some(p) = prev_code {
            an.after_label[i] = L::ends_label(&elems[p]);
        }
        if !elem.is_comment() {
            prev_code = Some(i);
        }
        let Some(syntax) = &elem.syntax else {
            continue;
        };
        let start = syntax.text_range().start();
        an.index.insert(start, i);

        // The constructs that begin with this element.
        let mut node = match syntax {
            NodeOrToken::Node(n) => Some(n.clone()),
            NodeOrToken::Token(t) => t.parent(),
        };
        while let Some(n) = node {
            if n.text_range().start() != start {
                break;
            }
            if is_item(&n) {
                an.item_start[i] = true;
                if let Some(parent) = n.parent().filter(|p| L::is_alternative(p.kind())) {
                    let mut statements = parent.children().filter(|c| L::is_item_kind(c.kind()));
                    let only = statements.next().filter(|_| statements.next().is_none());
                    if only.as_ref() == Some(&n) && L::is_simple_statement(n.kind()) {
                        an.alternative_body[i] = true;
                    }
                }
            }
            if L::is_alternative(n.kind()) {
                an.alternative_start[i] = true;
            }
            node = n.parent();
        }

        // Each container adds a level for the items inside it.
        let mut level = 0;
        let mut child = syntax.clone();
        let mut ancestor = child.parent();
        while let Some(a) = ancestor {
            if L::is_container(a.kind())
                && let NodeOrToken::Node(c) = &child
                && L::is_item_kind(c.kind())
            {
                level += 1;
            }
            ancestor = a.parent();
            child = NodeOrToken::Node(a);
        }
        an.level[i] = level;

        let NodeOrToken::Token(token) = syntax else {
            continue;
        };
        let Some(parent) = token.parent() else {
            continue;
        };
        an.section[i] = L::is_section_keyword(token.kind(), parent.kind());
        an.flush[i] = an.section[i] || L::is_flush_keyword(token.kind(), parent.kind());
        if config.align.conditional_continuation && !elem.is_comment() {
            let waveform = parent
                .ancestors()
                .take_while(|n| !is_item(n))
                .find(|n| L::is_hanging(n.kind()));
            if let Some(w) = waveform {
                let first = w.text_range().start();
                if first != start {
                    an.waveform_start[i] = an.index.get(&first).copied();
                }
            }
        }
        let opens_body = L::opens_body(token, &parent);
        if opens_body {
            shifted.push((i, parent.clone()));
        }
        let kind = token.kind();
        if kind == L::LPAREN {
            if let Some(layout) = L::opens_list(token, &parent, config) {
                an.opens[i] = Some(an.lists.len());
                let leading = !an.leading_sep.is_empty()
                    && L::separator(&parent, config) == Separator::Leading;
                list_of_node.insert(parent, an.lists.len());
                an.lists.push(List {
                    open: i,
                    close: None,
                    seps: Vec::new(),
                    layout,
                    src_multiline: false,
                    leading,
                    depth: depth[i],
                });
            }
            continue;
        }
        if let Some(op) = L::breakable_operator(token, &parent) {
            let id = *chain_of_node.entry(op.chain.clone()).or_insert_with(|| {
                chain_nodes.push(op.chain);
                an.chains.push(Chain {
                    ops: Vec::new(),
                    class: op.class,
                    after_only: op.after_only,
                    depth: depth[i],
                    end: i,
                    list: inside[i].and_then(|p| an.opens[p]),
                });
                an.chains.len() - 1
            });
            an.chains[id].ops.push(i);
            an.chain_of[i] = Some(id);
        }
        if L::is_separator(kind) {
            if let Some(&id) = list_of_node.get(&parent) {
                an.lists[id].seps.push(i);
                if an.lists[id].leading {
                    an.leading_sep[i] = Some(id);
                }
            }
        } else if kind == L::RPAREN
            && let Some(&id) = list_of_node.get(&parent)
            && parent.last_token().as_ref() == Some(token)
        {
            an.lists[id].close = Some(i);
        }
    }
    for (i, node) in shifted {
        // The last token still there: an `end` name may have been dropped.
        let mut tokens: Vec<_> = node
            .descendants_with_tokens()
            .filter_map(NodeOrToken::into_token)
            .collect();
        tokens.reverse();
        an.shift_end[i] = tokens
            .iter()
            .find_map(|t| an.index.get(&t.text_range().start()).copied());
    }
    for (chain, node) in an.chains.iter_mut().zip(chain_nodes) {
        if let Some(last) = node.last_token()
            && let Some(&end) = an.index.get(&last.text_range().start())
        {
            chain.end = end;
        }
    }
    let next_code = |i: usize| (i + 1..n).find(|&j| !elems[j].is_comment());
    for list in &mut an.lists {
        let end = list.close.unwrap_or(list.open);
        list.src_multiline = elems[list.open + 1..=end]
            .iter()
            .any(|e| e.src_newlines > 0);
        for &at in std::iter::once(&list.open).chain(&list.seps) {
            if list.leading && at != list.open {
                an.list_item_start[at] = true;
            }
            if let Some(j) = next_code(at) {
                an.list_item_start[j] = true;
            }
        }
    }
    an
}

/// Which lists start out laid out one element per line. No chain of
/// operators does.
pub(crate) fn initially_expanded<L: Lang>(an: &Analysis<L>) -> Expanded {
    Expanded {
        lists: an
            .lists
            .iter()
            .map(|list| match list.layout {
                ListLayout::Expand => true,
                ListLayout::Auto => list.src_multiline,
                ListLayout::Preserve => false,
            })
            .collect(),
        chains: vec![false; an.chains.len()],
    }
}

/// The number of line breaks before each element: 0 keeps it on the
/// previous element's line, 2 or more leaves blank lines.
pub(crate) fn breaks<L: Lang>(
    elems: &[Elem<L>],
    an: &Analysis<L>,
    expanded: &Expanded,
    config: &FormatConfig,
) -> Vec<u32> {
    let n = elems.len();
    let max = config.max_blank_lines + 1;
    let mut forced = vec![false; n];
    for i in 0..n {
        let keep_on_when_line = config.case_alternative == CaseAlternative::Preserve
            && an.alternative_body[i]
            && elems[i].src_newlines == 0;
        if (an.item_start[i] || an.section[i] || L::starts_line(elems[i].kind))
            && !keep_on_when_line
        {
            forced[i] = true;
        }
    }
    // The element after `i`, skipping comments that stay on `i`'s line.
    let next =
        |i: usize| (i + 1..n).find(|&j| !(elems[j].is_comment() && elems[j].src_newlines == 0));
    for (list, _) in an.lists.iter().zip(&expanded.lists).filter(|(_, e)| **e) {
        if let Some(j) = next(list.open) {
            forced[j] = true;
        }
        for (k, &sep) in list.seps.iter().enumerate() {
            if list.leading {
                // The break goes before the separator, which the element
                // follows on its line. An empty element (`f(a,, b)`) leaves
                // two separators together.
                let joins = elems[sep].src_newlines == 0
                    && elems
                        .get(sep + 1)
                        .is_some_and(|e| L::joins_previous_element(e));
                let empty = k > 0 && list.seps[k - 1] + 1 == sep;
                forced[sep] |= !joins && !empty;
                continue;
            }
            // An empty element (`f(a,, b)`) keeps its separator next to the
            // one before it, which a line break there is never kept before.
            if let Some(j) = next(sep).filter(|&j| {
                Some(j) != list.close
                    && list.seps.get(k + 1) != Some(&j)
                    && !(elems[j].src_newlines == 0 && L::joins_previous_element(&elems[j]))
            }) {
                forced[j] = true;
            }
        }
        if let Some(close) = list.close
            && config.close_paren == CloseParen::OwnLine
        {
            forced[close] = true;
        }
    }
    let position = config.operator_position;
    for (chain, _) in an.chains.iter().zip(&expanded.chains).filter(|(_, e)| **e) {
        for &op in &chain.ops {
            let after = next(op);
            let before = !chain.after_only
                && match position {
                    OperatorPosition::Leading => true,
                    OperatorPosition::Trailing => false,
                    // Written in the source on either side: kept there.
                    OperatorPosition::Preserve => {
                        elems[op].src_newlines > 0
                            || after.is_some_and(|j| elems[j].src_newlines > 0)
                    }
                };
            if before {
                forced[op] |= position != OperatorPosition::Preserve;
            } else if let Some(j) = after {
                forced[j] = true;
            }
        }
    }
    let mut nl = Vec::with_capacity(n);
    for (i, elem) in elems.iter().enumerate() {
        let mut count = elem.src_newlines.min(max);
        if i == 0 || L::stays_on_line(elems, i) {
            count = 0;
        } else if forced[i]
            || L::ends_line(elems[i - 1].kind)
                // A comment after a directive stays on its line.
                && !(L::keeps_column(elems[i - 1].kind) && count == 0 && elem.is_comment())
        {
            count = count.max(1);
        }
        nl.push(count);
    }
    if position != OperatorPosition::Preserve {
        place_operators(elems, an, &forced, position, &mut nl);
    }
    if !config.blank_lines.is_empty() {
        blank_line_rules(elems, an, config, &mut nl);
    }
    nl
}

/// Applies `[format.blank_lines]` to the line breaks.
fn blank_line_rules<L: Lang>(
    elems: &[Elem<L>],
    an: &Analysis<L>,
    config: &FormatConfig,
    nl: &mut [u32],
) {
    for i in 1..elems.len() {
        if nl[i] == 0 {
            continue;
        }
        let Some((rule, above_comments)) = L::blank_line_rule(elems, an, i, &config.blank_lines)
        else {
            continue;
        };
        let mut at = i;
        // The comment lines right above, with no blank line in between.
        while above_comments && nl[at] == 1 {
            let Some(start) = (0..at).rev().find(|&k| nl[k] > 0 || k == 0) else {
                break;
            };
            if start == 0 || !elems[start..at].iter().all(Elem::is_comment) {
                break;
            }
            at = start;
        }
        if at > 0 && nl[at] > 0 {
            nl[at] = rule.apply(nl[at] - 1) + 1;
        }
    }
}

/// Moves the line breaks written next to operators to the side
/// `position` puts them on. A break with a comment on the other side
/// stays.
fn place_operators<L: Lang>(
    elems: &[Elem<L>],
    an: &Analysis<L>,
    forced: &[bool],
    position: OperatorPosition,
    nl: &mut [u32],
) {
    for chain in an.chains.iter().filter(|c| !c.after_only) {
        for &op in &chain.ops {
            let (Some(prev), Some(next)) = (op.checked_sub(1), elems.get(op + 1)) else {
                continue;
            };
            if elems[prev].is_comment() || next.is_comment() {
                continue;
            }
            let (from, to) = match position {
                OperatorPosition::Leading => (op + 1, op),
                _ => (op, op + 1),
            };
            if nl[from] > 0 && !forced[from] {
                nl[to] = nl[to].max(nl[from]);
                nl[from] = 0;
            }
        }
    }
}

/// An output line: a run of elements.
#[derive(Debug, Clone)]
pub(crate) struct Line {
    pub start: usize,
    pub end: usize,
    /// Indentation in columns.
    pub indent: u32,
    /// For a line starting with a leading separator: how far left of the
    /// elements' indentation it starts.
    pub hanging: u32,
    pub blank_before: u32,
    /// Holds nothing but comments.
    pub comment_only: bool,
    /// Inside a region kept as written (`-- vfl: off`, `-- vfl: skip`).
    pub verbatim: bool,
    /// For a continuation line of conditional or selected values: the
    /// first value, which the line should start under.
    pub hang: Option<usize>,
    /// The line whose indentation this one follows: the one that opened
    /// the parentheses it is in, or for a comment line, the next code line.
    pub follows: Option<usize>,
}

/// Splits the elements into lines and indents each line.
pub(crate) fn lines<L: Lang>(
    elems: &[Elem<L>],
    an: &Analysis<L>,
    nl: &[u32],
    config: &FormatConfig,
) -> (Vec<Line>, Vec<usize>) {
    let n = elems.len();
    let indent = config.indent;
    let continuation = config.continuation_indent.unwrap_or(indent);
    let mut lines: Vec<Line> = Vec::new();
    let mut line_of = vec![0; n];
    let mut indented: Vec<bool> = Vec::new();
    // The indentation each element would get if it started a line.
    let mut would = vec![0u32; n];
    // Lines holding the `(` not closed yet, and the list each opens.
    let mut open: Vec<usize> = Vec::new();
    let mut open_list: Vec<Option<usize>> = Vec::new();
    // Ends of the type bodies shifted in because their keyword starts a line.
    let mut shifts: Vec<usize> = Vec::new();
    for i in 0..n {
        if i == 0 || nl[i] > 0 {
            lines.push(Line {
                start: i,
                end: i,
                indent: 0,
                hanging: 0,
                blank_before: nl[i].saturating_sub(1),
                comment_only: true,
                verbatim: false,
                hang: None,
                follows: None,
            });
            indented.push(false);
        }
        let li = lines.len() - 1;
        line_of[i] = li;
        lines[li].end = i + 1;
        let elem = &elems[i];
        if elem.is_comment() {
            continue;
        }
        lines[li].comment_only = false;
        if an.item_start[i] {
            open.clear();
            open_list.clear();
        }
        while shifts.last().is_some_and(|&end| end < i) {
            shifts.pop();
        }
        let closes = elem.is_token(L::RPAREN);
        would[i] = match open.last() {
            Some(&ol) if closes => lines[ol].indent + lines[ol].hanging,
            Some(&ol) => {
                // A line continuing an element of a list goes further in
                // than the elements.
                let continues =
                    open_list.last().is_some_and(Option::is_some) && !an.list_item_start[i];
                lines[ol].indent
                    + lines[ol].hanging
                    + indent
                    + if continues { continuation } else { 0 }
            }
            None => {
                let (nested, starts_body) = if L::NESTS_BODIES && !indented[li] {
                    body_nesting(elem, an, &line_of)
                } else {
                    (0, false)
                };
                let starts_construct =
                    an.item_start[i] || an.flush[i] || an.after_label[i] || starts_body;
                (an.level[i] + nested) * indent
                    + if starts_construct { 0 } else { continuation }
                    + shifts.len() as u32 * continuation
            }
        };
        if !indented[li] {
            // When the first element of the list starts a line too, a
            // leading separator hangs left of the elements so that they
            // line up.
            let first_starts_line = |id: usize| {
                let open = an.lists[id].open;
                (open + 1..i)
                    .find(|&k| !elems[k].is_comment())
                    .is_some_and(|k| line_of[k] != line_of[open])
            };
            let hanging = if let Some(id) = an.leading_list(i)
                && lines[li].start == i
                && i + 1 < n
                && first_starts_line(id)
            {
                (elems[i].width() as u32 + u32::from(an.space[i + 1])).min(would[i])
            } else {
                0
            };
            lines[li].hanging = hanging;
            lines[li].indent = would[i] - hanging;
            lines[li].follows = open.last().copied();
            if open.is_empty() && !an.item_start[i] {
                lines[li].hang = an.waveform_start[i].filter(|&f| line_of[f] < li);
            }
            indented[li] = true;
        }
        if let Some(end) = an.shift_end[i]
            && lines[li].start == i
        {
            shifts.push(end);
        }
        if elem.is_token(L::LPAREN) {
            open.push(li);
            open_list.push(an.opens[i]);
        } else if closes {
            open.pop();
            open_list.pop();
        }
    }
    // A comment line takes the indentation of the code after it. One that is
    // indented deeper than a following `end`, `else`, `when` or `)` stays
    // with the code above instead.
    for li in 0..lines.len() {
        if !lines[li].comment_only {
            continue;
        }
        let next = (lines[li].end..n).find(|&k| !elems[k].is_comment());
        lines[li].follows = next.map(|k| line_of[k]);
        let first = &elems[lines[li].start];
        if L::keeps_column(first.kind) {
            lines[li].indent = first.src_col;
            continue;
        }
        lines[li].indent = match next {
            Some(k) => {
                let closes_block =
                    an.section[k] || an.alternative_start[k] || elems[k].is_token(L::RPAREN);
                if closes_block && elems[lines[li].start].src_col > elems[k].src_col {
                    would[k] + indent
                } else {
                    would[k]
                }
            }
            None => 0,
        };
    }
    (lines, line_of)
}

/// A comment line before an `end`, `else`, `when` or `)` kept as written
/// is placed on the same side of that code's column as in the source, so
/// that the next time it stays where it is: the code keeps its column
/// while the comment moves.
pub(crate) fn indent_before_verbatim<L: Lang>(
    elems: &[Elem<L>],
    an: &Analysis<L>,
    lines: &mut [Line],
    line_of: &[usize],
    config: &FormatConfig,
) {
    for li in 0..lines.len() {
        if !lines[li].comment_only || lines[li].verbatim {
            continue;
        }
        let first = &elems[lines[li].start];
        if L::keeps_column(first.kind) {
            continue;
        }
        let Some(k) = (lines[li].end..elems.len()).find(|&k| !elems[k].is_comment()) else {
            continue;
        };
        let closes_block = an.section[k] || an.alternative_start[k] || elems[k].is_token(L::RPAREN);
        if !closes_block || !lines[line_of[k]].verbatim {
            continue;
        }
        let col = elems[k].src_col;
        let indent = &mut lines[li].indent;
        *indent = if first.src_col > col {
            (*indent).max(col + config.indent)
        } else {
            (*indent).min(col)
        };
    }
}

/// For an element starting a line: how many statement bodies it is in
/// that start on another line than their statement (`if (a)` then
/// `x = 1;`), and whether it starts one.
fn body_nesting<L: Lang>(elem: &Elem<L>, an: &Analysis<L>, line_of: &[usize]) -> (u32, bool) {
    let Some(syntax) = &elem.syntax else {
        return (0, false);
    };
    let start = syntax.text_range().start();
    let mut child = match syntax {
        NodeOrToken::Node(n) => Some(n.clone()),
        NodeOrToken::Token(t) => t.parent(),
    };
    let (mut nested, mut starts_body) = (0, false);
    while let Some(c) = child {
        let Some(parent) = c.parent() else {
            break;
        };
        if L::nests_body(parent.kind(), c.kind()) {
            let body = c.text_range().start();
            starts_body |= body == start;
            let line = |at| an.index.get(&at).map(|&k| line_of[k]);
            if let (Some(a), Some(b)) = (line(parent.text_range().start()), line(body))
                && a != b
            {
                nested += 1;
            }
        }
        child = Some(parent);
    }
    (nested, starts_body)
}

/// The column where element `e` of `line` starts. Without `pads`, before
/// alignment.
pub(crate) fn column<L: Lang>(
    line: &Line,
    e: usize,
    elems: &[Elem<L>],
    an: &Analysis<L>,
    pads: &[u32],
) -> usize {
    let mut col = line.indent as usize;
    for (j, elem) in elems.iter().enumerate().take(e + 1).skip(line.start) {
        if j > line.start {
            col += usize::from(an.space[j]) + pads.get(j).map_or(0, |&p| p as usize);
        }
        if j < e {
            col += elem.width();
        }
    }
    col
}

/// The width of a line, not counting a trailing comment.
pub(crate) fn code_width<L: Lang>(line: &Line, elems: &[Elem<L>], an: &Analysis<L>) -> usize {
    let mut end = line.end;
    while end > line.start + 1 && elems[end - 1].is_comment() {
        end -= 1;
    }
    let mut width = line.indent as usize;
    for (j, elem) in (line.start..end).zip(&elems[line.start..end]) {
        if j > line.start {
            width += usize::from(an.space[j]);
        }
        width += elem.width();
    }
    width
}

/// For each line that is too wide, picks a list on it to lay out one
/// element per line, or a chain of operators to break the line at.
/// Returns whether anything changed.
///
/// The outermost wins: what is in the fewest parentheses. At the same
/// depth, a port or generic list or map comes first, then the operators
/// that bind loosest (`else`, `and`, `=`, `+`, ...), then calls and
/// aggregates. An expression that is the single index or argument in its
/// parentheses comes last.
pub(crate) fn expand_long_lines<L: Lang>(
    elems: &[Elem<L>],
    an: &Analysis<L>,
    lines: &[Line],
    expanded: &mut Expanded,
    config: &FormatConfig,
) -> bool {
    #[derive(Clone, Copy)]
    enum Pick {
        List(usize),
        Chain(usize),
    }
    let max = config.max_width as usize;
    let mut changed = false;
    for line in lines {
        if line.comment_only || code_width(line, elems, an) <= max {
            continue;
        }
        // Breaking up a single index or argument (`v(7 downto 0)`) reads
        // worse than a long line, unless it is a port or generic list.
        let lists = (line.start..line.end)
            .filter_map(|i| an.opens[i])
            .filter(|&id| !expanded.lists[id])
            .filter(|&id| {
                let list = &an.lists[id];
                !list.seps.is_empty() || list.layout != ListLayout::Preserve
            })
            .map(|id| {
                let list = &an.lists[id];
                let rank = if list.layout == ListLayout::Preserve {
                    u32::MAX
                } else {
                    0
                };
                ((false, list.depth, rank), Pick::List(id))
            });
        // An expression breaks only when it runs past `max_width` itself.
        // As the single index or argument in parentheses (`v(i + 1)`), only
        // when nothing else on the line can break.
        let past_width = |id: usize| {
            let end = an.chains[id].end;
            end >= line.end || column(line, end, elems, an, &[]) + elems[end].width() > max
        };
        let chains = (line.start..line.end)
            .filter(|_| config.break_expressions)
            .filter_map(|i| an.chain_of[i])
            .filter(|&id| !expanded.chains[id])
            .filter(|&id| past_width(id))
            .map(|id| {
                let chain = &an.chains[id];
                let alone = chain.list.is_some_and(|l| an.lists[l].seps.is_empty());
                (
                    (alone, chain.depth, 1 + u32::from(chain.class)),
                    Pick::Chain(id),
                )
            });
        // The first of the best, in the order of the line.
        let pick =
            lists.chain(chains).fold(
                None,
                |best: Option<((bool, i32, u32), Pick)>, c| match best {
                    Some(b) if b.0 <= c.0 => Some(b),
                    _ => Some(c),
                },
            );
        match pick {
            Some((_, Pick::List(id))) => expanded.lists[id] = true,
            Some((_, Pick::Chain(id))) => expanded.chains[id] = true,
            None => continue,
        }
        changed = true;
    }
    changed
}

/// Writes the lines out. Also returns where each element landed.
pub(crate) fn render<L: Lang>(
    elems: &[Elem<L>],
    an: &Analysis<L>,
    lines: &[Line],
    pads: &[u32],
    config: &FormatConfig,
) -> (String, Vec<(usize, usize)>) {
    let mut out = String::new();
    let mut positions = vec![(0, 0); elems.len()];
    let tab = config.indent.max(1);
    for (li, line) in lines.iter().enumerate() {
        if li > 0 {
            out.push('\n');
            for _ in 0..line.blank_before {
                out.push('\n');
            }
        }
        if config.use_tabs {
            out.extend(std::iter::repeat_n('\t', (line.indent / tab) as usize));
            out.extend(std::iter::repeat_n(' ', (line.indent % tab) as usize));
        } else {
            out.extend(std::iter::repeat_n(' ', line.indent as usize));
        }
        for i in line.start..line.end {
            if i > line.start {
                let spaces = usize::from(an.space[i]) + pads[i] as usize;
                out.extend(std::iter::repeat_n(' ', spaces));
            }
            let start = out.len();
            out.push_str(&elems[i].text);
            positions[i] = (start, out.len());
        }
    }
    if !out.is_empty() {
        out.push('\n');
    }
    (out, positions)
}

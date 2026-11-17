//! Checks of statements and their neighbors: a step that breaks a run of
//! similar assignments or associations, branches that repeat a condition
//! or a body, associations that look swapped, testbench processes that
//! restart, and waits with no timeout.

use rustc_hash::FxHashMap;
use vfl_syntax::green_walk::ChildTokens;
use vfl_syntax::{SyntaxKind, SyntaxNode, TextRange};

use super::{Hazards, call_args, canonical, fingerprint};
use crate::decl::DeclKind;
use crate::diagnostic::codes;
use crate::rtl::reset::{if_branches, process_body};

/// An element of a run: `q(2) <= d(1)` is `(q, 2, d, 1)`.
struct Step {
    range: TextRange,
    target: String,
    at: i64,
    source: String,
    from: i64,
    text: String,
}

/// The name and static index of `x(3)`.
fn indexed(node: &SyntaxNode) -> Option<(String, i64)> {
    let node = super::super::unparen(node);
    if node.kind() != SyntaxKind::CALL_EXPR {
        return None;
    }
    let prefix = node.children().next()?;
    if prefix.kind() != SyntaxKind::NAME {
        return None;
    }
    let [index] = call_args(&node).try_into().ok()?;
    let index = super::super::unparen(&index);
    if index.kind() != SyntaxKind::LITERAL {
        return None;
    }
    let token = index.first_token()?;
    if token.kind() != SyntaxKind::DECIMAL_LITERAL {
        return None;
    }
    Some((canonical(&prefix), token.text().parse().ok()?))
}

/// A formal with a number: `d2` is `(d, 2)`, `d(2)` too.
fn numbered(node: &SyntaxNode) -> Option<(String, i64)> {
    if let Some(found) = indexed(node) {
        return Some(found);
    }
    if node.kind() != SyntaxKind::NAME {
        return None;
    }
    let text = canonical(node);
    let digits = text.len() - text.trim_end_matches(|c: char| c.is_ascii_digit()).len();
    if digits == 0 || digits == text.len() {
        return None;
    }
    let (name, number) = text.split_at(text.len() - digits);
    Some((name.to_string(), number.parse().ok()?))
}

/// The single value of an assignment: `x` in `q <= x;`.
fn single_value(statement: &SyntaxNode) -> Option<SyntaxNode> {
    match statement.kind() {
        SyntaxKind::VARIABLE_ASSIGN_STMT => statement.children().nth(1),
        SyntaxKind::SIGNAL_ASSIGN_STMT => {
            let waveform = statement
                .children()
                .find(|n| n.kind() == SyntaxKind::WAVEFORM)?;
            let mut elements = waveform.children();
            let element = elements.next()?;
            if elements.next().is_some() || element.has_child(|k| k == SyntaxKind::AFTER_KW) {
                return None;
            }
            element.children().next()
        }
        _ => None,
    }
}

/// Words of an identifier that say nothing about which signal it is.
const NOISE: &[&str] = &[
    "s", "sig", "i", "o", "in", "out", "r", "reg", "d", "q", "w", "wire", "int", "n", "p",
];

fn words(name: &str) -> Vec<String> {
    name.to_ascii_lowercase()
        .split('_')
        .filter(|w| !w.is_empty() && !NOISE.contains(w))
        .map(str::to_string)
        .collect()
}

/// Whether an actual's name says it is for a formal: the formal's words
/// are all in it.
fn fits(actual: &str, formal: &str) -> bool {
    let formal = words(formal);
    let actual = words(actual);
    !formal.is_empty() && formal.iter().all(|w| actual.contains(w))
}

impl Hazards<'_, '_, '_> {
    // ---- sema/broken-sequence --------------------------------------------------

    /// Runs of assignments `t(i) <= s(j)` among the statements of `node`.
    pub(super) fn statement_sequence(&mut self, node: &SyntaxNode) {
        let mut run: Vec<Step> = Vec::new();
        for child in node.children() {
            let step = matches!(
                child.kind(),
                SyntaxKind::SIGNAL_ASSIGN_STMT | SyntaxKind::VARIABLE_ASSIGN_STMT
            )
            .then(|| {
                let (target, at) = indexed(&child.children().next()?)?;
                let (source, from) = indexed(&single_value(&child)?)?;
                Some(Step {
                    range: child.text_range(),
                    target,
                    at,
                    source,
                    from,
                    text: self.text(&child).trim_end_matches(';').trim().to_string(),
                })
            })
            .flatten();
            match step {
                Some(step)
                    if run
                        .last()
                        .is_none_or(|l| l.target == step.target && l.source == step.source) =>
                {
                    run.push(step)
                }
                Some(step) => {
                    self.sequence(std::mem::take(&mut run));
                    run.push(step);
                }
                None => self.sequence(std::mem::take(&mut run)),
            }
        }
        self.sequence(run);
    }

    /// Runs of associations `f2 => x(2)` in an association list.
    pub(super) fn association_sequence(&mut self, list: &SyntaxNode) {
        let mut groups: FxHashMap<(String, String), Vec<Step>> = FxHashMap::default();
        let mut order = Vec::new();
        for element in list
            .children()
            .filter(|n| n.kind() == SyntaxKind::ASSOCIATION_ELEMENT)
        {
            if !element.has_child(|k| k == SyntaxKind::ARROW) {
                continue;
            }
            let mut parts = element.children();
            let (Some(formal), Some(actual)) = (parts.next(), parts.next()) else {
                continue;
            };
            let (Some((target, at)), Some((source, from))) = (numbered(&formal), indexed(&actual))
            else {
                continue;
            };
            let key = (target.clone(), source.clone());
            if !groups.contains_key(&key) {
                order.push(key.clone());
            }
            groups.entry(key).or_default().push(Step {
                range: element.text_range(),
                target,
                at,
                source,
                from,
                text: self.text(&element).trim().to_string(),
            });
        }
        for key in order {
            if let Some(run) = groups.remove(&key) {
                self.sequence(run);
            }
        }
    }

    /// Reports the one step of a run whose offset between target and
    /// source index differs from all the others, when at least three agree.
    fn sequence(&mut self, run: Vec<Step>) {
        if run.len() < 4 {
            return;
        }
        let mut counts: FxHashMap<i64, usize> = FxHashMap::default();
        for s in &run {
            *counts.entry(s.at - s.from).or_default() += 1;
        }
        let Some((&offset, &count)) = counts.iter().max_by_key(|(_, c)| **c) else {
            return;
        };
        if count != run.len() - 1 || count < 3 {
            return;
        }
        let Some(odd) = run.iter().find(|s| s.at - s.from != offset) else {
            return;
        };
        let expected = odd.at - offset;
        self.report(
            codes::BROKEN_SEQUENCE,
            odd.range,
            format!(
                "`{}` breaks the pattern of its {count} neighbors (`{}` index i goes with `{}` \
                 index i{}): `{}({expected})` expected",
                odd.text,
                odd.target,
                odd.source,
                match offset {
                    0 => String::new(),
                    o if o > 0 => format!(" - {o}"),
                    o => format!(" + {}", -o),
                },
                odd.source
            ),
        );
    }

    // ---- sema/branch-clone -------------------------------------------------------

    pub(super) fn if_clones(&mut self, node: &SyntaxNode) {
        let branches = if_branches(node);
        // A condition that repeats an earlier one: its branch never runs.
        let mut seen: Vec<(u64, TextRange)> = Vec::new();
        for (condition, _) in &branches {
            let Some(condition) = condition else {
                continue;
            };
            let key = fingerprint([condition]);
            if let Some((_, first)) = seen.iter().find(|(k, _)| *k == key) {
                let line = self.line(*first);
                self.report(
                    codes::BRANCH_CLONE,
                    condition.text_range(),
                    format!(
                        "this condition is the same as the one on line {line}, so its branch \
                         never runs"
                    ),
                );
            } else {
                seen.push((key, condition.text_range()));
            }
        }
        // Every branch, `else` included, doing the same.
        if branches.len() >= 2 && branches.last().is_some_and(|(c, _)| c.is_none()) {
            let bodies: Vec<u64> = branches.iter().map(|(_, s)| fingerprint(s)).collect();
            if !branches[0].1.is_empty() && bodies.iter().all(|b| *b == bodies[0]) {
                let start = node.text_range().start();
                self.report(
                    codes::BRANCH_CLONE,
                    TextRange::new(start, start + vfl_syntax::TextSize::from(2)),
                    "every branch of this `if` does the same thing: the conditions change nothing"
                        .into(),
                );
            }
        }
    }

    /// Neighboring alternatives of a `case` with the same statements.
    pub(super) fn case_clones(&mut self, node: &SyntaxNode) {
        let alternatives: Vec<SyntaxNode> = node
            .children()
            .filter(|n| n.kind() == SyntaxKind::CASE_ALTERNATIVE)
            .collect();
        // A decoder table (one alternative a line) repeats on purpose.
        let one_liners = alternatives
            .iter()
            .filter(|a| !self.text(a).contains('\n'))
            .count();
        if one_liners > 3 {
            return;
        }
        // The body of each alternative (`None` for nothing or `null`), and
        // whether it is `others`.
        let alternatives: Vec<(SyntaxNode, Option<u64>, bool)> = alternatives
            .into_iter()
            .map(|a| {
                let others = a
                    .children()
                    .find(|n| n.kind() == SyntaxKind::CHOICES)
                    .is_some_and(|c| c.has_child(|k| k == SyntaxKind::OTHERS_KW));
                let body: Vec<SyntaxNode> = a
                    .children()
                    .filter(|n| super::super::is_statement(n.kind()))
                    .collect();
                let empty = match body.as_slice() {
                    [] => true,
                    [one] => one.kind() == SyntaxKind::NULL_STMT,
                    _ => false,
                };
                let body = (!empty).then(|| fingerprint(&body));
                (a, body, others)
            })
            .collect();
        for pair in alternatives.windows(2) {
            let [(_, first, first_others), (second, body, others)] = pair else {
                continue;
            };
            if *first_others || *others || body.is_none() || first != body {
                continue;
            }
            let choices = second
                .children()
                .find(|n| n.kind() == SyntaxKind::CHOICES)
                .map_or(second.text_range(), |c| c.text_range());
            self.report(
                codes::BRANCH_CLONE,
                choices,
                "these choices do the same as the alternative above: merge them \
                 (`when a | b =>`), or check that one of the two is not a copy left unchanged"
                    .into(),
            );
        }
    }

    // ---- sema/process-restarts -------------------------------------------------------

    pub(super) fn process_restarts(&mut self, node: &SyntaxNode) {
        use SyntaxKind::*;
        if node.has_child(|k| k == SENSITIVITY_LIST)
            || node.children().any(|n| n.kind() == SENSITIVITY_LIST)
        {
            return;
        }
        let body = process_body(node);
        let Some(last) = body.last() else {
            return;
        };
        let timed = vfl_syntax::green_walk::descendants_where(node, |k| k == WAIT_STMT)
            .iter()
            .any(|w| w.has_child(|k| k == FOR_KW));
        if !timed {
            return;
        }
        let stops = match last.kind() {
            // `wait;`
            WAIT_STMT => !last.has_child(|k| matches!(k, UNTIL_KW | ON_KW | FOR_KW)),
            PROCEDURE_CALL_STMT => last
                .children()
                .next()
                .and_then(|n| crate::resolver::mark_token(&n))
                .is_some_and(|t| {
                    matches!(
                        t.text().to_ascii_lowercase().as_str(),
                        "stop" | "finish" | "end_simulation" | "std_stop"
                    )
                }),
            // `loop ... end loop;` with no scheme never ends.
            LOOP_STMT => !last.has_child(|k| matches!(k, WHILE_KW | FOR_KW)),
            ASSERT_STMT | REPORT_STMT => canonical(last).contains("failure"),
            _ => false,
        };
        if stops {
            return;
        }
        // A clock or toggle generator restarts on purpose: it only
        // assigns one signal and waits.
        let mut targets = Vec::new();
        let mut only_assignments = true;
        for statement in &body {
            match statement.kind() {
                SIGNAL_ASSIGN_STMT => {
                    if let Some(t) = statement.children().next() {
                        let t = canonical(&t);
                        if !targets.contains(&t) {
                            targets.push(t);
                        }
                    }
                }
                WAIT_STMT
                    if statement.has_child(|k| k == FOR_KW)
                        && !statement.has_child(|k| matches!(k, UNTIL_KW | ON_KW)) => {}
                _ => only_assignments = false,
            }
        }
        if only_assignments && targets.len() <= 1 && last.kind() == WAIT_STMT {
            return;
        }
        // Waits only on signals, and a process that loops on purpose
        // (`while true loop`), are left alone too: only processes driven
        // by time are checked.
        let start = node.text_range().start();
        let label = node
            .children()
            .find(|n| n.kind() == LABEL)
            .map(|l| l.text_range());
        let range =
            label.unwrap_or_else(|| TextRange::new(start, start + vfl_syntax::TextSize::from(7)));
        self.report(
            codes::PROCESS_RESTARTS,
            range,
            "this process has no sensitivity list and does not end with `wait;`, \
             `std.env.stop` or an endless loop: after its last statement it starts over and \
             plays its stimuli again"
                .into(),
        );
    }

    // ---- sema/unbounded-wait ------------------------------------------------------

    pub(super) fn unbounded_wait(&mut self, node: &SyntaxNode) {
        use SyntaxKind::*;
        if !node.has_child(|k| matches!(k, UNTIL_KW | ON_KW)) || node.has_child(|k| k == FOR_KW) {
            return;
        }
        if self.c.rtl != Some(false) {
            return;
        }
        // A clocked wait, `wait until rising_edge(clk)`, always ends.
        if node
            .children()
            .find(|n| n.kind() != LABEL)
            .is_some_and(|c| super::is_edge(&c))
        {
            return;
        }
        // In a procedure, the caller decides.
        if node.ancestors().any(|a| a.kind() == SUBPROGRAM_BODY) {
            return;
        }
        self.report(
            codes::UNBOUNDED_WAIT,
            node.text_range(),
            "this `wait` has no timeout: if the design never answers, the simulation hangs \
             without a message; add `for <time>` and check the condition after it"
                .into(),
        );
    }

    // ---- sema/swapped-association ---------------------------------------------------

    pub(super) fn swapped(&mut self, list: &SyntaxNode) {
        let mut pairs: Vec<(String, String, TextRange)> = Vec::new();
        for element in list
            .children()
            .filter(|n| n.kind() == SyntaxKind::ASSOCIATION_ELEMENT)
        {
            if !element.has_child(|k| k == SyntaxKind::ARROW) {
                continue;
            }
            let mut parts = element.children();
            let (Some(formal), Some(actual)) = (parts.next(), parts.next()) else {
                continue;
            };
            if formal.kind() != SyntaxKind::NAME || actual.kind() != SyntaxKind::NAME {
                continue;
            }
            // Only objects: a constant or a literal is chosen on purpose.
            if !self.named(&actual).is_some_and(|(_, d)| {
                matches!(
                    d.kind,
                    DeclKind::Signal | DeclKind::Port { .. } | DeclKind::Variable
                )
            }) {
                continue;
            }
            pairs.push((canonical(&formal), canonical(&actual), element.text_range()));
        }
        for i in 0..pairs.len() {
            for j in i + 1..pairs.len() {
                let (fa, aa, _) = &pairs[i];
                let (fb, ab, range) = &pairs[j];
                if fits(aa, fb) && fits(ab, fa) && !fits(aa, fa) && !fits(ab, fb) {
                    self.report(
                        codes::SWAPPED_ASSOCIATION,
                        *range,
                        format!(
                            "`{fa} => {aa}` and `{fb} => {ab}` look swapped: each actual's \
                             name fits the other formal"
                        ),
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_that_fit() {
        assert!(fits("s_din_b", "din_b"));
        assert!(!fits("s_din_b", "din_a"));
        assert!(fits("rx_data", "data"));
        assert!(!fits("i", "clk"));
    }
}

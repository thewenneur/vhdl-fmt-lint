//! Why a counter with no test around its increment cannot pass its bound
//! (`sema/integer-overflow`): a test further away that stops it in time,
//! or a loop that runs too few times to get there.

use rustc_hash::FxHashSet;
use vfl_syntax::green_walk::descendants_where;
use vfl_syntax::{SyntaxKind, SyntaxNode};

use super::{Hazards, if_branches, is_edge, is_reset_test, op};
use crate::decl::{DeclId, DeclKind};

/// A counter being checked: its increment and its range.
pub(super) struct Counter<'n> {
    pub node: &'n SyntaxNode,
    pub process: &'n SyntaxNode,
    pub id: DeclId,
    pub lo: Option<i64>,
    pub hi: Option<i64>,
    pub up: bool,
    pub step: i64,
}

impl Counter<'_> {
    /// Whether a test of the counter against `value` comes before its
    /// bound: `cnt = 9` on `0 to 9` counting up does.
    fn in_time(&self, value: i64) -> bool {
        if self.up {
            self.hi.is_none_or(|hi| value <= hi)
        } else {
            self.lo.is_none_or(|lo| value >= lo)
        }
    }
}

impl Hazards<'_, '_, '_> {
    /// Whether something other than a test around the increment keeps the
    /// counter in its range.
    pub(super) fn cannot_overflow(&self, counter: &Counter<'_>) -> bool {
        self.bounded_by_loops(counter) || self.stopped_in_time(counter)
    }

    /// A test of the counter, anywhere it can act, whose branches leave the
    /// state the increment runs in, or reload the counter: `if cnt = 9
    /// then state <= done;` next to a `case state` whose `run` counts.
    fn stopped_in_time(&self, counter: &Counter<'_>) -> bool {
        use SyntaxKind::*;
        let variable = self
            .c
            .decl(counter.id)
            .is_some_and(|d| d.kind == DeclKind::Variable);
        // A variable lives in its process; a signal can be tested by any
        // process of the architecture.
        let scope = if variable {
            counter.process.clone()
        } else {
            counter
                .node
                .ancestors()
                .find(|a| a.kind() == ARCHITECTURE_BODY)
                .unwrap_or_else(|| counter.process.clone())
        };
        let stoppers = self.stoppers(counter, &scope);
        let statements = descendants_where(&scope, |k| {
            matches!(
                k,
                IF_STMT | CASE_STMT | SIGNAL_ASSIGN_STMT | VARIABLE_ASSIGN_STMT
            )
        });
        statements.iter().any(|s| match s.kind() {
            IF_STMT => {
                let branches = if_branches(s);
                branches.iter().any(|(c, _)| {
                    c.as_ref().is_some_and(|c| {
                        !is_edge(c) && !is_reset_test(c) && self.tests_in_time(c, counter)
                    })
                }) && branches
                    .iter()
                    .flat_map(|(_, body)| body)
                    .any(|b| self.assigns_any(b, &stoppers, counter))
            }
            CASE_STMT => self.case_stops(s, &stoppers, counter),
            // `next_state <= done when cnt = 9 else run;`
            _ => {
                s.children().any(|c| c.kind() == CONDITIONAL_WAVEFORMS)
                    && self.assigns_any(s, &stoppers, counter)
                    && s.children()
                        .filter(|c| c.kind() == CONDITIONAL_WAVEFORMS)
                        .flat_map(|w| w.children())
                        .filter(|p| p.kind() != WAVEFORM)
                        .any(|c| self.tests_in_time(&c, counter))
            }
        })
    }

    /// What, when assigned, stops the increment: the counter itself (a
    /// reload), the objects its enclosing conditions and `case` selectors
    /// read (a state, an enable), and the objects those are copied from
    /// (`state <= next_state`).
    fn stoppers(&self, counter: &Counter<'_>, scope: &SyntaxNode) -> FxHashSet<DeclId> {
        use SyntaxKind::*;
        let mut reads = vec![counter.id];
        for a in counter
            .node
            .ancestors()
            .skip(1)
            .take_while(|a| a != counter.process)
        {
            match a.kind() {
                IF_STMT => {
                    for (c, _) in if_branches(&a) {
                        if let Some(c) = c.filter(|c| !is_edge(c) && !is_reset_test(c)) {
                            self.reads(c.text_range(), &mut reads);
                        }
                    }
                }
                CASE_STMT => {
                    if let Some(selector) = a.children().find(|n| n.kind() != LABEL) {
                        self.reads(selector.text_range(), &mut reads);
                    }
                }
                _ => {}
            }
        }
        let mut stoppers: FxHashSet<DeclId> = reads.iter().copied().collect();
        for assign in descendants_where(scope, |k| matches!(k, SIGNAL_ASSIGN_STMT)) {
            let Some((target, _)) = crate::rtl::assignment_target(&assign) else {
                continue;
            };
            if !self
                .object(&target)
                .is_some_and(|(id, _)| id != counter.id && reads.contains(&id))
            {
                continue;
            }
            let value = assign
                .children()
                .find(|n| n.kind() == WAVEFORM)
                .and_then(|w| w.children().next())
                .and_then(|e| e.children().next());
            if let Some((source, _)) = value.and_then(|v| self.object(&v)) {
                stoppers.insert(source);
            }
        }
        stoppers
    }

    /// Whether `statement` assigns one of `stoppers`, other than by
    /// counting the counter on.
    fn assigns_any(
        &self,
        statement: &SyntaxNode,
        stoppers: &FxHashSet<DeclId>,
        counter: &Counter<'_>,
    ) -> bool {
        use SyntaxKind::*;
        descendants_where(statement, |k| {
            matches!(
                k,
                SIGNAL_ASSIGN_STMT | VARIABLE_ASSIGN_STMT | SELECTED_ASSIGN_STMT
            )
        })
        .iter()
        .any(|assign| {
            let Some(target) = assign.children().find(|n| n.kind() != LABEL) else {
                return false;
            };
            let Some((id, _)) = self.object(&target) else {
                return false;
            };
            if !stoppers.contains(&id) {
                return false;
            }
            // `cnt <= cnt + 1` is the counting, not a reload.
            if id == counter.id {
                let mut reads = Vec::new();
                for value in assign.children().skip_while(|c| *c != target).skip(1) {
                    self.reads(value.text_range(), &mut reads);
                }
                return !reads.contains(&counter.id);
            }
            true
        })
    }

    /// A `case` on the counter with a choice in time whose alternative
    /// assigns a stopper.
    fn case_stops(
        &self,
        case: &SyntaxNode,
        stoppers: &FxHashSet<DeclId>,
        counter: &Counter<'_>,
    ) -> bool {
        use SyntaxKind::*;
        let Some(selector) = case.children().find(|n| n.kind() != LABEL) else {
            return false;
        };
        if self.object(&selector).map(|(id, _)| id) != Some(counter.id) {
            return false;
        }
        case.children()
            .filter(|n| n.kind() == CASE_ALTERNATIVE)
            .any(|alternative| {
                let choices = alternative.children().find(|n| n.kind() == CHOICES);
                let in_time = choices.is_some_and(|c| {
                    c.children().any(|choice| match self.c.int(&choice) {
                        Some(v) => counter.in_time(v),
                        // A range, a name the values do not know: trust it.
                        None => choice.kind() != OTHERS_KW,
                    })
                });
                in_time
                    && alternative
                        .children()
                        .filter(|n| super::super::is_statement(n.kind()))
                        .any(|s| self.assigns_any(&s, stoppers, counter))
            })
    }

    /// Whether a condition compares the counter with a value it reaches
    /// before its bound. A value that is not static counts: it may be.
    fn tests_in_time(&self, condition: &SyntaxNode, counter: &Counter<'_>) -> bool {
        use SyntaxKind::*;
        let mut reads = Vec::new();
        self.reads(condition.text_range(), &mut reads);
        if !reads.contains(&counter.id) {
            return false;
        }
        let comparisons: Vec<SyntaxNode> = descendants_where(condition, |k| k == BINARY_EXPR)
            .into_iter()
            .filter(|b| matches!(op(b), Some(EQ | NEQ | LT | LTE | GT | GTE)))
            .collect();
        let mut tested = false;
        for comparison in &comparisons {
            let mut sides = comparison.children();
            let (Some(a), Some(b)) = (sides.next(), sides.next()) else {
                continue;
            };
            let other = if self.object(&a).map(|(id, _)| id) == Some(counter.id) {
                b
            } else if self.object(&b).map(|(id, _)| id) == Some(counter.id) {
                a
            } else {
                continue;
            };
            tested = true;
            match self.c.int(&other) {
                Some(v) if !counter.in_time(v) => {}
                _ => return true,
            }
        }
        // Read some other way (a function, an index): trust it.
        !tested
    }

    /// A variable counted inside `for` loops of static length, set to a
    /// static value before them in its process: `n := 0; for i in v'range
    /// loop if v(i) = '1' then n := n + 1; ...` cannot pass `0 to 8`.
    fn bounded_by_loops(&self, counter: &Counter<'_>) -> bool {
        use SyntaxKind::*;
        if !self
            .c
            .decl(counter.id)
            .is_some_and(|d| d.kind == DeclKind::Variable)
        {
            return false;
        }
        let mut runs: i64 = 1;
        let mut outermost = None;
        for a in counter
            .node
            .ancestors()
            .take_while(|a| a != counter.process)
            .filter(|a| a.kind() == LOOP_STMT)
        {
            let Some(length) = self.loop_length(&a) else {
                return false;
            };
            runs = runs.saturating_mul(length);
            outermost = Some(a);
        }
        let Some(outermost) = outermost else {
            return false;
        };
        // The last static value given to the counter before the loops, in
        // the same statement list.
        let Some(start) = outermost
            .siblings(rowan::Direction::Prev)
            .skip(1)
            .find_map(|s| {
                (s.kind() == VARIABLE_ASSIGN_STMT
                    && s.children()
                        .find(|n| n.kind() != LABEL)
                        .and_then(|t| self.object(&t))
                        .is_some_and(|(id, _)| id == counter.id))
                .then(|| s.children().nth(1).and_then(|v| self.c.int(&v)))
            })
            .flatten()
        else {
            return false;
        };
        let reach = runs.saturating_mul(counter.step);
        if counter.up {
            counter
                .hi
                .is_some_and(|hi| start.saturating_add(reach) <= hi)
        } else {
            counter
                .lo
                .is_some_and(|lo| start.saturating_sub(reach) >= lo)
        }
    }

    /// How many times a `for` loop runs, when its range is static.
    fn loop_length(&self, looped: &SyntaxNode) -> Option<i64> {
        use SyntaxKind::*;
        let scheme = looped.children().find(|n| n.kind() == FOR_SCHEME)?;
        let range = scheme
            .descendants()
            .find(|n| matches!(n.kind(), RANGE_EXPR | ATTRIBUTE_NAME))?;
        let bounds = if range.kind() == RANGE_EXPR {
            self.c.values.range(&range, self.c.unit, 0)?
        } else {
            // `v'range`, `v'reverse_range`.
            let prefix = range.children().next()?;
            let (id, _) = self.object(&prefix)?;
            self.c.values.object_bounds(id)?
        };
        Some((bounds.hi() - bounds.lo() + 1).max(0))
    }
}

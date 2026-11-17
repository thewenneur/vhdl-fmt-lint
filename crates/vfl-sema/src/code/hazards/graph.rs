//! What drives each signal of an architecture: its assignments, whether a
//! clock edge registers them, what they read. Built once per architecture
//! for the checks that follow signals from statement to statement.

use rustc_hash::{FxHashMap, FxHashSet};
use vfl_syntax::{SyntaxKind, SyntaxNode};

use super::{Clocked, Hazards, clocked, edge_signal};
use crate::decl::{Access, DeclId};
use crate::rtl::assignment_target;

/// One assignment of a signal or variable.
pub(super) struct Assign {
    pub target: DeclId,
    pub statement: SyntaxNode,
    /// The expressions assigned.
    pub values: Vec<SyntaxNode>,
    /// The conditions the assignment is under, and the selectors of the
    /// `case` statements it is in.
    pub conditions: Vec<SyntaxNode>,
    /// The clock signal, for an assignment on a clock edge.
    pub clock: Option<DeclId>,
    /// Whether it runs on a reset.
    pub reset: bool,
}

/// The assignments of an architecture.
pub(super) struct Graph {
    pub assigns: Vec<Assign>,
    pub by_target: FxHashMap<DeclId, Vec<usize>>,
    /// Signals an instance drives, whose logic is not known here.
    pub opaque: FxHashSet<DeclId>,
    /// The clocked processes.
    pub clocked: Vec<Clocked>,
}

impl Graph {
    /// The assignments of a signal outside resets.
    pub fn drivers(&self, id: DeclId) -> impl Iterator<Item = &Assign> {
        self.by_target
            .get(&id)
            .into_iter()
            .flatten()
            .map(|&i| &self.assigns[i])
            .filter(|a| !a.reset)
    }
}

impl Hazards<'_, '_, '_> {
    /// The graph of an architecture body.
    pub(super) fn graph(&self, body: &SyntaxNode) -> Graph {
        use SyntaxKind::*;
        let mut graph = Graph {
            assigns: Vec::new(),
            by_target: FxHashMap::default(),
            opaque: FxHashSet::default(),
            clocked: Vec::new(),
        };
        let mut statements = Vec::new();
        statements_under(body, &mut |n| {
            let found = matches!(
                n.kind(),
                PROCESS_STMT
                    | SIGNAL_ASSIGN_STMT
                    | SELECTED_ASSIGN_STMT
                    | COMPONENT_INSTANTIATION_STMT
            );
            if found {
                statements.push(n.clone());
            }
            // Processes and subprograms are walked on their own.
            !found
        });
        for statement in statements {
            match statement.kind() {
                PROCESS_STMT => self.process(&statement, &mut graph),
                COMPONENT_INSTANTIATION_STMT => {
                    for r in self.c.refs_in(statement.text_range()) {
                        if matches!(r.access, Access::Write | Access::ReadWrite)
                            && let Some(id) = super::super::single_decl(r)
                        {
                            graph.opaque.insert(id);
                        }
                    }
                }
                _ => {
                    let Some((target, _)) = assignment_target(&statement) else {
                        continue;
                    };
                    let mut values = Vec::new();
                    let mut conditions = Vec::new();
                    for child in statement.children() {
                        match child.kind() {
                            CONDITIONAL_WAVEFORMS | SELECTED_WAVEFORMS => {
                                for part in child.children() {
                                    match part.kind() {
                                        WAVEFORM => values.extend(waveform_values(&part)),
                                        CHOICES => {}
                                        _ => conditions.push(part),
                                    }
                                }
                            }
                            WAVEFORM => values.extend(waveform_values(&child)),
                            _ => {}
                        }
                    }
                    if statement.kind() == SELECTED_ASSIGN_STMT
                        && let Some(selector) = statement.children().find(|n| n.kind() != LABEL)
                    {
                        conditions.push(selector);
                    }
                    self.add(
                        &mut graph,
                        &target,
                        statement.clone(),
                        values,
                        conditions,
                        None,
                        false,
                    );
                }
            }
        }
        graph
    }

    fn process(&self, process: &SyntaxNode, graph: &mut Graph) {
        use SyntaxKind::*;
        // A testbench process, driven by time.
        let mut timed = false;
        statements_under(process, &mut |n| {
            timed |= n.kind() == WAIT_STMT
                && vfl_syntax::green_walk::ChildTokens::has_child(n, |k| k == FOR_KW);
            !timed
        });
        if timed {
            return;
        }
        match clocked(process) {
            Some(parts) => {
                let clock = edge_signal(&parts.edge)
                    .and_then(|n| self.named(&n))
                    .map(|(id, _)| id);
                for (statements, reset) in [(&parts.reset, true), (&parts.clocked, false)] {
                    for statement in statements {
                        self.walk(statement, clock, reset, &mut Vec::new(), graph);
                    }
                }
                graph.clocked.push(parts);
            }
            None => {
                for statement in crate::rtl::reset::process_body(process) {
                    self.walk(&statement, None, false, &mut Vec::new(), graph);
                }
            }
        }
    }

    /// One walk down the statements, with the conditions of the `if`,
    /// `case` and loops around each one on a stack.
    fn walk(
        &self,
        node: &SyntaxNode,
        clock: Option<DeclId>,
        reset: bool,
        around: &mut Vec<SyntaxNode>,
        graph: &mut Graph,
    ) {
        use SyntaxKind::*;
        let depth = around.len();
        match node.kind() {
            SIGNAL_ASSIGN_STMT | VARIABLE_ASSIGN_STMT => {
                self.assignment(node, clock, reset, around, graph);
                return;
            }
            IF_STMT => around.extend(
                crate::rtl::reset::if_branches(node)
                    .into_iter()
                    .filter_map(|(c, _)| c)
                    .filter(|c| !super::is_edge(c) && !super::is_reset_test(c)),
            ),
            CASE_STMT => {
                if let Some(selector) = node.children().find(|n| n.kind() != LABEL) {
                    around.push(selector);
                }
            }
            _ => {}
        }
        for child in node.children() {
            if super::super::is_statement(child.kind()) || child.kind() == CASE_ALTERNATIVE {
                self.walk(&child, clock, reset, around, graph);
            }
        }
        around.truncate(depth);
    }

    fn assignment(
        &self,
        assign: &SyntaxNode,
        clock: Option<DeclId>,
        reset: bool,
        around: &[SyntaxNode],
        graph: &mut Graph,
    ) {
        use SyntaxKind::*;
        let Some(target) = assign.children().find(|n| n.kind() != LABEL) else {
            return;
        };
        let mut values = Vec::new();
        let mut conditions = Vec::new();
        if assign.kind() == VARIABLE_ASSIGN_STMT {
            for child in assign.children().skip_while(|c| *c != target).skip(1) {
                if child.kind() == CONDITIONAL_WAVEFORMS {
                    for part in child.children() {
                        match part.kind() {
                            WAVEFORM => values.extend(waveform_values(&part)),
                            _ => conditions.push(part),
                        }
                    }
                } else {
                    values.push(child);
                }
            }
        } else {
            for child in assign.children() {
                match child.kind() {
                    WAVEFORM => values.extend(waveform_values(&child)),
                    CONDITIONAL_WAVEFORMS => {
                        for part in child.children() {
                            match part.kind() {
                                WAVEFORM => values.extend(waveform_values(&part)),
                                _ => conditions.push(part),
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        // Innermost first, as the walk up the ancestors gave them.
        conditions.extend(around.iter().rev().cloned());
        let registered = clock.filter(|_| assign.kind() == SIGNAL_ASSIGN_STMT);
        self.add(
            graph,
            &target,
            assign.clone(),
            values,
            conditions,
            registered,
            reset,
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn add(
        &self,
        graph: &mut Graph,
        target: &SyntaxNode,
        statement: SyntaxNode,
        values: Vec<SyntaxNode>,
        conditions: Vec<SyntaxNode>,
        clock: Option<DeclId>,
        reset: bool,
    ) {
        let Some((id, _)) = self.object(target) else {
            return;
        };
        graph
            .by_target
            .entry(id)
            .or_default()
            .push(graph.assigns.len());
        graph.assigns.push(Assign {
            target: id,
            statement,
            values,
            conditions,
            clock,
            reset,
        });
    }

    /// The signals and variables an assignment reads.
    pub(super) fn assign_reads(&self, assign: &Assign) -> Vec<DeclId> {
        let mut out = Vec::new();
        for node in assign.values.iter().chain(&assign.conditions) {
            self.reads(node.text_range(), &mut out);
        }
        // The index of the target: `mem(addr) <= d` reads `addr`.
        if let Some(target) = assign
            .statement
            .children()
            .find(|n| n.kind() == SyntaxKind::CALL_EXPR)
            .filter(|t| assign.statement.children().next().as_ref() == Some(t))
            && let Some(list) = target
                .children()
                .find(|n| n.kind() == SyntaxKind::ASSOCIATION_LIST)
        {
            self.reads(list.text_range(), &mut out);
        }
        out
    }
}

/// Calls `f` on each statement under `node`, outermost first, without
/// looking into expressions; `f` says whether to go on into the statement.
pub(super) fn statements_under(node: &SyntaxNode, f: &mut impl FnMut(&SyntaxNode) -> bool) {
    use SyntaxKind::*;
    for child in node.children() {
        let kind = child.kind();
        if super::super::is_statement(kind) {
            if f(&child) {
                statements_under(&child, f);
            }
        } else if matches!(kind, GENERATE_BODY | CASE_ALTERNATIVE | ARCHITECTURE_BODY) {
            statements_under(&child, f);
        }
    }
}

/// The expressions of a waveform.
fn waveform_values(waveform: &SyntaxNode) -> Vec<SyntaxNode> {
    waveform
        .children()
        .filter(|e| e.kind() == SyntaxKind::WAVEFORM_ELEMENT)
        .filter_map(|e| e.children().next())
        .collect()
}

//! State machines: a `case` on a signal or variable of an enumeration
//! type, whose alternatives assign literals of that type to it or to a
//! next-state object. Reports states no transition reaches, and states
//! nothing but a reset leaves.

use std::collections::{HashMap, HashSet};

use vfl_syntax::{SyntaxKind, SyntaxNode, TextRange};

use super::Checker;
use crate::decl::{DeclId, DeclKind, Target, TypeDef};
use crate::diagnostic::{Diagnostic, codes};
use crate::resolver::mark_token;

/// One alternative of a `case` on the state.
struct Alternative {
    range: TextRange,
    /// The literals it names, with the range of each choice.
    choices: Vec<(usize, TextRange)>,
}

impl Checker<'_> {
    /// The enumeration type of an object, with its literals, when it is
    /// declared in the project rather than a standard library.
    fn user_enum(&self, id: DeclId) -> Option<(DeclId, &[String])> {
        let mut mark = self.decl(id)?.subtype?;
        for _ in 0..32 {
            let decl = self.decl(mark)?;
            match decl.kind {
                DeclKind::Type => break,
                DeclKind::Subtype => mark = decl.subtype?,
                _ => return None,
            }
        }
        let decl = self.decl(mark)?;
        if self.ctx.files[decl.file as usize].builtin {
            return None;
        }
        match decl.type_def.as_deref()? {
            TypeDef::Enum(literals)
                if literals.len() > 1 && literals.iter().any(|l| !l.starts_with('\'')) =>
            {
                Some((mark, literals))
            }
            _ => None,
        }
    }

    /// The index of the literal of `ty` a name denotes.
    fn literal_of(&self, name: &SyntaxNode, ty: DeclId, literals: &[String]) -> Option<usize> {
        if name.kind() != SyntaxKind::NAME {
            return None;
        }
        let token = mark_token(name)?;
        let r = &self.result.refs[*self.refs_at.get(&token.text_range())?];
        r.targets.iter().find_map(|t| match t {
            Target::Decl(id) => self
                .decl(*id)
                .filter(|d| d.kind == DeclKind::EnumLiteral && d.subtype == Some(ty))
                .and_then(|d| literals.iter().position(|l| *l == d.name)),
            Target::Library(_) => None,
        })
    }

    /// A state object: a signal or variable of this unit.
    fn state_object(&self, name: &SyntaxNode) -> Option<DeclId> {
        if name.kind() != SyntaxKind::NAME {
            return None;
        }
        let id = self.target(&mark_token(name)?)?;
        (id.unit == self.unit
            && matches!(self.decl(id)?.kind, DeclKind::Signal | DeclKind::Variable))
        .then_some(id)
    }

    pub(super) fn state_machines(&mut self, root: &SyntaxNode) {
        let mut machines: HashMap<DeclId, Vec<SyntaxNode>> = HashMap::new();
        for case in std::mem::take(&mut self.cases) {
            let Some(selector) = case.children().find(|n| n.kind() != SyntaxKind::LABEL) else {
                continue;
            };
            if let Some(id) = self.state_object(&selector)
                && self.user_enum(id).is_some()
            {
                machines.entry(id).or_default().push(case);
            }
        }
        if machines.is_empty() {
            return;
        }
        let assignments: Vec<SyntaxNode> = root
            .descendants()
            .filter(|n| {
                matches!(
                    n.kind(),
                    SyntaxKind::SIGNAL_ASSIGN_STMT
                        | SyntaxKind::VARIABLE_ASSIGN_STMT
                        | SyntaxKind::SELECTED_ASSIGN_STMT
                )
            })
            .collect();
        let mut machines: Vec<(DeclId, Vec<SyntaxNode>)> = machines.into_iter().collect();
        machines.sort_by_key(|(id, _)| *id);
        for (state, cases) in machines {
            self.machine(root, state, &cases, &assignments);
        }
    }

    /// The target of an assignment.
    fn assignment_target(node: &SyntaxNode) -> Option<SyntaxNode> {
        let selected = node.kind() == SyntaxKind::SELECTED_ASSIGN_STMT;
        node.children_with_tokens()
            .skip_while(|e| selected && e.kind() != SyntaxKind::SELECT_KW)
            .filter_map(|e| e.into_node())
            .find(|n| n.kind() != SyntaxKind::LABEL)
    }

    fn machine(
        &mut self,
        root: &SyntaxNode,
        state: DeclId,
        cases: &[SyntaxNode],
        assignments: &[SyntaxNode],
    ) -> Option<()> {
        use SyntaxKind::*;
        let (ty, literals) = self.user_enum(state)?;
        let count = literals.len();

        // The alternatives, and which one each state takes in each case.
        let mut alternatives: Vec<Alternative> = Vec::new();
        // For each alternative, the states that take it.
        let mut takers: Vec<Vec<usize>> = Vec::new();
        for case in cases {
            let mut taken = vec![false; count];
            let mut others = None;
            for alternative in case.children().filter(|n| n.kind() == CASE_ALTERNATIVE) {
                let choices = alternative.children().find(|n| n.kind() == CHOICES)?;
                let mut named = Vec::new();
                for element in choices.children_with_tokens() {
                    match element {
                        rowan::NodeOrToken::Token(t) if t.kind() == OTHERS_KW => {
                            others = Some(alternatives.len());
                        }
                        rowan::NodeOrToken::Token(_) => {}
                        rowan::NodeOrToken::Node(n) => {
                            // Ranges and anything else are not followed.
                            let literal = self.literal_of(&n, ty, literals)?;
                            named.push((literal, n.text_range()));
                        }
                    }
                }
                let mut states = Vec::new();
                for &(literal, _) in &named {
                    if !taken[literal] {
                        taken[literal] = true;
                        states.push(literal);
                    }
                }
                takers.push(states);
                alternatives.push(Alternative {
                    range: alternative.text_range(),
                    choices: named,
                });
            }
            if let Some(others) = others {
                takers[others].extend((0..count).filter(|&s| !taken[s]));
            }
        }

        // The state object and the next-state objects assigned in the
        // alternatives.
        let mut group: HashSet<DeclId> = HashSet::from([state]);
        let in_alternative = |r: TextRange| alternatives.iter().any(|a| a.range.contains_range(r));
        for assignment in assignments {
            if in_alternative(assignment.text_range())
                && let Some(id) =
                    Self::assignment_target(assignment).and_then(|t| self.state_object(&t))
                && self.user_enum(id).is_some_and(|(t, _)| t == ty)
            {
                group.insert(id);
            }
        }

        // Every assignment to the group: a literal, or another member.
        let mut accounted: HashSet<TextRange> = HashSet::new();
        let mut assigned: Vec<(TextRange, usize)> = Vec::new();
        for assignment in assignments {
            let Some(target) = Self::assignment_target(assignment) else {
                continue;
            };
            if self
                .state_object(&target)
                .is_none_or(|id| !group.contains(&id))
            {
                continue;
            }
            accounted.insert(mark_token(&target)?.text_range());
            let mut values = Vec::new();
            for n in assignment.children().skip_while(|n| *n != target).skip(1) {
                if n.kind() != DELAY_MECHANISM {
                    Self::assigned_values(&n, &mut values);
                }
            }
            for value in values {
                if let Some(literal) = self.literal_of(&value, ty, literals) {
                    assigned.push((assignment.text_range(), literal));
                } else if self
                    .state_object(&value)
                    .is_none_or(|v| !group.contains(&v))
                {
                    return None;
                }
            }
        }
        // Written any other way (a procedure, an aggregate, a port map):
        // the transitions are not known.
        for r in &self.result.refs {
            if r.access.writes()
                && !accounted.contains(&r.range)
                && r.targets
                    .iter()
                    .any(|t| matches!(t, Target::Decl(id) if group.contains(id)))
            {
                return None;
            }
        }

        // Transitions.
        let mut edges: Vec<HashSet<usize>> = vec![HashSet::new(); count];
        let mut any_edge = false;
        let mut roots: Vec<usize> = Vec::new();
        let mut global: Vec<usize> = Vec::new();
        for (range, literal) in assigned {
            let innermost = alternatives
                .iter()
                .enumerate()
                .filter(|(_, a)| a.range.contains_range(range))
                .min_by_key(|(_, a)| a.range.len());
            match innermost {
                Some((index, _)) => {
                    any_edge = true;
                    for &from in &takers[index] {
                        edges[from].insert(literal);
                    }
                }
                None if self.reset_ranges.iter().any(|r| r.contains_range(range)) => {
                    roots.push(literal)
                }
                // Assigned outside the `case`: possibly from any state.
                None => global.push(literal),
            }
        }
        if !any_edge {
            return None;
        }

        // The initial value, or the first literal.
        let decl = self.decl(state)?;
        let initial = match root.covering_element(decl.full_range) {
            rowan::NodeOrToken::Node(n) => n
                .ancestors()
                .find(|n| matches!(n.kind(), SIGNAL_DECL | VARIABLE_DECL))
                .and_then(|d| {
                    d.children_with_tokens()
                        .skip_while(|e| e.kind() != COLON_EQ)
                        .find_map(|e| e.into_node())
                })
                .and_then(|v| self.literal_of(&v, ty, literals)),
            rowan::NodeOrToken::Token(_) => None,
        }
        .unwrap_or(0);

        let mut reached = vec![false; count];
        let mut stack: Vec<usize> = std::iter::once(initial)
            .chain(roots.iter().copied())
            .chain(global.iter().copied())
            .collect();
        while let Some(s) = stack.pop() {
            if std::mem::replace(&mut reached[s], true) {
                continue;
            }
            stack.extend(edges[s].iter().copied().filter(|&t| !reached[t]));
        }

        let type_name = self
            .decl(ty)
            .map(|d| d.spelling.clone())
            .unwrap_or_default();
        let state_name = decl.spelling.clone();
        let spelling = |literal: usize| -> String {
            self.decl(ty)
                .and_then(|d| d.members.as_ref())
                .and_then(|m| m.get(&literals[literal]).first().copied())
                .and_then(|id| self.decl(id))
                .map_or_else(|| literals[literal].clone(), |d| d.spelling.clone())
        };
        let mut reported: HashSet<usize> = HashSet::new();
        let mut diags = Vec::new();
        for alternative in &alternatives {
            for &(literal, range) in &alternative.choices {
                if !reported.insert(literal) {
                    continue;
                }
                if !reached[literal] {
                    diags.push(Diagnostic {
                        code: codes::FSM_UNREACHABLE_STATE,
                        file: self.file,
                        range,
                        message: format!(
                            "no transition of `{state_name}` reaches state `{}` of `{type_name}`",
                            spelling(literal)
                        ),
                        fix: None,
                    });
                } else if global.is_empty() && edges[literal].iter().all(|&t| t == literal) {
                    diags.push(Diagnostic {
                        code: codes::FSM_STUCK_STATE,
                        file: self.file,
                        range,
                        message: format!(
                            "`{state_name}` never leaves state `{}` once there, except by a reset",
                            spelling(literal)
                        ),
                        fix: None,
                    });
                }
            }
        }
        self.diags.extend(diags);
        Some(())
    }
}

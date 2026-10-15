//! Branches of an `if` or `case` that leave out a signal the other
//! branches assign: every signal a process drives must be assigned in
//! every branch, unless it is assigned before the statement.

use rustc_hash::{FxHashMap, FxHashSet};
use vfl_syntax::{SyntaxKind, SyntaxNode, SyntaxToken, TextRange};

use super::reset::{if_branches, process_body};
use super::{Checker, Paths};
use crate::decl::DeclId;
use crate::diagnostic::{Diagnostic, codes};

/// The signals a statement assigns whole, with their first target range.
type Assigned = FxHashMap<DeclId, TextRange>;

impl Checker<'_> {
    /// Checks the statements of a combinational process.
    pub(super) fn combinational_branches(&mut self, body: &[SyntaxNode]) {
        self.branches(body, codes::MISSING_BRANCH_ASSIGNMENT);
    }

    /// Checks the clocked branch of a clocked process without `wait`; the
    /// reset branches are left out.
    pub(super) fn clocked_branches(&mut self, process: &SyntaxNode, edge: &SyntaxNode) {
        let body = process_body(process);
        let [statement] = body.as_slice() else {
            return;
        };
        if statement.kind() != SyntaxKind::IF_STMT {
            return;
        }
        let Some((_, clocked)) = if_branches(statement).into_iter().find(|(c, _)| {
            c.as_ref()
                .is_some_and(|c| c.text_range().contains_range(edge.text_range()))
        }) else {
            return;
        };
        self.branches(&clocked, codes::MISSING_BRANCH_ASSIGNMENT_CLOCKED);
    }

    fn branches(&mut self, body: &[SyntaxNode], code: &'static str) {
        let mut paths = Paths::default();
        self.sequence(body, &mut paths);
        if paths.first.is_empty() {
            return;
        }
        // Signals assigned in part somewhere are left alone, as for latches.
        let mut need: FxHashSet<DeclId> = paths
            .first
            .keys()
            .filter(|id| !paths.partial.contains(id))
            .copied()
            .collect();
        let mut found = Vec::new();
        self.branch_sequence(body, &mut need, &paths.partial, &mut found);
        found.sort_by_key(|(range, _)| range.start());
        for (range, message) in found {
            self.diags.push(Diagnostic {
                code,
                file: self.file,
                range,
                message,
                fix: None,
            });
        }
    }

    /// Walks a sequence of statements; `need` holds the signals not yet
    /// assigned on every path. Returns with the signals still missing.
    fn branch_sequence(
        &self,
        statements: &[SyntaxNode],
        need: &mut FxHashSet<DeclId>,
        partial: &FxHashSet<DeclId>,
        found: &mut Vec<(TextRange, String)>,
    ) {
        use SyntaxKind::*;
        for statement in statements {
            if need.is_empty() {
                return;
            }
            match statement.kind() {
                IF_STMT | CASE_STMT => self.branch_statement(statement, need, partial, found),
                LOOP_STMT | SEQUENTIAL_BLOCK_STMT => {
                    // A loop is assumed to run its body.
                    let body: Vec<SyntaxNode> = statement
                        .children()
                        .filter(|n| !matches!(n.kind(), LABEL | FOR_SCHEME | WHILE_SCHEME))
                        .collect();
                    self.branch_sequence(&body, need, partial, found);
                }
                _ => {
                    let always = self.statement(statement, &mut Paths::default());
                    need.retain(|id| !always.contains(id));
                }
            }
        }
    }

    fn branch_statement(
        &self,
        statement: &SyntaxNode,
        need: &mut FxHashSet<DeclId>,
        partial: &FxHashSet<DeclId>,
        found: &mut Vec<(TextRange, String)>,
    ) {
        let mut paths = Paths::default();
        self.statement(statement, &mut paths);
        // The signals this statement assigns that no earlier statement
        // assigns on every path.
        let inner: FxHashSet<DeclId> = paths
            .first
            .keys()
            .filter(|id| need.contains(id) && !partial.contains(id))
            .copied()
            .collect();
        if inner.is_empty() {
            return;
        }
        let first = &paths.first;
        let is_if = statement.kind() == SyntaxKind::IF_STMT;
        let what = if is_if { "if" } else { "case" };
        let branches: Vec<(SyntaxToken, Vec<SyntaxNode>)> = if is_if {
            if_keywords(statement)
                .into_iter()
                .zip(if_branches(statement))
                .map(|(k, (_, b))| (k, b))
                .collect()
        } else {
            case_alternatives(statement)
        };
        let resets = is_if
            && branches.first().is_some_and(|(_, b)| {
                b.first().is_some_and(|s| {
                    self.reset_ranges
                        .iter()
                        .any(|r| r.contains_range(s.text_range()))
                })
            });
        for (keyword, body) in &branches {
            let mut missing = inner.clone();
            self.branch_sequence(body, &mut missing, partial, found);
            // A reset branch sets the registers that have a reset value;
            // the other branches are checked on their own.
            if resets {
                continue;
            }
            if !missing.is_empty() {
                let branch = match keyword.kind() {
                    SyntaxKind::IF_KW => "this `if` branch",
                    SyntaxKind::ELSIF_KW => "this `elsif` branch",
                    SyntaxKind::ELSE_KW => "this `else` branch",
                    _ => "this `when` alternative",
                };
                found.push((
                    keyword.text_range(),
                    format!(
                        "{branch} does not assign {}, which another branch of the `{what}` assigns",
                        names(self, &missing, first)
                    ),
                ));
            }
        }
        if is_if
            && !resets
            && !branches
                .iter()
                .any(|(k, _)| k.kind() == SyntaxKind::ELSE_KW)
            && let Some((keyword, _)) = branches.first()
        {
            found.push((
                keyword.text_range(),
                format!(
                    "this `if` has no `else`, so {} {} not assigned when no condition holds",
                    names(self, &inner, first),
                    if inner.len() == 1 { "is" } else { "are" }
                ),
            ));
        }
        // Reported once, here.
        need.retain(|id| !inner.contains(id));
    }
}

/// The `if`, `elsif` and `else` keywords of an `if` statement, one per
/// branch of [`if_branches`].
fn if_keywords(node: &SyntaxNode) -> Vec<SyntaxToken> {
    let mut keywords = Vec::new();
    for t in node.children_with_tokens().filter_map(|e| e.into_token()) {
        match t.kind() {
            SyntaxKind::IF_KW | SyntaxKind::ELSIF_KW | SyntaxKind::ELSE_KW => keywords.push(t),
            SyntaxKind::END_KW => break,
            _ => {}
        }
    }
    keywords
}

/// The `when` keyword and statements of each alternative of a `case`.
fn case_alternatives(node: &SyntaxNode) -> Vec<(SyntaxToken, Vec<SyntaxNode>)> {
    node.children()
        .filter(|n| n.kind() == SyntaxKind::CASE_ALTERNATIVE)
        .filter_map(|alternative| {
            let when = alternative
                .children_with_tokens()
                .filter_map(|e| e.into_token())
                .find(|t| t.kind() == SyntaxKind::WHEN_KW)?;
            let body = alternative
                .children_with_tokens()
                .skip_while(|e| e.kind() != SyntaxKind::ARROW)
                .filter_map(|e| e.into_node())
                .collect();
            Some((when, body))
        })
        .collect()
}

/// The names of `ids` in the order they are first assigned: "`a`",
/// "`a` and `b`", "`a`, `b` and `c`".
fn names(checker: &Checker, ids: &FxHashSet<DeclId>, first: &Assigned) -> String {
    let mut ids: Vec<(TextRange, DeclId)> = ids
        .iter()
        .map(|id| (first.get(id).copied().unwrap_or_default(), *id))
        .collect();
    ids.sort_by_key(|(r, _)| r.start());
    let names: Vec<String> = ids
        .iter()
        .map(|(_, id)| {
            let name = checker
                .decl(*id)
                .map(|d| d.spelling.as_str())
                .unwrap_or_default();
            format!("`{name}`")
        })
        .collect();
    match names.split_last() {
        Some((last, rest)) if !rest.is_empty() => format!("{} and {last}", rest.join(", ")),
        _ => names.concat(),
    }
}

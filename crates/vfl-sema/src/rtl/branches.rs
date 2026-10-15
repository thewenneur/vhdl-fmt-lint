//! Branches of an `if` or `case` that leave out a signal the other
//! branches assign: every signal a process drives must be assigned in
//! every branch, unless it is assigned before the statement.
//!
//! Fixes: in a clocked process, the missing branches assign the register
//! its own value (`q <= q;`), which keeps the design's meaning; in a
//! combinational process, the signals get a default assignment before the
//! statement, which removes the latch and so changes the design.

use rustc_hash::{FxHashMap, FxHashSet};
use vfl_syntax::{SyntaxKind, SyntaxNode, TextRange, TextSize};

use super::reset::{if_branches, process_body};
use super::{Checker, Paths};
use crate::decl::{DeclId, DeclKind, Mode, TypeDef};
use crate::diagnostic::{Diagnostic, Edit, Fix, codes};

/// The signals a statement assigns whole, with their first target range.
type Assigned = FxHashMap<DeclId, TextRange>;

/// A finding: where, what, and its fix.
type Found = (TextRange, String, Option<Fix>);

/// One branch of an `if` or `case`.
struct Branch {
    /// The `if`, `elsif`, `else` or `when` keyword.
    keyword: SyntaxKind,
    keyword_range: TextRange,
    /// The end of the branch's header: its `then`, `else` or `=>`.
    header_end: TextSize,
    statements: Vec<SyntaxNode>,
}

impl Checker<'_> {
    /// Checks the statements of a combinational process.
    pub(super) fn combinational_branches(&mut self, body: &[SyntaxNode]) {
        self.branches(body, false);
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
        self.branches(&clocked, true);
    }

    fn branches(&mut self, body: &[SyntaxNode], clocked: bool) {
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
        let walk = Walk {
            partial: &paths.partial,
            clocked,
        };
        self.branch_sequence(body, &mut need, &walk, &mut found);
        found.sort_by_key(|(range, ..)| range.start());
        let code = if clocked {
            codes::MISSING_BRANCH_ASSIGNMENT_CLOCKED
        } else {
            codes::MISSING_BRANCH_ASSIGNMENT
        };
        for (range, message, fix) in found {
            self.diags.push(Diagnostic {
                code,
                file: self.file,
                range,
                message,
                fix,
            });
        }
    }

    /// Walks a sequence of statements; `need` holds the signals not yet
    /// assigned on every path, and is left with the ones still missing.
    fn branch_sequence(
        &self,
        statements: &[SyntaxNode],
        need: &mut FxHashSet<DeclId>,
        walk: &Walk,
        found: &mut Vec<Found>,
    ) {
        use SyntaxKind::*;
        for statement in statements {
            if need.is_empty() {
                return;
            }
            match statement.kind() {
                IF_STMT | CASE_STMT => self.branch_statement(statement, need, walk, found),
                LOOP_STMT | SEQUENTIAL_BLOCK_STMT => {
                    // A loop is assumed to run its body.
                    let body: Vec<SyntaxNode> = statement
                        .children()
                        .filter(|n| !matches!(n.kind(), LABEL | FOR_SCHEME | WHILE_SCHEME))
                        .collect();
                    self.branch_sequence(&body, need, walk, found);
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
        walk: &Walk,
        found: &mut Vec<Found>,
    ) {
        let mut paths = Paths::default();
        self.statement(statement, &mut paths);
        // The signals this statement assigns that no earlier statement
        // assigns on every path.
        let inner: FxHashSet<DeclId> = paths
            .first
            .keys()
            .filter(|id| need.contains(id) && !walk.partial.contains(id))
            .copied()
            .collect();
        if inner.is_empty() {
            return;
        }
        let first = &paths.first;
        let is_if = statement.kind() == SyntaxKind::IF_STMT;
        let what = if is_if { "if" } else { "case" };
        let branches = if is_if {
            if_branch_list(statement)
        } else {
            case_alternatives(statement)
        };
        let resets = is_if
            && branches.first().is_some_and(|b| {
                b.statements.first().is_some_and(|s| {
                    self.reset_ranges
                        .iter()
                        .any(|r| r.contains_range(s.text_range()))
                })
            });
        // What each branch misses, then what a missing `else` misses.
        let mut gaps: Vec<(&Branch, Vec<DeclId>)> = Vec::new();
        for branch in &branches {
            let mut missing = inner.clone();
            self.branch_sequence(&branch.statements, &mut missing, walk, found);
            // A reset branch sets the registers that have a reset value;
            // the other branches are checked on their own.
            if !resets && !missing.is_empty() {
                gaps.push((branch, ordered(&missing, first)));
            }
        }
        let no_else =
            is_if && !resets && !branches.iter().any(|b| b.keyword == SyntaxKind::ELSE_KW);
        let unassigned = no_else.then(|| ordered(&inner, first));
        // Reported once, here.
        need.retain(|id| !inner.contains(id));
        if gaps.is_empty() && unassigned.is_none() {
            return;
        }

        let text: &str = &self.ctx.files[self.file as usize].text;
        // The indentation of the statements of the branches.
        let indent = branches
            .iter()
            .find_map(|b| b.statements.first())
            .map(|s| line_indent(text, s.text_range().start()))
            .unwrap_or_default();
        let defaults = (!walk.clocked)
            .then(|| {
                let mut all: Vec<DeclId> = gaps.iter().flat_map(|(_, ids)| ids.clone()).collect();
                all.extend(unassigned.iter().flatten());
                let all = ordered(&all.into_iter().collect(), first);
                self.defaults_fix(text, statement, what, &all)
            })
            .flatten();
        for (branch, ids) in gaps {
            let label = match branch.keyword {
                SyntaxKind::IF_KW => "this `if` branch",
                SyntaxKind::ELSIF_KW => "this `elsif` branch",
                SyntaxKind::ELSE_KW => "this `else` branch",
                _ => "this `when` alternative",
            };
            let fix = if walk.clocked {
                self.hold_in_branch(text, branch, indent, &ids)
            } else {
                defaults.clone()
            };
            found.push((
                branch.keyword_range,
                format!(
                    "{label} does not assign {}, which another branch of the `{what}` assigns",
                    self.names(&ids)
                ),
                fix,
            ));
        }
        if let (Some(ids), Some(first_branch)) = (unassigned, branches.first()) {
            let fix = if walk.clocked {
                self.hold_in_else(text, statement, indent, &ids)
            } else {
                defaults
            };
            found.push((
                first_branch.keyword_range,
                format!(
                    "this `if` has no `else`, so {} {} not assigned when no condition holds",
                    self.names(&ids),
                    if ids.len() == 1 { "is" } else { "are" }
                ),
                fix,
            ));
        }
    }

    /// "`a`", "`a` and `b`", "`a`, `b` and `c`".
    fn names(&self, ids: &[DeclId]) -> String {
        let names: Vec<String> = ids
            .iter()
            .map(|id| format!("`{}`", self.spelling(*id)))
            .collect();
        match names.split_last() {
            Some((last, rest)) if !rest.is_empty() => format!("{} and {last}", rest.join(", ")),
            _ => names.concat(),
        }
    }

    fn spelling(&self, id: DeclId) -> &str {
        self.decl(id).map_or("", |d| d.spelling.as_str())
    }

    /// `q <= q;` for each register: reading an `out` port needs VHDL-2008,
    /// so the fix is unsafe for those.
    fn holds(&self, ids: &[DeclId], indent: &str) -> (Vec<String>, bool) {
        let lines = ids
            .iter()
            .map(|id| {
                let name = self.spelling(*id);
                format!("{indent}{name} <= {name};")
            })
            .collect();
        let safe = ids.iter().all(|id| {
            !self.decl(*id).is_some_and(|d| {
                matches!(
                    d.kind,
                    DeclKind::Port {
                        mode: Mode::Out,
                        ..
                    }
                )
            })
        });
        (lines, safe)
    }

    /// Adds `q <= q;` at the end of a branch.
    fn hold_in_branch(
        &self,
        text: &str,
        branch: &Branch,
        indent: &str,
        ids: &[DeclId],
    ) -> Option<Fix> {
        let at = branch
            .statements
            .last()
            .map_or(branch.header_end, |s| s.text_range().end());
        let nl = newline(text);
        let (lines, safe) = self.holds(ids, indent);
        let edit = if rest_of_line_is_empty(text, at) {
            let end = line_end(text, at);
            Edit {
                range: TextRange::empty(end),
                text: format!("{nl}{}", lines.join(nl)),
            }
        } else {
            let (lines, _) = self.holds(ids, "");
            Edit {
                range: TextRange::empty(at),
                text: format!(" {}", lines.join(" ")),
            }
        };
        Some(Fix {
            title: format!("Keep the value of {} in this branch", self.names(ids)),
            edits: vec![edit],
            safe,
        })
    }

    /// Adds an `else` branch with `q <= q;` to an `if`.
    fn hold_in_else(
        &self,
        text: &str,
        statement: &SyntaxNode,
        indent: &str,
        ids: &[DeclId],
    ) -> Option<Fix> {
        let end = statement
            .children_with_tokens()
            .filter_map(|e| e.into_token())
            .find(|t| t.kind() == SyntaxKind::END_KW)?
            .text_range()
            .start();
        let nl = newline(text);
        let (lines, safe) = self.holds(ids, indent);
        let edit = if starts_line(text, end) {
            let line_start = end - TextSize::from(line_indent(text, end).len() as u32);
            Edit {
                range: TextRange::empty(line_start),
                text: format!("{}else{nl}{}{nl}", line_indent(text, end), lines.join(nl)),
            }
        } else {
            let (lines, _) = self.holds(ids, "");
            Edit {
                range: TextRange::empty(end),
                text: format!("else {} ", lines.join(" ")),
            }
        };
        Some(Fix {
            title: format!("Add an `else` that keeps the value of {}", self.names(ids)),
            edits: vec![edit],
            safe,
        })
    }

    /// Assigns the signals a default value before the statement. `None`
    /// when no signal's type has an obvious default, or when the
    /// statement does not start its line.
    fn defaults_fix(
        &self,
        text: &str,
        statement: &SyntaxNode,
        what: &str,
        ids: &[DeclId],
    ) -> Option<Fix> {
        let start = statement.text_range().start();
        if !starts_line(text, start) {
            return None;
        }
        let indent = line_indent(text, start);
        let assigned: Vec<(DeclId, String)> = ids
            .iter()
            .filter_map(|id| Some((*id, self.default_value(*id)?)))
            .collect();
        if assigned.is_empty() {
            return None;
        }
        let lines: String = assigned
            .iter()
            .map(|(id, value)| {
                format!(
                    "{indent}{} <= {value};{}",
                    self.spelling(*id),
                    newline(text)
                )
            })
            .collect();
        let named: Vec<DeclId> = assigned.iter().map(|(id, _)| *id).collect();
        Some(Fix {
            title: format!(
                "Assign {} a default value before the `{what}`",
                self.names(&named)
            ),
            edits: vec![Edit {
                range: TextRange::empty(start - TextSize::from(indent.len() as u32)),
                text: lines,
            }],
            safe: false,
        })
    }

    /// A default value of a signal's type: `'0'` for bits and logic,
    /// `(others => '0')` for vectors of them, `false`, `0` for `integer`
    /// and `natural`, else the type's first value.
    fn default_value(&self, id: DeclId) -> Option<String> {
        let decl = self.decl(id)?;
        let mark = self.decl(decl.subtype?)?;
        let scalar = |literals: &[String]| {
            if literals.iter().any(|l| l == "'0'") {
                Some("'0'".to_string())
            } else if literals.iter().any(|l| l == "false") {
                Some("false".to_string())
            } else {
                None
            }
        };
        match self.object_type(decl)? {
            TypeDef::Enum(literals) => {
                scalar(literals).or_else(|| Some(format!("{}'left", mark.spelling)))
            }
            TypeDef::Integer => Some(if matches!(mark.name.as_str(), "integer" | "natural") {
                "0".to_string()
            } else {
                format!("{}'low", mark.spelling)
            }),
            TypeDef::Array {
                indexes,
                element: Some(element),
            } if indexes.len() == 1 => match self.type_def(*element)? {
                TypeDef::Enum(literals) => scalar(literals).map(|v| format!("(others => {v})")),
                _ => None,
            },
            _ => None,
        }
    }
}

/// What the walk of one process needs.
struct Walk<'p> {
    partial: &'p FxHashSet<DeclId>,
    clocked: bool,
}

/// The ids in the order they are first assigned.
fn ordered(ids: &FxHashSet<DeclId>, first: &Assigned) -> Vec<DeclId> {
    let mut ids: Vec<(TextSize, DeclId)> = ids
        .iter()
        .map(|id| {
            (
                first.get(id).map_or(TextSize::default(), |r| r.start()),
                *id,
            )
        })
        .collect();
    ids.sort_by_key(|(start, _)| *start);
    ids.into_iter().map(|(_, id)| id).collect()
}

/// The branches of an `if` statement, in the order of [`if_branches`].
fn if_branch_list(node: &SyntaxNode) -> Vec<Branch> {
    use SyntaxKind::*;
    let mut keywords = Vec::new();
    let mut header_ends = Vec::new();
    for t in node.children_with_tokens().filter_map(|e| e.into_token()) {
        match t.kind() {
            IF_KW | ELSIF_KW => keywords.push((t.kind(), t.text_range())),
            ELSE_KW => {
                keywords.push((t.kind(), t.text_range()));
                header_ends.push(t.text_range().end());
            }
            THEN_KW => header_ends.push(t.text_range().end()),
            END_KW => break,
            _ => {}
        }
    }
    keywords
        .into_iter()
        .zip(header_ends)
        .zip(if_branches(node))
        .map(
            |(((keyword, keyword_range), header_end), (_, statements))| Branch {
                keyword,
                keyword_range,
                header_end,
                statements,
            },
        )
        .collect()
}

/// The alternatives of a `case`.
fn case_alternatives(node: &SyntaxNode) -> Vec<Branch> {
    node.children()
        .filter(|n| n.kind() == SyntaxKind::CASE_ALTERNATIVE)
        .filter_map(|alternative| {
            let tokens = || {
                alternative
                    .children_with_tokens()
                    .filter_map(|e| e.into_token())
            };
            let when = tokens().find(|t| t.kind() == SyntaxKind::WHEN_KW)?;
            let arrow = tokens().find(|t| t.kind() == SyntaxKind::ARROW)?;
            let statements = alternative
                .children_with_tokens()
                .skip_while(|e| e.kind() != SyntaxKind::ARROW)
                .filter_map(|e| e.into_node())
                .collect();
            Some(Branch {
                keyword: SyntaxKind::WHEN_KW,
                keyword_range: when.text_range(),
                header_end: arrow.text_range().end(),
                statements,
            })
        })
        .collect()
}

/// The leading whitespace of the line holding `offset`.
fn line_indent(text: &str, offset: TextSize) -> &str {
    let at = usize::from(offset);
    let start = text[..at].rfind('\n').map_or(0, |i| i + 1);
    let line = &text[start..];
    &line[..line.len() - line.trim_start_matches([' ', '\t']).len()]
}

/// Whether only whitespace precedes `offset` on its line.
fn starts_line(text: &str, offset: TextSize) -> bool {
    let at = usize::from(offset);
    let start = text[..at].rfind('\n').map_or(0, |i| i + 1);
    text[start..at].chars().all(|c| c == ' ' || c == '\t')
}

/// Whether only whitespace or a comment follows `offset` on its line.
fn rest_of_line_is_empty(text: &str, offset: TextSize) -> bool {
    let at = usize::from(offset);
    let end = usize::from(line_end(text, offset));
    let rest = text[at..end].trim();
    rest.is_empty() || rest.starts_with("--")
}

/// The offset of the end of the line holding `offset` (before `\r\n`).
fn line_end(text: &str, offset: TextSize) -> TextSize {
    let at = usize::from(offset);
    let end = text[at..].find('\n').map_or(text.len(), |i| at + i);
    let end = if text[..end].ends_with('\r') {
        end - 1
    } else {
        end
    };
    TextSize::from(end as u32)
}

/// The file's line ending.
fn newline(text: &str) -> &'static str {
    if text.contains("\r\n") { "\r\n" } else { "\n" }
}

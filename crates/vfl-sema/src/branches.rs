//! `sema/missing-branch-assignment`: branches of an `if` or `case` in a
//! process that leave out a signal the other branches assign. Every
//! signal a process drives must be assigned in every branch, unless it is
//! assigned before the statement.
//!
//! Computed on demand for the files the rule is on for, so that it costs
//! nothing otherwise.
//!
//! Fixes: in a clocked process, the missing branches assign the register
//! its own value (`q <= q;`), which keeps the design's meaning; in a
//! combinational process, the signals get a default assignment before the
//! statement, which removes the latch and so changes the design.

use rustc_hash::FxHashSet;
use vfl_syntax::{SyntaxKind, SyntaxNode, SyntaxToken, TextRange, TextSize};

use vfl_syntax::green_walk::{ChildTokens, descendants_where};

use crate::analysis::Analysis;
use crate::decl::{Decl, DeclId, DeclKind, FileId, Mode, Target, TypeDef, UnitId};
use crate::diagnostic::{Diagnostic, Edit, Fix, codes};
use crate::resolver::{is_clock_edge, mark_token};
use crate::rtl::assignment_target;
use crate::rtl::reset::{if_branches, process_body};

/// Signals assigned whole, with their first target range, in the order
/// they are first assigned.
type Assigned = Vec<(DeclId, TextRange)>;

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

impl Analysis {
    /// The `sema/missing-branch-assignment` findings of a file: in its
    /// combinational processes, and in the clocked branch of its clocked
    /// processes too when `clocked`.
    pub fn missing_branch_assignments(&self, file: FileId, clocked: bool) -> Vec<Diagnostic> {
        let Some(analyzed) = self.files().get(file as usize) else {
            return Vec::new();
        };
        if analyzed.builtin || analyzed.foreign.is_some() {
            return Vec::new();
        }
        let mut checker = Branches {
            analysis: self,
            text: &analyzed.text,
            units: self.file_units(file).collect(),
            found: Vec::new(),
        };
        let root = analyzed.syntax();
        for process in descendants_where(&root, |k| k == SyntaxKind::PROCESS_STMT) {
            checker.process(&process, clocked);
        }
        let mut found = checker.found;
        found.sort_by_key(|(range, ..)| range.start());
        found
            .into_iter()
            .map(|(range, message, fix)| Diagnostic {
                code: codes::MISSING_BRANCH_ASSIGNMENT,
                file,
                range,
                message,
                fix,
            })
            .collect()
    }
}

struct Branches<'a> {
    analysis: &'a Analysis,
    text: &'a str,
    /// The file's units, whose references are sorted by range.
    units: Vec<UnitId>,
    found: Vec<Found>,
}

impl<'a> Branches<'a> {
    fn decl(&self, id: DeclId) -> Option<&'a Decl> {
        self.analysis.decl(id)
    }

    fn type_def(&self, mut mark: DeclId) -> Option<&'a TypeDef> {
        for _ in 0..32 {
            let decl = self.decl(mark)?;
            match decl.kind {
                DeclKind::Type => return decl.type_def.as_deref(),
                DeclKind::Subtype => mark = decl.subtype?,
                _ => return None,
            }
        }
        None
    }

    fn object_type(&self, decl: &Decl) -> Option<&'a TypeDef> {
        self.type_def(decl.subtype?)
    }

    /// The signal a target token writes.
    fn written_signal(&self, token: &SyntaxToken) -> Option<DeclId> {
        let range = token.text_range();
        let r = self.units.iter().find_map(|unit| {
            let refs = self.analysis.unit_references(*unit);
            let i = refs.partition_point(|r| r.range.start() < range.start());
            refs[i..]
                .iter()
                .take_while(|r| r.range.start() == range.start())
                .find(|r| r.range == range)
        })?;
        if !r.access.writes() {
            return None;
        }
        r.targets.iter().find_map(|t| match t {
            Target::Decl(id) => self.decl(*id).filter(|d| d.kind.is_signal()).map(|_| *id),
            Target::Library(_) => None,
        })
    }

    /// Checks a process: combinational ones (a sensitivity list and no
    /// `wait`, or one trailing `wait on`, and no clock edge), and the
    /// clocked branch of clocked ones without `wait` when `clocked`.
    /// Processes with `after` delays are simulation models and left out.
    fn process(&mut self, process: &SyntaxNode, clocked: bool) {
        let mut waits = 0;
        let mut edge = None;
        let wanted = |k| {
            use SyntaxKind::*;
            matches!(
                k,
                WAIT_STMT | ERROR | WAVEFORM_ELEMENT | CALL_EXPR | ATTRIBUTE_NAME
            )
        };
        for n in descendants_where(process, wanted) {
            match n.kind() {
                SyntaxKind::WAIT_STMT => waits += 1,
                SyntaxKind::ERROR => return,
                SyntaxKind::WAVEFORM_ELEMENT if n.has_child(|k| k == SyntaxKind::AFTER_KW) => {
                    return;
                }
                SyntaxKind::CALL_EXPR | SyntaxKind::ATTRIBUTE_NAME
                    if edge.is_none() && is_clock_edge(&n) =>
                {
                    edge = Some(n)
                }
                _ => {}
            }
        }
        let body = process_body(process);
        match edge {
            Some(edge) if clocked && waits == 0 => {
                let [statement] = body.as_slice() else {
                    return;
                };
                if statement.kind() != SyntaxKind::IF_STMT {
                    return;
                }
                if let Some((_, branch)) = if_branches(statement).into_iter().find(|(c, _)| {
                    c.as_ref()
                        .is_some_and(|c| c.text_range().contains_range(edge.text_range()))
                }) {
                    self.branches(&branch, true);
                }
            }
            Some(_) => {}
            None => {
                let has_list = process
                    .children()
                    .any(|n| n.kind() == SyntaxKind::SENSITIVITY_LIST);
                // `wait on a, b;` as the last statement is a sensitivity list.
                let trailing_wait_on = waits == 1
                    && body.last().is_some_and(|last| {
                        last.kind() == SyntaxKind::WAIT_STMT && {
                            let has = |kind| last.has_child(|k| k == kind);
                            has(SyntaxKind::ON_KW)
                                && !has(SyntaxKind::UNTIL_KW)
                                && !has(SyntaxKind::FOR_KW)
                        }
                    });
                let combinational = if has_list {
                    waits == 0
                } else {
                    trailing_wait_on
                };
                if combinational {
                    self.branches(&body, false);
                }
            }
        }
    }

    /// The signals a statement assigns in part (an element or a slice),
    /// looking into compound statements but not into expressions.
    fn partial_writes(&self, statement: &SyntaxNode, out: &mut FxHashSet<DeclId>) {
        use SyntaxKind::*;
        match statement.kind() {
            SIGNAL_ASSIGN_STMT | SELECTED_ASSIGN_STMT => {
                if let Some((target, _)) = assignment_target(statement)
                    && !matches!(target.kind(), NAME | AGGREGATE)
                    && let Some(token) = target
                        .descendants()
                        .find(|n| n.kind() == NAME)
                        .and_then(|n| mark_token(&n))
                    && let Some(id) = self.written_signal(&token)
                {
                    out.insert(id);
                }
            }
            IF_STMT | CASE_STMT | CASE_ALTERNATIVE | LOOP_STMT | SEQUENTIAL_BLOCK_STMT => {
                for child in statement.children() {
                    self.partial_writes(&child, out);
                }
            }
            _ => {}
        }
    }

    fn branches(&mut self, body: &[SyntaxNode], clocked: bool) {
        let mut partial = FxHashSet::default();
        for statement in body {
            self.partial_writes(statement, &mut partial);
        }
        let mut scan = Scan {
            partial: &partial,
            clocked,
            done: Vec::new(),
            found: Vec::new(),
        };
        let mut assigned = Vec::new();
        self.branch_sequence(body, &mut scan, &mut assigned);
        self.found.append(&mut scan.found);
    }

    /// Walks a sequence of statements, adding to `scan.done` the signals
    /// it assigns on every path and to `assigned` the needed signals it
    /// assigns anywhere, with their first target.
    fn branch_sequence(&self, statements: &[SyntaxNode], scan: &mut Scan, assigned: &mut Assigned) {
        use SyntaxKind::*;
        for statement in statements {
            match statement.kind() {
                IF_STMT | CASE_STMT => self.branch_statement(statement, scan, assigned),
                LOOP_STMT | SEQUENTIAL_BLOCK_STMT => {
                    // A loop is assumed to run its body.
                    let body: Vec<SyntaxNode> = statement
                        .children()
                        .filter(|n| !matches!(n.kind(), LABEL | FOR_SCHEME | WHILE_SCHEME))
                        .collect();
                    self.branch_sequence(&body, scan, assigned);
                }
                SIGNAL_ASSIGN_STMT | SELECTED_ASSIGN_STMT => {
                    let Some((target, complete)) = assignment_target(statement) else {
                        continue;
                    };
                    let mut record = |id: DeclId, range: TextRange| {
                        if scan.needed(id) {
                            if !assigned.iter().any(|(a, _)| *a == id) {
                                assigned.push((id, range));
                            }
                            if complete {
                                scan.done.push(id);
                            }
                        }
                    };
                    match target.kind() {
                        NAME => {
                            if let Some(token) = mark_token(&target)
                                && let Some(id) = self.written_signal(&token)
                            {
                                record(id, token.text_range());
                            }
                        }
                        AGGREGATE => {
                            for name in target.descendants().filter(|n| n.kind() == NAME) {
                                if let Some(token) = mark_token(&name)
                                    && let Some(id) = self.written_signal(&token)
                                {
                                    record(id, token.text_range());
                                }
                            }
                        }
                        // Assigned in part: left alone.
                        _ => {}
                    }
                }
                PROCEDURE_CALL_STMT => {
                    // As for latches: signals passed whole to a procedure
                    // count as assigned, but are not compared.
                    for name in statement
                        .descendants()
                        .filter(|n| n.kind() == ASSOCIATION_ELEMENT)
                        .filter_map(|e| e.children().last())
                        .filter(|n| n.kind() == NAME)
                    {
                        if let Some(token) = mark_token(&name)
                            && let Some(id) = self.written_signal(&token)
                            && scan.needed(id)
                        {
                            scan.done.push(id);
                        }
                    }
                }
                _ => {}
            }
        }
    }

    fn branch_statement(&self, statement: &SyntaxNode, scan: &mut Scan, assigned: &mut Assigned) {
        let is_if = statement.kind() == SyntaxKind::IF_STMT;
        let branches = if is_if {
            if_branch_list(statement)
        } else {
            case_alternatives(statement)
        };
        // Each branch is walked once, for what it assigns anywhere and on
        // every path.
        let mut first = Assigned::new();
        let mark = scan.done.len();
        let walked: Vec<Vec<DeclId>> = branches
            .iter()
            .map(|branch| {
                self.branch_sequence(&branch.statements, scan, &mut first);
                scan.done.drain(mark..).collect()
            })
            .collect();
        // The signals this statement assigns that no earlier statement
        // assigns on every path.
        if first.is_empty() {
            return;
        }
        for (id, range) in &first {
            if !assigned.iter().any(|(a, _)| a == id) {
                assigned.push((*id, *range));
            }
        }
        // Reported once, here.
        scan.done.extend(first.iter().map(|(id, _)| *id));
        let first = &first;
        let clocked = scan.clocked;
        let what = if is_if { "if" } else { "case" };
        // In a clocked process, `if rst = '1'` resets the registers that
        // have a reset value: its branches are checked on their own.
        let resets = clocked
            && is_if
            && statement
                .children()
                .next()
                .is_some_and(|c| is_reset_test(&c));
        // What each branch misses, then what a missing `else` misses.
        let mut gaps: Vec<(&Branch, Vec<DeclId>)> = Vec::new();
        for (branch, done) in branches.iter().zip(walked) {
            // A reset branch sets the registers that have a reset value;
            // the other branches are checked on their own.
            let missing: Vec<DeclId> = first
                .iter()
                .map(|(id, _)| *id)
                .filter(|id| !done.contains(id))
                .collect();
            if !resets && !missing.is_empty() {
                gaps.push((branch, missing));
            }
        }
        let no_else =
            is_if && !resets && !branches.iter().any(|b| b.keyword == SyntaxKind::ELSE_KW);
        let unassigned = no_else.then(|| first.iter().map(|(id, _)| *id).collect::<Vec<_>>());
        if gaps.is_empty() && unassigned.is_none() {
            return;
        }

        let text = self.text;
        // The indentation of the statements of the branches.
        let indent = branches
            .iter()
            .find_map(|b| b.statements.first())
            .map(|s| line_indent(text, s.text_range().start()))
            .unwrap_or_default();
        let defaults = (!clocked)
            .then(|| {
                let all: Vec<DeclId> = first
                    .iter()
                    .map(|(id, _)| *id)
                    .filter(|id| {
                        unassigned.is_some() || gaps.iter().any(|(_, ids)| ids.contains(id))
                    })
                    .collect();
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
            let fix = if clocked {
                self.hold_in_branch(text, branch, indent, &ids)
            } else {
                defaults.clone()
            };
            scan.found.push((
                branch.keyword_range,
                format!(
                    "{label} does not assign {}, which another branch of the `{what}` assigns",
                    self.names(&ids)
                ),
                fix,
            ));
        }
        if let (Some(ids), Some(first_branch)) = (unassigned, branches.first()) {
            let fix = if clocked {
                self.hold_in_else(text, statement, indent, &ids)
            } else {
                defaults
            };
            scan.found.push((
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
        let nl = newline(text, at);
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
        let nl = newline(text, end);
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
                    newline(text, start)
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

/// The state of the walk of one process.
struct Scan<'p> {
    /// Signals assigned in part somewhere, which are left alone.
    partial: &'p FxHashSet<DeclId>,
    clocked: bool,
    /// The signals assigned on every path so far, in the enclosing
    /// sequences: a stack, each branch truncating it back.
    done: Vec<DeclId>,
    found: Vec<Found>,
}

impl Scan<'_> {
    fn needed(&self, id: DeclId) -> bool {
        !self.done.contains(&id) && !self.partial.contains(&id)
    }
}

/// A test of a reset signal: `rst`, `rst = '1'`, `not rst_n`,
/// `(reset /= '0')`, by its name.
fn is_reset_test(cond: &SyntaxNode) -> bool {
    use SyntaxKind::*;
    match cond.kind() {
        PAREN_EXPR | UNARY_EXPR => cond.children().next().is_some_and(|c| is_reset_test(&c)),
        BINARY_EXPR => cond
            .children()
            .any(|c| c.kind() == NAME && is_reset_test(&c)),
        NAME => mark_token(cond).is_some_and(|t| {
            let name = t.text().to_ascii_lowercase();
            name.contains("rst") || name.contains("reset")
        }),
        _ => false,
    }
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

/// The line ending of the line holding `offset`.
fn newline(text: &str, offset: TextSize) -> &'static str {
    let end = usize::from(line_end(text, offset));
    if text[end..].starts_with("\r\n") {
        "\r\n"
    } else {
        "\n"
    }
}

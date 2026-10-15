//! `sema/multiple-drivers`: signals driven by more than one process,
//! concurrent statement or instance of an architecture; and
//! `sema/combinational-loop`: signals that depend on themselves through
//! concurrent assignments and combinational processes, with no register on
//! the way.
//!
//! Computed on demand for the files the rules are on for, so that they
//! cost nothing otherwise. Both look at the statements of each
//! architecture, its blocks and its `for` generates, and at what each
//! statement writes and reads, from the references name resolution
//! recorded: one pass over them.
//!
//! What a statement drives is its targets, down to static parts: `v(3)`,
//! `v(7 downto 4)` and `r.field` are parts, which do not overlap when
//! their indexes or fields differ. A part whose index is not a static
//! value (`v(i)` in a `for` generate or a loop) is assumed not to overlap
//! other parts, so that it is never reported. Statements under an `if` or
//! `case` generate are left out: their alternatives never coexist.
//!
//! A combinational statement is a concurrent assignment without a clock
//! edge or a delay, or a process with a sensitivity list (or one trailing
//! `wait on`) and neither. An assignment in it depends on the signals it
//! reads and on those of the conditions it is under. Dependencies through
//! variables and procedure calls are not followed.

use rustc_hash::FxHashMap;
use vfl_syntax::green_walk::{ChildTokens, for_each_token};
use vfl_syntax::{SyntaxKind, SyntaxNode, SyntaxToken, TextRange};

use crate::analysis::Analysis;
use crate::decl::{Access, Decl, DeclId, DeclKind, FileId, Reference, Target, TypeDef};
use crate::diagnostic::{Diagnostic, codes};
use crate::resolver::mark_token;
use crate::rtl::static_int;

/// Which signals of a resolved type (`std_logic`) `sema/multiple-drivers`
/// reports: the `resolved` option.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ResolvedDrivers {
    /// In every architecture but testbenches (of entities without ports).
    #[default]
    Rtl,
    /// In every architecture.
    Always,
    /// Never: only signals of unresolved types are reported.
    Never,
}

/// Which of the two checks to run.
#[derive(Debug, Clone, Copy, Default)]
pub struct DriverChecks {
    /// `sema/multiple-drivers`, with its `resolved` option.
    pub multiple: Option<ResolvedDrivers>,
    /// `sema/combinational-loop`.
    pub loops: bool,
}

/// How deep types and names are followed.
const MAX_DEPTH: usize = 32;

/// One step of a part of a signal: `v(3)`, `v(7 downto 4)`, `r.f`.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Step {
    /// Indexes from `.0` to `.1`.
    Range(i64, i64),
    /// A record element, by key.
    Field(String),
    /// An index that is not a static value.
    Unknown,
}

/// Whether two parts of a signal may overlap. Parts whose indexes are not
/// known are assumed not to.
fn overlap(a: &[Step], b: &[Step]) -> bool {
    for (x, y) in a.iter().zip(b) {
        match (x, y) {
            (Step::Range(l1, h1), Step::Range(l2, h2)) if l1 > h2 || l2 > h1 => return false,
            (Step::Field(f), Step::Field(g)) if f != g => return false,
            (Step::Unknown, _) | (_, Step::Unknown) => return false,
            _ => {}
        }
    }
    true
}

/// A signal a statement accesses, and which part.
struct Named {
    signal: DeclId,
    part: Vec<Step>,
    /// The name's range.
    range: TextRange,
}

/// A statement that drives signals: a process, a concurrent assignment,
/// an instance.
struct Driver {
    node: SyntaxNode,
    writes: Vec<Named>,
}

/// An assignment of a combinational statement: the signal part it writes
/// and the signals it reads (its values, indexes and the conditions it is
/// under).
struct Assignment {
    write: Named,
    reads: Vec<usize>,
}

impl Analysis {
    /// The `sema/multiple-drivers` and `sema/combinational-loop` findings
    /// of a file.
    pub fn driver_checks(&self, file: FileId, checks: DriverChecks) -> Vec<Diagnostic> {
        let Some(analyzed) = self.files().get(file as usize) else {
            return Vec::new();
        };
        if analyzed.builtin
            || analyzed.foreign.is_some()
            || (checks.multiple.is_none() && !checks.loops)
        {
            return Vec::new();
        }
        let root = analyzed.syntax();
        let design_units: Vec<SyntaxNode> = root
            .children()
            .filter(|n| n.kind() == SyntaxKind::DESIGN_UNIT)
            .collect();
        let mut out = Vec::new();
        for unit in self.file_units(file) {
            let info = &self.units()[unit as usize];
            if info.kind != DeclKind::Architecture {
                continue;
            }
            let Some(body) = design_units.get(info.index).and_then(|u| {
                u.children()
                    .find(|n| n.kind() == SyntaxKind::ARCHITECTURE_BODY)
            }) else {
                continue;
            };
            if body.descendants().any(|n| n.kind() == SyntaxKind::ERROR) {
                continue;
            }
            let mut arch = Architecture {
                analysis: self,
                file,
                refs: self.unit_references(unit),
                reads: Vec::new(),
                out: &mut out,
            };
            let mut statements = Vec::new();
            concurrent_statements(&body, &mut statements);
            if let Some(resolved) = checks.multiple {
                arch.multiple_drivers(&body, &statements, resolved);
            }
            if checks.loops {
                arch.loops(&statements);
            }
        }
        out.sort_by_key(|d| d.range.start());
        out
    }
}

/// The statements of an architecture, its blocks and its `for` generates
/// that may drive signals.
fn concurrent_statements(node: &SyntaxNode, out: &mut Vec<SyntaxNode>) {
    use SyntaxKind::*;
    for child in node.children() {
        match child.kind() {
            PROCESS_STMT
            | SIGNAL_ASSIGN_STMT
            | SELECTED_ASSIGN_STMT
            | COMPONENT_INSTANTIATION_STMT
            | PROCEDURE_CALL_STMT => out.push(child),
            BLOCK_STMT | FOR_GENERATE_STMT | GENERATE_BODY => concurrent_statements(&child, out),
            _ => {}
        }
    }
}

struct Architecture<'a> {
    analysis: &'a Analysis,
    file: FileId,
    /// The unit's references, in source order.
    refs: &'a [Reference],
    /// Signal reads of the assignments, by index in `Assignment::reads`.
    reads: Vec<Named>,
    out: &'a mut Vec<Diagnostic>,
}

impl<'a> Architecture<'a> {
    fn decl(&self, id: DeclId) -> Option<&'a Decl> {
        self.analysis.decl(id)
    }

    /// The references inside `range`.
    fn refs_in(&self, range: TextRange) -> &'a [Reference] {
        let from = self
            .refs
            .partition_point(|r| r.range.start() < range.start());
        let to = self.refs.partition_point(|r| r.range.start() < range.end());
        &self.refs[from..to.max(from)]
    }

    /// The single declaration the name at `range` refers to.
    fn target_at(&self, range: TextRange) -> Option<DeclId> {
        let refs = self.refs_in(range);
        let r = refs.iter().find(|r| r.range == range)?;
        let mut ids = r.targets.iter().filter_map(|t| match t {
            Target::Decl(id) => Some(*id),
            Target::Library(_) => None,
        });
        let id = ids.next()?;
        ids.next().is_none().then_some(id)
    }

    fn signal_of(&self, r: &Reference) -> Option<DeclId> {
        r.targets.iter().find_map(|t| match t {
            Target::Decl(id) => self.decl(*id).filter(|d| d.kind.is_signal()).map(|_| *id),
            Target::Library(_) => None,
        })
    }

    fn line(&self, range: TextRange) -> u32 {
        let file = &self.analysis.files()[self.file as usize];
        file.line_index.line_col(&file.text, range.start()).line + 1
    }

    fn name(&self, id: DeclId) -> String {
        self.decl(id)
            .map(|d| d.spelling.clone())
            .unwrap_or_default()
    }

    /// The signal accesses inside `node` that `keep` accepts.
    fn accesses(&self, node: &SyntaxNode, keep: impl Fn(Access) -> bool) -> Vec<Named> {
        self.refs_in(node.text_range())
            .iter()
            .filter(|r| keep(r.access))
            .filter_map(|r| {
                let signal = self.signal_of(r)?;
                Some(Named {
                    signal,
                    part: self.part(node, r.range),
                    range: r.range,
                })
            })
            .collect()
    }

    /// The part of its signal that the name at `range` denotes, inside
    /// `node`.
    fn part(&self, node: &SyntaxNode, range: TextRange) -> Vec<Step> {
        use SyntaxKind::*;
        let token = match node.covering_element(range) {
            rowan::NodeOrToken::Token(t) => t,
            rowan::NodeOrToken::Node(n) => match n.first_token() {
                Some(t) => t,
                None => return Vec::new(),
            },
        };
        let Some(mut name) = token.parent() else {
            return Vec::new();
        };
        let mut part = Vec::new();
        for _ in 0..MAX_DEPTH {
            let Some(parent) = name.parent() else {
                break;
            };
            // Only the prefix of an index, a slice or a selected name.
            if parent.first_child().as_ref() != Some(&name) {
                break;
            }
            match parent.kind() {
                SELECTED_NAME => {
                    let Some(field) = parent
                        .children_with_tokens()
                        .filter_map(|e| e.into_token())
                        .filter(|t| matches!(t.kind(), IDENT | EXT_IDENT))
                        .last()
                    else {
                        break;
                    };
                    part.push(Step::Field(key(&field)));
                }
                CALL_EXPR => {
                    let Some(list) = parent.children().find(|n| n.kind() == ASSOCIATION_LIST)
                    else {
                        break;
                    };
                    let mut elements = list.children();
                    let step = match (elements.next(), elements.next()) {
                        (Some(element), None) => element
                            .children()
                            .next()
                            .and_then(|index| self.index_step(&index))
                            .unwrap_or(Step::Unknown),
                        _ => Step::Unknown,
                    };
                    part.push(step);
                }
                _ => break,
            }
            name = parent;
        }
        part
    }

    /// The indexes an index or a discrete range denotes, when static.
    fn index_step(&self, index: &SyntaxNode) -> Option<Step> {
        let value = |n: &SyntaxNode| {
            let token = mark_token(n)?;
            self.decl(self.target_at(token.text_range())?)?.value
        };
        if index.kind() == SyntaxKind::RANGE_EXPR {
            let mut bounds = index.children();
            let left = static_int(&bounds.next()?, &value)?;
            let right = static_int(&bounds.next()?, &value)?;
            return Some(Step::Range(left.min(right), left.max(right)));
        }
        let v = static_int(index, &value)?;
        Some(Step::Range(v, v))
    }

    // ---- Multiple drivers ---------------------------------------------------

    fn multiple_drivers(
        &mut self,
        body: &SyntaxNode,
        statements: &[SyntaxNode],
        resolved_option: ResolvedDrivers,
    ) {
        // Worked out for the first resolved signal with several drivers.
        let mut report_resolved = None;
        let drivers: Vec<Driver> = statements
            .iter()
            .map(|node| Driver {
                writes: self.accesses(node, |a| a == Access::Write),
                node: node.clone(),
            })
            .filter(|d| !d.writes.is_empty())
            .collect();
        // The drivers of each signal, in source order.
        let mut by_signal: FxHashMap<DeclId, Vec<usize>> = FxHashMap::default();
        for (i, driver) in drivers.iter().enumerate() {
            for write in &driver.writes {
                let list = by_signal.entry(write.signal).or_default();
                if list.last() != Some(&i) {
                    list.push(i);
                }
            }
        }
        let mut signals: Vec<(DeclId, Vec<usize>)> = by_signal
            .into_iter()
            .filter(|(_, list)| list.len() > 1)
            .collect();
        signals.sort_by_key(|(id, _)| *id);
        for (signal, list) in signals {
            let resolved = self
                .decl(signal)
                .and_then(|d| d.subtype)
                .is_some_and(|mark| self.resolved(mark, 0));
            if resolved {
                let report = *report_resolved.get_or_insert_with(|| match resolved_option {
                    ResolvedDrivers::Always => true,
                    ResolvedDrivers::Never => false,
                    ResolvedDrivers::Rtl => !self.is_testbench(body),
                });
                if !report || list.iter().any(|&i| tri_state(&drivers[i].node)) {
                    continue;
                }
            }
            // Each driver whose write overlaps one of an earlier driver.
            for (n, &later) in list.iter().enumerate().skip(1) {
                let found = drivers[later]
                    .writes
                    .iter()
                    .filter(|w| w.signal == signal)
                    .find_map(|w| {
                        list[..n].iter().find_map(|&earlier| {
                            drivers[earlier]
                                .writes
                                .iter()
                                .filter(|e| e.signal == signal)
                                .find(|e| overlap(&e.part, &w.part))
                                .map(|e| (w.range, e.range))
                        })
                    });
                let Some((range, first)) = found else {
                    continue;
                };
                let name = self.name(signal);
                let line = self.line(first);
                let message = if resolved {
                    format!(
                        "`{name}` is also driven by the statement at line {line}; its type is resolved, but two drivers are rarely intended outside a tri-state bus"
                    )
                } else {
                    format!(
                        "`{name}` is also driven by the statement at line {line}; a signal of an unresolved type can have only one driver"
                    )
                };
                self.out.push(Diagnostic {
                    code: codes::MULTIPLE_DRIVERS,
                    file: self.file,
                    range,
                    message,
                    fix: None,
                });
            }
        }
    }

    /// Whether a type or subtype has a resolution function, or is a
    /// composite whose elements all do (`std_logic_vector` before
    /// VHDL-2008).
    fn resolved(&self, mut mark: DeclId, depth: usize) -> bool {
        for _ in 0..MAX_DEPTH {
            if depth > MAX_DEPTH {
                return false;
            }
            let Some(decl) = self.decl(mark) else {
                return false;
            };
            match decl.kind {
                DeclKind::Subtype => {
                    if self.has_resolution(decl) {
                        return true;
                    }
                    match decl.subtype {
                        Some(next) => mark = next,
                        None => return false,
                    }
                }
                DeclKind::Type => {
                    return match decl.type_def.as_deref() {
                        Some(TypeDef::Array {
                            element: Some(element),
                            ..
                        }) => self.resolved(*element, depth + 1),
                        Some(TypeDef::Record(elements)) => {
                            !elements.is_empty()
                                && elements
                                    .iter()
                                    .all(|e| e.subtype.is_some_and(|s| self.resolved(s, depth + 1)))
                        }
                        _ => false,
                    };
                }
                _ => return false,
            }
        }
        false
    }

    /// Whether a subtype declaration names a resolution function:
    /// `subtype std_logic is resolved std_ulogic`.
    fn has_resolution(&self, decl: &Decl) -> bool {
        let Some(file) = self.analysis.files().get(decl.file as usize) else {
            return false;
        };
        let root = file.syntax();
        let node = match root.covering_element(decl.range) {
            rowan::NodeOrToken::Node(n) => n,
            rowan::NodeOrToken::Token(t) => match t.parent() {
                Some(p) => p,
                None => return false,
            },
        };
        node.ancestors()
            .find(|n| n.kind() == SyntaxKind::SUBTYPE_DECL)
            .and_then(|d| {
                d.children()
                    .find(|n| n.kind() == SyntaxKind::SUBTYPE_INDICATION)
            })
            .is_some_and(|i| {
                i.children()
                    .any(|n| n.kind() == SyntaxKind::RESOLUTION_INDICATION)
            })
    }

    /// Whether an architecture is a testbench's: its entity has no ports.
    fn is_testbench(&self, body: &SyntaxNode) -> bool {
        let Some(entity) = body
            .children()
            .find(|n| n.kind() == SyntaxKind::NAME)
            .and_then(|n| mark_token(&n))
            .and_then(|t| self.target_at(t.text_range()))
            .and_then(|id| self.decl(id))
        else {
            return false;
        };
        entity.members.as_ref().is_some_and(|members| {
            !members.iter().any(|id| {
                self.decl(id)
                    .is_some_and(|d| matches!(d.kind, DeclKind::Port { .. }))
            })
        })
    }

    // ---- Combinational loops ------------------------------------------------

    fn loops(&mut self, statements: &[SyntaxNode]) {
        use SyntaxKind::*;
        let mut assignments: Vec<Assignment> = Vec::new();
        for statement in statements {
            match statement.kind() {
                SIGNAL_ASSIGN_STMT | SELECTED_ASSIGN_STMT => {
                    if !combinational(statement) {
                        continue;
                    }
                    self.assignment(statement, &[], &mut assignments);
                }
                PROCESS_STMT => {
                    if !combinational_process(statement) {
                        continue;
                    }
                    let body: Vec<SyntaxNode> = statement
                        .children_with_tokens()
                        .skip_while(|e| e.kind() != BEGIN_KW)
                        .filter_map(|e| e.into_node())
                        .collect();
                    let mut conditions = Vec::new();
                    self.sequence(&body, &mut conditions, &mut assignments);
                }
                _ => {}
            }
        }
        if assignments.is_empty() {
            return;
        }
        self.cycles(&assignments);
    }

    /// Records the assignments of a sequence of statements, under the
    /// signals in `conditions`.
    fn sequence(
        &mut self,
        statements: &[SyntaxNode],
        conditions: &mut Vec<usize>,
        out: &mut Vec<Assignment>,
    ) {
        use SyntaxKind::*;
        for statement in statements {
            match statement.kind() {
                SIGNAL_ASSIGN_STMT | SELECTED_ASSIGN_STMT => {
                    let conditions = conditions.clone();
                    self.assignment(statement, &conditions, out);
                }
                IF_STMT | CASE_STMT | LOOP_STMT | SEQUENTIAL_BLOCK_STMT | CASE_ALTERNATIVE => {
                    let depth = conditions.len();
                    // Conditions, selectors and loop schemes.
                    let mut nested = Vec::new();
                    for child in statement.children() {
                        match child.kind() {
                            k if is_sequential(k) || k == CASE_ALTERNATIVE => nested.push(child),
                            LABEL | CHOICES => {}
                            _ => {
                                for read in self.accesses(&child, Access::reads) {
                                    conditions.push(self.reads.len());
                                    self.reads.push(read);
                                }
                            }
                        }
                    }
                    self.sequence(&nested, conditions, out);
                    conditions.truncate(depth);
                }
                _ => {}
            }
        }
    }

    /// Records an assignment, which depends on what it reads and on
    /// `conditions`.
    fn assignment(&mut self, node: &SyntaxNode, conditions: &[usize], out: &mut Vec<Assignment>) {
        let writes = self.accesses(node, |a| a == Access::Write);
        if writes.is_empty() {
            return;
        }
        let mut reads: Vec<usize> = conditions.to_vec();
        for read in self.accesses(node, Access::reads) {
            // `q <= q;` keeps a value: a latch rather than a loop.
            if writes.iter().any(|w| w.signal == read.signal) && keeps_value(node, read.range) {
                continue;
            }
            reads.push(self.reads.len());
            self.reads.push(read);
        }
        for write in writes {
            out.push(Assignment {
                write,
                reads: reads.clone(),
            });
        }
    }

    /// Finds the cycles of the graph whose nodes are the assignments, with
    /// an edge from an assignment to those that read what it writes.
    fn cycles(&mut self, assignments: &[Assignment]) {
        let mut writers: FxHashMap<DeclId, Vec<usize>> = FxHashMap::default();
        for (i, a) in assignments.iter().enumerate() {
            writers.entry(a.write.signal).or_default().push(i);
        }
        // `edges[i]`: the assignments that read what `i` writes.
        let mut edges: Vec<Vec<usize>> = vec![Vec::new(); assignments.len()];
        for (j, a) in assignments.iter().enumerate() {
            for &r in &a.reads {
                let read = &self.reads[r];
                let Some(list) = writers.get(&read.signal) else {
                    continue;
                };
                for &i in list {
                    if overlap(&assignments[i].write.part, &read.part) && !edges[i].contains(&j) {
                        edges[i].push(j);
                    }
                }
            }
        }
        for component in strongly_connected(&edges) {
            let cyclic = component.len() > 1 || edges[component[0]].contains(&component[0]);
            if !cyclic {
                continue;
            }
            let path = cycle(&edges, &component);
            // Reported at the assignment of the cycle that comes first.
            let Some(&first) = path
                .iter()
                .min_by_key(|&&i| assignments[i].write.range.start())
            else {
                continue;
            };
            let at = path.iter().position(|&i| i == first).unwrap_or(0);
            let mut names: Vec<String> = path[at..]
                .iter()
                .chain(&path[..at])
                .map(|&i| format!("`{}`", self.name(assignments[i].write.signal)))
                .collect();
            names.dedup();
            names.push(names[0].clone());
            let message = if names.len() == 2 {
                format!(
                    "combinational loop: {} depends on itself with no register on the way",
                    names[0]
                )
            } else {
                format!(
                    "combinational loop with no register on the way: {}",
                    names.join(" -> ")
                )
            };
            self.out.push(Diagnostic {
                code: codes::COMBINATIONAL_LOOP,
                file: self.file,
                range: assignments[first].write.range,
                message,
                fix: None,
            });
        }
    }
}

/// The lookup key of an identifier.
fn key(token: &SyntaxToken) -> String {
    if token.kind() == SyntaxKind::EXT_IDENT {
        token.text().to_owned()
    } else {
        token.text().to_ascii_lowercase()
    }
}

fn is_sequential(kind: SyntaxKind) -> bool {
    use SyntaxKind::*;
    matches!(
        kind,
        SIGNAL_ASSIGN_STMT
            | SELECTED_ASSIGN_STMT
            | VARIABLE_ASSIGN_STMT
            | IF_STMT
            | CASE_STMT
            | LOOP_STMT
            | SEQUENTIAL_BLOCK_STMT
            | PROCEDURE_CALL_STMT
            | WAIT_STMT
            | NULL_STMT
            | NEXT_STMT
            | EXIT_STMT
            | RETURN_STMT
            | ASSERT_STMT
            | REPORT_STMT
    )
}

/// Whether a statement drives `'Z'` on some path: a tri-state driver.
fn tri_state(node: &SyntaxNode) -> bool {
    let mut z = false;
    for_each_token(node, |kind, _, text| {
        use SyntaxKind::*;
        if matches!(kind, CHAR_LITERAL | STRING_LITERAL | BIT_STRING_LITERAL)
            && text.contains(['Z', 'z'])
        {
            z = true;
        }
    });
    z
}

/// Whether a concurrent assignment is combinational: no clock edge and no
/// delay (`clk <= not clk after 5 ns` is a clock generator).
fn combinational(node: &SyntaxNode) -> bool {
    let mut previous = SyntaxKind::ERROR;
    let mut combinational = true;
    for_each_token(node, |kind, _, text| {
        use SyntaxKind::*;
        if kind.is_trivia() {
            return;
        }
        if kind == AFTER_KW
            || (kind == IDENT
                && (text.eq_ignore_ascii_case("rising_edge")
                    || text.eq_ignore_ascii_case("falling_edge")
                    || (previous == TICK
                        && (text.eq_ignore_ascii_case("event")
                            || text.eq_ignore_ascii_case("stable")))))
        {
            combinational = false;
        }
        previous = kind;
    });
    combinational
}

/// Whether a process is combinational: a sensitivity list and no `wait`,
/// or one trailing `wait on`; no clock edge, no delay.
fn combinational_process(process: &SyntaxNode) -> bool {
    use SyntaxKind::*;
    if !combinational(process) {
        return false;
    }
    let mut waits = 0;
    for_each_token(process, |kind, _, _| {
        if kind == WAIT_KW {
            waits += 1;
        }
    });
    if process.children().any(|n| n.kind() == SENSITIVITY_LIST) {
        return waits == 0;
    }
    let last = process
        .children_with_tokens()
        .skip_while(|e| e.kind() != BEGIN_KW)
        .filter_map(|e| e.into_node())
        .last();
    waits == 1
        && last.is_some_and(|last| {
            last.kind() == WAIT_STMT && {
                let has = |kind| last.has_child(|k| k == kind);
                has(ON_KW) && !has(UNTIL_KW) && !has(FOR_KW)
            }
        })
}

/// Whether the name at `range` is the whole value of a waveform element:
/// `q <= q`, `q <= d when en else q`, `r.f <= r.f`.
fn keeps_value(statement: &SyntaxNode, range: TextRange) -> bool {
    use SyntaxKind::*;
    let Some(mut name) = statement
        .covering_element(range)
        .into_token()
        .and_then(|t| t.parent())
    else {
        return false;
    };
    // Up to the whole name: `r.f`, `v(3)`.
    while let Some(parent) = name.parent() {
        if !matches!(parent.kind(), SELECTED_NAME | CALL_EXPR)
            || parent.first_child().as_ref() != Some(&name)
        {
            break;
        }
        name = parent;
    }
    name.parent()
        .is_some_and(|p| p.kind() == WAVEFORM_ELEMENT && p.children().count() == 1)
}

/// The strongly connected components of a graph (Tarjan's algorithm,
/// without recursion).
fn strongly_connected(edges: &[Vec<usize>]) -> Vec<Vec<usize>> {
    let n = edges.len();
    let mut index = vec![usize::MAX; n];
    let mut low = vec![0; n];
    let mut on_stack = vec![false; n];
    let mut stack = Vec::new();
    let mut components = Vec::new();
    let mut next = 0;
    for start in 0..n {
        if index[start] != usize::MAX {
            continue;
        }
        // (node, next edge to follow)
        let mut work = vec![(start, 0)];
        while let Some(&mut (v, ref mut e)) = work.last_mut() {
            if *e == 0 && index[v] == usize::MAX {
                index[v] = next;
                low[v] = next;
                next += 1;
                stack.push(v);
                on_stack[v] = true;
            }
            if let Some(&w) = edges[v].get(*e) {
                *e += 1;
                if index[w] == usize::MAX {
                    work.push((w, 0));
                } else if on_stack[w] {
                    low[v] = low[v].min(index[w]);
                }
                continue;
            }
            work.pop();
            if let Some(&(parent, _)) = work.last() {
                low[parent] = low[parent].min(low[v]);
            }
            if low[v] == index[v] {
                let mut component = Vec::new();
                while let Some(w) = stack.pop() {
                    on_stack[w] = false;
                    component.push(w);
                    if w == v {
                        break;
                    }
                }
                components.push(component);
            }
        }
    }
    components
}

/// A cycle through the smallest node of a strongly connected component:
/// its nodes, in order.
fn cycle(edges: &[Vec<usize>], component: &[usize]) -> Vec<usize> {
    let start = *component.iter().min().expect("components are not empty");
    if edges[start].contains(&start) {
        return vec![start];
    }
    // Breadth-first, inside the component, back to `start`.
    let inside: rustc_hash::FxHashSet<usize> = component.iter().copied().collect();
    let mut previous: FxHashMap<usize, usize> = FxHashMap::default();
    let mut queue = std::collections::VecDeque::from([start]);
    while let Some(v) = queue.pop_front() {
        for &w in &edges[v] {
            if w == start {
                let mut path = vec![v];
                let mut at = v;
                while at != start {
                    at = previous[&at];
                    path.push(at);
                }
                path.reverse();
                return path;
            }
            if inside.contains(&w) && !previous.contains_key(&w) {
                previous.insert(w, v);
                queue.push_back(w);
            }
        }
    }
    vec![start]
}

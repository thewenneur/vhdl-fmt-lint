//! `always` blocks: blocking and non-blocking assignments, latches,
//! sensitivity lists, `case` statements without a `default`, resets and
//! variables with more than one driver.

use rustc_hash::{FxHashMap, FxHashSet};
use vfl_sema::resets::{ResetUse, consistency, is_reset_name, named_active_low};
use vfl_sema::{Edit, Fix, codes};
use vfl_verilog_syntax::{SyntaxKind as K, SyntaxNode, SyntaxToken, TextRange, TextSize};

use super::scope::{self, Access, Kind, has_token, ident, tokens};
use super::{Unit, procedure_body, width};
use crate::Finding;

/// What a procedure models.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Model {
    /// `always_comb`, `always @*` or `always @(a or b)` (`listed`).
    Combinational { listed: bool },
    /// `always_ff`, or `always` on clock edges.
    Clocked,
    /// `always_latch`.
    Latch,
    /// `initial`, `final`, or `always` with delays.
    Other,
}

pub struct Procedure {
    pub node: SyntaxNode,
    pub keyword: SyntaxToken,
    pub model: Model,
    pub event: Option<SyntaxNode>,
    /// The signals of the event list: name, edge keyword, `EVENT_EXPR`.
    pub events: Vec<(String, Option<K>, SyntaxNode)>,
    pub body: Option<SyntaxNode>,
}

impl Procedure {
    pub fn new(node: SyntaxNode) -> Option<Procedure> {
        let keyword = tokens(&node).next()?;
        let first = node.children().next();
        let event = first
            .as_ref()
            .filter(|n| n.kind() == K::TIMED_STMT)
            .and_then(|t| t.children().find(|n| n.kind() == K::EVENT_CONTROL));
        let events: Vec<(String, Option<K>, SyntaxNode)> = event
            .iter()
            .flat_map(|e| e.children().filter(|n| n.kind() == K::EVENT_EXPR))
            .filter_map(|e| {
                let edge = tokens(&e)
                    .map(|t| t.kind())
                    .find(|k| matches!(k, K::POSEDGE_KW | K::NEGEDGE_KW | K::EDGE_KW));
                let signal = e.children().next()?;
                let mut base = signal;
                while base.kind() == K::INDEX_EXPR {
                    base = base.first_child()?;
                }
                let name = (base.kind() == K::NAME_REF && scope::is_simple_ref(&base))
                    .then(|| scope::ref_name(&base))
                    .flatten()?;
                Some((ident(&name), edge, e))
            })
            .collect();
        // `@*` or `@(*)`.
        let star = event.as_ref().is_some_and(|e| has_token(e, K::STAR));
        let model = match keyword.kind() {
            K::ALWAYS_COMB_KW => Model::Combinational { listed: false },
            K::ALWAYS_FF_KW => Model::Clocked,
            K::ALWAYS_LATCH_KW => Model::Latch,
            K::ALWAYS_KW => match &event {
                Some(_) if star => Model::Combinational { listed: false },
                // Edges and levels mixed: not a model this checks.
                Some(_) if events.iter().any(|(_, edge, _)| edge.is_some()) => {
                    if events.iter().all(|(_, edge, _)| edge.is_some()) {
                        Model::Clocked
                    } else {
                        Model::Other
                    }
                }
                Some(e)
                    if !events.is_empty()
                        && e.children().filter(|n| n.kind() == K::EVENT_EXPR).count()
                            == events.len() =>
                {
                    Model::Combinational { listed: true }
                }
                _ => Model::Other,
            },
            _ => Model::Other,
        };
        let body = procedure_body(&node);
        Some(Procedure {
            node,
            keyword,
            model,
            event,
            events,
            body,
        })
    }

    /// The names declared in the block itself: its local variables.
    pub fn locals(&self, unit: &Unit) -> FxHashSet<String> {
        unit.scope
            .decls
            .iter()
            .filter(|d| {
                self.node.text_range().contains_range(d.range)
                    && matches!(d.kind, Kind::Var | Kind::Genvar)
            })
            .map(|d| d.name.clone())
            .collect()
    }
}

/// The procedures of a unit, in order.
pub fn procedures(unit: &Unit) -> Vec<Procedure> {
    unit.node
        .descendants()
        .filter(|n| n.kind() == K::PROCEDURE)
        .filter_map(Procedure::new)
        .collect()
}

/// The statements of a block, or the statement itself.
pub fn statements(statement: &SyntaxNode) -> Vec<SyntaxNode> {
    match statement.kind() {
        K::BLOCK_STMT => statement
            .children()
            .filter(|n| !matches!(n.kind(), K::END_LABEL | K::LOCAL_DECL | K::LABEL))
            .collect(),
        _ => vec![statement.clone()],
    }
}

/// An `if` statement: its condition, its statement and the statement of
/// its `else`.
pub fn if_parts(node: &SyntaxNode) -> Option<(SyntaxNode, Option<SyntaxNode>, Option<SyntaxNode>)> {
    let mut children = node.children().filter(|n| n.kind() != K::LABEL);
    let condition = children.next()?;
    let then = children.next().filter(|n| n.kind() != K::ELSE_CLAUSE);
    let otherwise = node
        .children()
        .find(|n| n.kind() == K::ELSE_CLAUSE)
        .and_then(|e| e.children().next());
    Some((condition, then, otherwise))
}

/// A `case` item: its labels (none for `default`) and its statement.
pub fn case_item(item: &SyntaxNode) -> (Vec<SyntaxNode>, Option<SyntaxNode>) {
    let colon = tokens(item)
        .find(|t| t.kind() == K::COLON)
        .map(|t| t.text_range().start());
    let mut labels = Vec::new();
    let mut statement = None;
    for child in item.children() {
        match colon {
            Some(colon) if child.text_range().start() < colon => labels.push(child),
            _ => statement = Some(child),
        }
    }
    (labels, statement)
}

/// The expression a `case` statement selects on.
pub fn case_selector(node: &SyntaxNode) -> Option<SyntaxNode> {
    node.children()
        .find(|n| !matches!(n.kind(), K::CASE_ITEM | K::LABEL))
}

pub fn has_default(case: &SyntaxNode) -> bool {
    case.children()
        .filter(|n| n.kind() == K::CASE_ITEM)
        .any(|i| has_token(&i, K::DEFAULT_KW))
}

/// The variables an assignment statement writes.
pub fn assignment_targets(statement: &SyntaxNode) -> Vec<(SyntaxNode, bool)> {
    let mut out = Vec::new();
    if matches!(
        statement.kind(),
        K::BLOCKING_ASSIGN | K::NONBLOCKING_ASSIGN | K::COMPOUND_ASSIGN
    ) && let Some(target) = statement.first_child()
    {
        scope::targets(&target, &mut out);
    }
    out
}

/// The names `statement` writes on every path through it.
fn definitely(unit: &Unit, statement: &SyntaxNode) -> FxHashSet<String> {
    match statement.kind() {
        K::BLOCKING_ASSIGN | K::NONBLOCKING_ASSIGN | K::COMPOUND_ASSIGN => {
            assignment_targets(statement)
                .iter()
                .filter_map(|(n, _)| scope::ref_name(n))
                .map(|t| ident(&t))
                .collect()
        }
        K::BLOCK_STMT => statements(statement)
            .iter()
            .flat_map(|s| definitely(unit, s))
            .collect(),
        K::IF_STMT => match if_parts(statement) {
            // `if (Cfg.Feature)`: elaboration keeps one branch, and a
            // variable only the dropped branch assigns is not used.
            Some((condition, Some(then), otherwise)) if elaboration_constant(unit, &condition) => {
                let mut set = definitely(unit, &then);
                set.extend(otherwise.map(|o| definitely(unit, &o)).unwrap_or_default());
                set
            }
            Some((_, Some(then), Some(otherwise))) => {
                let then = definitely(unit, &then);
                let otherwise = definitely(unit, &otherwise);
                then.intersection(&otherwise).cloned().collect()
            }
            _ => FxHashSet::default(),
        },
        K::CASE_STMT => {
            if !full_case(unit, statement) {
                return FxHashSet::default();
            }
            let mut sets = statement
                .children()
                .filter(|n| n.kind() == K::CASE_ITEM)
                .map(|item| {
                    case_item(&item)
                        .1
                        .map(|s| definitely(unit, &s))
                        .unwrap_or_default()
                });
            let Some(first) = sets.next() else {
                return FxHashSet::default();
            };
            sets.fold(first, |acc, s| acc.intersection(&s).cloned().collect())
        }
        // A loop is taken to run: `for (i = 0; i < 8; i++) y[i] = ...`.
        K::FOR_STMT
        | K::FOREACH_STMT
        | K::REPEAT_STMT
        | K::WHILE_STMT
        | K::DO_WHILE_STMT
        | K::FOREVER_STMT
        | K::TIMED_STMT => statement
            .children()
            .filter(|n| is_statement(n.kind()))
            .last()
            .map(|s| definitely(unit, &s))
            .unwrap_or_default(),
        _ => FxHashSet::default(),
    }
}

/// Whether an expression depends only on parameters and package
/// constants, so that elaboration decides it.
fn elaboration_constant(unit: &Unit, expr: &SyntaxNode) -> bool {
    expr.descendants().all(|n| match n.kind() {
        K::NAME_REF => {
            !scope::is_simple_ref(&n)
                || scope::has_token(&n, K::SYSTEM_IDENT)
                || scope::ref_name(&n).is_some_and(|t| {
                    unit.decl(&ident(&t))
                        .is_some_and(|d| matches!(d.kind, Kind::Param | Kind::LocalParam))
                })
        }
        K::CALL_EXPR => n
            .first_child()
            .is_some_and(|c| scope::has_token(&c, K::SYSTEM_IDENT)),
        _ => true,
    })
}

pub fn is_statement(kind: K) -> bool {
    matches!(
        kind,
        K::BLOCK_STMT
            | K::FORK_STMT
            | K::IF_STMT
            | K::CASE_STMT
            | K::FOR_STMT
            | K::FOREACH_STMT
            | K::WHILE_STMT
            | K::DO_WHILE_STMT
            | K::REPEAT_STMT
            | K::FOREVER_STMT
            | K::BLOCKING_ASSIGN
            | K::NONBLOCKING_ASSIGN
            | K::COMPOUND_ASSIGN
            | K::EXPR_STMT
            | K::TIMED_STMT
            | K::WAIT_STMT
            | K::DISABLE_STMT
            | K::EVENT_TRIGGER
            | K::RETURN_STMT
            | K::BREAK_STMT
            | K::CONTINUE_STMT
            | K::NULL_STMT
            | K::PROCEDURAL_CONTINUOUS
            | K::IMMEDIATE_ASSERT
            | K::OPAQUE_STMT
            | K::MACRO_ITEM
    )
}

/// Whether a `case` covers every value of what it selects: it has a
/// `default`, is `unique` or `priority`, lists every member of an
/// enumeration, or every value of a few bits.
pub fn full_case(unit: &Unit, case: &SyntaxNode) -> bool {
    if has_default(case)
        || has_token(case, K::UNIQUE_KW)
        || has_token(case, K::PRIORITY_KW)
        || full_case_directive(case)
    {
        return true;
    }
    let Some(selector) = case_selector(case) else {
        return false;
    };
    let labels: Vec<SyntaxNode> = case
        .children()
        .filter(|n| n.kind() == K::CASE_ITEM)
        .flat_map(|i| case_item(&i).0)
        .collect();
    if let Some(members) = super::fsm::enum_members(unit, &selector) {
        let listed: FxHashSet<String> = labels
            .iter()
            .filter(|l| l.kind() == K::NAME_REF)
            .filter_map(scope::ref_name)
            .map(|t| ident(&t))
            .collect();
        return members.iter().all(|m| listed.contains(m));
    }
    // A selector of a width set by parameters takes the labels' size
    // when they all have the same: `3'd0` to `3'd7`.
    let sizes: FxHashSet<Option<u32>> = labels
        .iter()
        .map(|l| width::literal_size(l).and_then(|s| u32::try_from(s).ok()))
        .collect();
    let label_size = match sizes.iter().collect::<Vec<_>>().as_slice() {
        [Some(size)] => Some(*size),
        _ => None,
    };
    let Some(bits) = width::constant_width(unit, &selector)
        .or(label_size)
        .filter(|w| *w <= 12)
    else {
        return false;
    };
    let values: FxHashSet<u64> = labels.iter().filter_map(width::literal_value).collect();
    values.len() as u64 >= 1 << bits
}

/// `(* full_case *)` before a `case`, or `// synopsys full_case` after
/// its selector: synthesis takes the `case` as covering every value.
fn full_case_directive(case: &SyntaxNode) -> bool {
    let before = std::iter::successors(case.first_token().and_then(|t| t.prev_token()), |t| {
        t.prev_token()
    })
    .take_while(|t| t.kind().is_trivia());
    let after = case
        .children_with_tokens()
        .take_while(|e| e.kind() != K::CASE_ITEM)
        .filter_map(|e| e.into_token())
        .filter(|t| t.kind().is_trivia());
    before
        .chain(after)
        .any(|t| t.kind() != K::WHITESPACE && t.text().contains("full_case"))
}

pub fn check(unit: &Unit, enabled: &impl Fn(&str) -> bool, out: &mut Vec<Finding>) {
    let procedures = procedures(unit);
    let mut resets = Vec::new();
    for procedure in &procedures {
        let Some(body) = &procedure.body else {
            continue;
        };
        match procedure.model {
            Model::Combinational { listed } => {
                if enabled("verilog/nonblocking-in-combinational") {
                    assignments(procedure, body, K::NONBLOCKING_ASSIGN, out);
                }
                if enabled(codes::INFERRED_LATCH) {
                    latches(unit, procedure, body, out);
                }
                if listed {
                    sensitivity(unit, procedure, body, enabled, out);
                }
                if enabled("verilog/case-default") {
                    case_defaults(unit, body, out);
                }
            }
            Model::Clocked => {
                if enabled("verilog/blocking-in-clocked") {
                    let locals = procedure.locals(unit);
                    blocking(unit, procedure, body, &locals, out);
                }
                resets.extend(reset(unit, procedure, body, enabled, out));
            }
            Model::Latch | Model::Other => {}
        }
    }
    reset_consistency(unit, &resets, enabled, out);
    if enabled("verilog/multiple-drivers") {
        drivers(unit, &procedures, out);
    }
}

/// The assignments of `kind` under `body`, outside nested functions.
fn assignments_of(body: &SyntaxNode, kind: K) -> impl Iterator<Item = SyntaxNode> + '_ {
    body.descendants().filter(move |n| n.kind() == kind)
}

/// Non-blocking assignments in a combinational block.
fn assignments(procedure: &Procedure, body: &SyntaxNode, kind: K, out: &mut Vec<Finding>) {
    for assign in assignments_of(body, kind) {
        let Some(operator) = tokens(&assign).find(|t| t.kind() == K::LTE) else {
            continue;
        };
        out.push(
            Finding::new(
                "verilog/nonblocking-in-combinational",
                operator.text_range(),
                format!(
                    "non-blocking assignment in a combinational `{}` block; use `=`",
                    procedure.keyword.text()
                ),
            )
            .with_fix(Fix {
                title: "Use a blocking assignment".into(),
                edits: vec![Edit {
                    range: operator.text_range(),
                    text: "=".into(),
                }],
                safe: false,
            }),
        );
    }
}

/// Blocking assignments to signals in a clocked block. A variable
/// declared in the block, or one nothing outside it reads, is a
/// temporary, which may be assigned so.
fn blocking(
    unit: &Unit,
    procedure: &Procedure,
    body: &SyntaxNode,
    locals: &FxHashSet<String>,
    out: &mut Vec<Finding>,
) {
    for assign in assignments_of(body, K::BLOCKING_ASSIGN) {
        let names: Vec<String> = assignment_targets(&assign)
            .iter()
            .filter_map(|(n, _)| scope::ref_name(n))
            .map(|t| ident(&t))
            .filter(|n| !locals.contains(n))
            .filter(|n| unit.decl(n).is_some_and(|d| d.is_signal() && d.is_static()))
            .filter(|n| !temporary(unit, n, procedure.node.text_range()))
            .collect();
        let Some(name) = names.first() else {
            continue;
        };
        let Some(operator) = tokens(&assign).find(|t| t.kind() == K::EQ) else {
            continue;
        };
        out.push(
            Finding::new(
                "verilog/blocking-in-clocked",
                operator.text_range(),
                format!(
                    "blocking assignment to `{name}` in a clocked `{}` block; use `<=`",
                    procedure.keyword.text()
                ),
            )
            .with_fix(Fix {
                title: "Use a non-blocking assignment".into(),
                edits: vec![Edit {
                    range: operator.text_range(),
                    text: "<=".into(),
                }],
                safe: false,
            }),
        );
    }
}

/// The variables a combinational block assigns on some paths only.
fn latches(unit: &Unit, procedure: &Procedure, body: &SyntaxNode, out: &mut Vec<Finding>) {
    // A macro may assign anything.
    if body.descendants().any(|n| n.kind() == K::MACRO_ITEM) {
        return;
    }
    let locals = procedure.locals(unit);
    let definite = definitely(unit, body);
    let mut reported = FxHashSet::default();
    for assign in body.descendants().filter(|n| {
        matches!(
            n.kind(),
            K::BLOCKING_ASSIGN | K::NONBLOCKING_ASSIGN | K::COMPOUND_ASSIGN
        )
    }) {
        for (target, _) in assignment_targets(&assign) {
            let Some(name) = scope::ref_name(&target).map(|t| ident(&t)) else {
                continue;
            };
            if definite.contains(&name)
                || locals.contains(&name)
                || !unit
                    .decl(&name)
                    .is_some_and(|d| d.is_signal() && d.is_static())
                || temporary(unit, &name, procedure.node.text_range())
                || !reported.insert(name.clone())
            {
                continue;
            }
            out.push(Finding::new(
                codes::INFERRED_LATCH,
                target.text_range(),
                format!(
                    "`{name}` is not assigned on every path through this combinational block, which infers a latch"
                ),
            ));
        }
    }
}

/// Whether nothing outside a block reads `name`: a temporary whose old
/// value is never seen, which synthesis does not hold in a latch.
fn temporary(unit: &Unit, name: &str, block: TextRange) -> bool {
    !matches!(
        unit.decl(name).map(|d| &d.kind),
        Some(Kind::Port(_) | Kind::HeaderPort)
    ) && !unit
        .uses
        .iter()
        .any(|u| u.name == name && u.access != Access::Write && !block.contains_range(u.range))
}

/// Signals a combinational block with a sensitivity list reads but does
/// not list, and those it lists but does not read.
fn sensitivity(
    unit: &Unit,
    procedure: &Procedure,
    body: &SyntaxNode,
    enabled: &impl Fn(&str) -> bool,
    out: &mut Vec<Finding>,
) {
    let Some(event) = &procedure.event else {
        return;
    };
    let locals = procedure.locals(unit);
    let written: FxHashSet<String> = body
        .descendants()
        .flat_map(|n| assignment_targets(&n))
        .filter_map(|(n, _)| scope::ref_name(&n))
        .map(|t| ident(&t))
        .collect();
    let listed: FxHashSet<&str> = procedure.events.iter().map(|(n, ..)| n.as_str()).collect();
    let mut read: Vec<String> = Vec::new();
    for u in super::reads_in(&unit.uses, body.text_range()) {
        if read.contains(&u.name) || locals.contains(&u.name) {
            continue;
        }
        if unit
            .decl(&u.name)
            .is_some_and(|d| d.is_signal() && d.is_static() && !d.unpacked)
        {
            read.push(u.name.clone());
        }
    }
    let missing: Vec<&String> = read
        .iter()
        .filter(|n| !listed.contains(n.as_str()) && !written.contains(*n))
        .collect();
    if enabled(codes::SENSITIVITY_LIST) && !missing.is_empty() {
        let quoted: Vec<String> = missing.iter().map(|n| format!("`{n}`")).collect();
        let separator = if has_token(event, K::COMMA) {
            ", "
        } else {
            " or "
        };
        let fix = procedure.events.last().map(|(_, _, last)| {
            let end = super::trimmed_range(last).end();
            let text: String = missing.iter().map(|n| format!("{separator}{n}")).collect();
            Fix {
                title: format!("Add {} to the sensitivity list", quoted.join(", ")),
                edits: vec![Edit {
                    range: TextRange::empty(end),
                    text,
                }],
                safe: false,
            }
        });
        let mut finding = Finding::new(
            codes::SENSITIVITY_LIST,
            event.text_range(),
            format!(
                "the block reads {} but its sensitivity list does not name {}",
                quoted.join(", "),
                if missing.len() == 1 { "it" } else { "them" }
            ),
        );
        finding.fix = fix;
        out.push(finding);
    }
    if enabled(codes::UNNEEDED_SENSITIVITY) {
        let count = procedure.events.len();
        for (i, (name, _, expr)) in procedure.events.iter().enumerate() {
            if read.contains(name) {
                continue;
            }
            // A block without its list would never wait: keep the last.
            let fix = (count > 1).then(|| {
                let range = list_item_removal(&procedure.events, i);
                Fix {
                    title: format!("Remove `{name}` from the sensitivity list"),
                    edits: vec![Edit {
                        range,
                        text: String::new(),
                    }],
                    safe: false,
                }
            });
            let mut finding = Finding::new(
                codes::UNNEEDED_SENSITIVITY,
                super::trimmed_range(expr),
                format!("`{name}` is in the sensitivity list but the block never reads it"),
            );
            finding.fix = fix;
            out.push(finding);
        }
    }
}

/// The range removing item `i` of an event list removes: the item and
/// the separator after it, or before it for the last item.
fn list_item_removal(events: &[(String, Option<K>, SyntaxNode)], i: usize) -> TextRange {
    let range = |n: &SyntaxNode| super::trimmed_range(n);
    if i + 1 < events.len() {
        TextRange::new(range(&events[i].2).start(), range(&events[i + 1].2).start())
    } else {
        let end = range(&events[i].2).end();
        let start = range(&events[i - 1].2).end();
        TextRange::new(start, end)
    }
}

/// `case` statements of a combinational block that have no `default`
/// and do not list every value.
fn case_defaults(unit: &Unit, body: &SyntaxNode, out: &mut Vec<Finding>) {
    for case in body.descendants().filter(|n| n.kind() == K::CASE_STMT) {
        if full_case(unit, &case) || has_token(&case, K::UNIQUE0_KW) {
            continue;
        }
        let Some(keyword) =
            tokens(&case).find(|t| matches!(t.kind(), K::CASE_KW | K::CASEZ_KW | K::CASEX_KW))
        else {
            continue;
        };
        out.push(Finding::new(
            "verilog/case-default",
            keyword.text_range(),
            "`case` without a `default` in a combinational block",
        ));
    }
}

/// The value of a one-bit literal: `1'b0`, `0`, `'1`.
fn bit_literal(node: &SyntaxNode) -> Option<bool> {
    if node.kind() != K::LITERAL {
        return None;
    }
    let text = node.text().to_string();
    let digits = text
        .rsplit(['b', 'B', 'h', 'H', 'd', 'D', '\''])
        .next()?
        .trim();
    match digits {
        "0" => Some(false),
        "1" => Some(true),
        _ => None,
    }
}

/// A test of one signal: `rst`, `!rst_n`, `~rst_n`, `rst == 1'b1`,
/// `rst_n == 0`. The signal's name, its range and whether the test is
/// true when it is high (`None` when compared with a parameter).
fn signal_test(node: &SyntaxNode) -> Option<(String, TextRange, Option<bool>)> {
    match node.kind() {
        K::NAME_REF if scope::is_simple_ref(node) => {
            let token = scope::ref_name(node)?;
            Some((ident(&token), token.text_range(), Some(true)))
        }
        K::PAREN_EXPR => signal_test(&node.first_child()?),
        K::UNARY_EXPR if has_token(node, K::BANG) || has_token(node, K::TILDE) => {
            let (name, range, high) = signal_test(&node.first_child()?)?;
            Some((name, range, high.map(|h| !h)))
        }
        K::BINARY_EXPR => {
            let operator = tokens(node).next()?.kind();
            let equal = match operator {
                K::EQ2 | K::EQ3 => true,
                K::NEQ | K::NEQ2 => false,
                _ => return None,
            };
            let mut operands = node.children();
            let (a, b) = (operands.next()?, operands.next()?);
            let (signal, value) = if a.kind() == K::NAME_REF && b.kind() != K::NAME_REF {
                (a, b)
            } else if b.kind() == K::NAME_REF && a.kind() != K::NAME_REF {
                (b, a)
            } else if a.kind() == K::NAME_REF {
                // `rst == RESET_LEVEL`.
                let token = scope::ref_name(&a)?;
                return Some((ident(&token), token.text_range(), None));
            } else {
                return None;
            };
            let token = scope::ref_name(&signal).filter(|_| scope::is_simple_ref(&signal))?;
            let level = bit_literal(&value)?;
            Some((ident(&token), token.text_range(), Some(level == equal)))
        }
        _ => None,
    }
}

/// The reset of a clocked block, if it has one, after checking what it
/// resets.
fn reset(
    unit: &Unit,
    procedure: &Procedure,
    body: &SyntaxNode,
    enabled: &impl Fn(&str) -> bool,
    out: &mut Vec<Finding>,
) -> Option<ResetUse<String>> {
    let statements = statements(body);
    let [statement] = statements.as_slice() else {
        return None;
    };
    if statement.kind() != K::IF_STMT {
        return None;
    }
    let (condition, then, otherwise) = if_parts(statement)?;
    let (name, range, active_high) = signal_test(&condition)?;
    let edge = procedure
        .events
        .iter()
        .find(|(n, ..)| *n == name)
        .map(|(_, edge, _)| *edge);
    let sync = match edge {
        Some(_) if procedure.events.len() > 1 => false,
        Some(_) => return None,
        // A reset sets registers to constants; a branch doing other work
        // is an enable (`if (resetn) begin ... end`).
        None if is_reset_name(&name) && then.as_ref().is_some_and(|t| sets_constants(unit, t)) => {
            true
        }
        None => return None,
    };
    let in_generate = procedure
        .node
        .ancestors()
        .any(|n| matches!(n.kind(), K::GENERATE_IF | K::GENERATE_CASE));
    if enabled(codes::RESET_POLARITY) {
        if let (Some(Some(edge)), Some(high)) = (edge, active_high)
            && matches!(edge, K::POSEDGE_KW | K::NEGEDGE_KW)
            && (edge == K::POSEDGE_KW) != high
        {
            out.push(Finding::new(
                codes::RESET_POLARITY,
                range,
                format!(
                    "`{name}` is listed with `{}` but resets when {} here",
                    if edge == K::POSEDGE_KW {
                        "posedge"
                    } else {
                        "negedge"
                    },
                    if high { "high" } else { "low" }
                ),
            ));
        } else if let Some(message) = named_active_low(&name, active_high, true) {
            out.push(Finding::new(codes::RESET_POLARITY, range, message));
        }
    }
    if enabled(codes::MISSING_RESET) {
        let locals = procedure.locals(unit);
        let reset_targets: FxHashSet<String> = then
            .iter()
            .flat_map(|s| s.descendants())
            .flat_map(|n| assignment_targets(&n))
            .filter_map(|(n, _)| scope::ref_name(&n))
            .map(|t| ident(&t))
            .collect();
        let mut reported = FxHashSet::default();
        for assign in otherwise.iter().flat_map(|s| s.descendants()) {
            for (target, _) in assignment_targets(&assign) {
                let Some(target_name) = scope::ref_name(&target).map(|t| ident(&t)) else {
                    continue;
                };
                let memory = unit
                    .decl(&target_name)
                    .is_none_or(|d| d.unpacked || !d.is_signal());
                if reset_targets.contains(&target_name)
                    || locals.contains(&target_name)
                    || memory
                    || !reported.insert(target_name.clone())
                {
                    continue;
                }
                out.push(Finding::new(
                    codes::MISSING_RESET,
                    target.text_range(),
                    format!(
                        "`{target_name}` is not reset by `{name}` in this block, so the reset acts as an enable on it"
                    ),
                ));
            }
        }
    }
    Some(ResetUse {
        key: name.clone(),
        name,
        sync,
        active_high,
        range,
        in_generate,
    })
}

/// Whether every assignment under `statement` assigns a constant, and
/// there is one.
fn sets_constants(unit: &Unit, statement: &SyntaxNode) -> bool {
    fn constant(unit: &Unit, expr: &SyntaxNode) -> bool {
        match expr.kind() {
            K::LITERAL => true,
            K::CONCAT_EXPR | K::REPLICATION_EXPR | K::PAREN_EXPR | K::UNARY_EXPR => {
                expr.children().all(|c| constant(unit, &c))
            }
            K::ASSIGNMENT_PATTERN | K::PATTERN_ITEM => expr
                .children()
                .all(|c| c.kind() == K::NAME_REF || constant(unit, &c)),
            K::NAME_REF => scope::ref_name(expr).is_some_and(|t| {
                !scope::is_simple_ref(expr)
                    || unit.decl(&ident(&t)).is_none_or(|d| {
                        matches!(d.kind, Kind::Param | Kind::LocalParam | Kind::EnumMember)
                    })
            }),
            _ => false,
        }
    }
    let assignments: Vec<SyntaxNode> = statement
        .descendants()
        .filter(|n| matches!(n.kind(), K::BLOCKING_ASSIGN | K::NONBLOCKING_ASSIGN))
        .collect();
    !assignments.is_empty()
        && assignments
            .iter()
            .all(|a| a.children().nth(1).is_some_and(|v| constant(unit, &v)))
}

/// Reset signals used both synchronously and asynchronously, or tested
/// with both polarities, within the unit.
fn reset_consistency(
    unit: &Unit,
    uses: &[ResetUse<String>],
    enabled: &impl Fn(&str) -> bool,
    out: &mut Vec<Finding>,
) {
    let line = |at| unit.ctx.line(at);
    let mixed = enabled(codes::MIXED_RESET);
    let polarity = enabled(codes::RESET_POLARITY);
    for i in consistency(uses, "block", line, mixed, polarity) {
        out.push(Finding::new(i.code, i.range, i.message));
    }
}

/// Variables assigned whole by more than one block, or by a block and a
/// continuous assignment.
fn drivers(unit: &Unit, procedures: &[Procedure], out: &mut Vec<Finding>) {
    // Per variable: each driver's node and the first assignment in it.
    let mut drivers: FxHashMap<String, Vec<(SyntaxNode, TextRange)>> = FxHashMap::default();
    let mut add = |name: String, driver: &SyntaxNode, range: TextRange| {
        let list = drivers.entry(name).or_default();
        if !list.iter().any(|(d, _)| d == driver) {
            list.push((driver.clone(), range));
        }
    };
    for procedure in procedures {
        if matches!(procedure.model, Model::Other) {
            continue;
        }
        let Some(body) = &procedure.body else {
            continue;
        };
        for assign in body.descendants() {
            for (target, whole) in assignment_targets(&assign) {
                if let (true, Some(name)) = (whole, scope::ref_name(&target)) {
                    add(ident(&name), &procedure.node, target.text_range());
                }
            }
        }
    }
    for assign in unit
        .node
        .descendants()
        .filter(|n| n.kind() == K::CONTINUOUS_ASSIGN)
    {
        for expr in assign.children().filter(|n| n.kind() == K::ASSIGN_EXPR) {
            let mut targets = Vec::new();
            if let Some(target) = expr.first_child() {
                scope::targets(&target, &mut targets);
            }
            for (target, whole) in targets {
                if let (true, Some(name)) = (whole, scope::ref_name(&target)) {
                    add(ident(&name), &assign, target.text_range());
                }
            }
        }
    }
    let mut found: Vec<(TextRange, String, TextSize)> = Vec::new();
    for (name, list) in drivers {
        if list.len() < 2 {
            continue;
        }
        let Some(decl) = unit.decl(&name) else {
            continue;
        };
        let variable = unit.scope.named(&name).any(|d| d.is_variable());
        if !variable || !decl.is_static() || decl.region != unit.node {
            continue;
        }
        // Alternatives of an `if` or `case` generate never coexist.
        if list.iter().any(|(d, _)| {
            d.ancestors()
                .any(|a| matches!(a.kind(), K::GENERATE_IF | K::GENERATE_CASE))
        }) {
            continue;
        }
        let mut list = list;
        list.sort_by_key(|(_, r)| r.start());
        for (_, range) in &list[1..] {
            found.push((*range, name.clone(), list[0].1.start()));
        }
    }
    found.sort_by_key(|(r, ..)| r.start());
    for (range, name, first) in found {
        out.push(Finding::new(
            "verilog/multiple-drivers",
            range,
            format!(
                "`{name}` is also assigned by the block at line {}; a variable must have one driver",
                unit.ctx.line(first)
            ),
        ));
    }
}

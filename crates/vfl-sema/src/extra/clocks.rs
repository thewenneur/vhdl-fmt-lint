//! Clocks, resets and registers: clocks used as data or made by logic,
//! mixed edges, resets from logic or from another clock domain, resets of
//! constants, buses synchronized bit by bit, unregistered outputs,
//! internal tri-states, metavalues in RTL, state machines without a safe
//! encoding.

use rustc_hash::{FxHashMap, FxHashSet};
use vfl_syntax::green_walk::ChildTokens;
use vfl_syntax::{SyntaxKind, SyntaxNode, TextRange};

use super::{Unit, extra_codes as codes, statements, trimmed};
use crate::decl::{DeclId, DeclKind, Mode, TypeDef};
use crate::diagnostic::Diagnostic;
use crate::resolver::is_clock_edge;
use crate::rtl::reset::if_branches;

/// A clock edge in a process.
struct Edge {
    clock: DeclId,
    rising: Option<bool>,
    range: TextRange,
}

/// A clocked process: its edges, the signals it registers, its
/// asynchronous resets.
struct Clocked {
    node: SyntaxNode,
    clock: DeclId,
    registers: FxHashSet<DeclId>,
    /// Asynchronous resets, with the range of their test.
    async_resets: Vec<(DeclId, TextRange)>,
    /// The statements that run on a reset, and the ones on the edge.
    reset_part: Vec<SyntaxNode>,
    clock_part: Vec<SyntaxNode>,
}

pub(super) fn check(u: &Unit, out: &mut Vec<Diagnostic>) {
    let ids = [
        codes::CLOCK_AS_DATA,
        codes::MIXED_CLOCK_EDGES,
        codes::GATED_CLOCK,
        codes::RESET_SOURCE,
        codes::SUPERFLUOUS_RESET,
        codes::MULTI_BIT_SYNCHRONIZER,
        codes::UNREGISTERED_OUTPUT,
        codes::INTERNAL_TRISTATE,
        codes::METAVALUE_ASSIGNMENT,
        codes::UNSAFE_FSM,
        codes::STATE_TYPE,
    ];
    if !ids.iter().any(|id| u.on(id)) {
        return;
    }
    if u.analysis.units()[u.unit as usize].kind != DeclKind::Architecture {
        return;
    }
    let testbench = is_testbench(u);
    let mut edges = Vec::new();
    let mut clocked = Vec::new();
    let mut processes = Vec::new();
    for process in u
        .node
        .descendants()
        .filter(|n| n.kind() == SyntaxKind::PROCESS_STMT)
    {
        let mut mine = Vec::new();
        for n in process.descendants() {
            if is_clock_edge(&n)
                && let Some(edge) = edge_of(u, &n)
            {
                mine.push(edge);
            }
        }
        if let Some(first) = mine.first()
            && let Some(c) = clocked_process(u, &process, first.clock)
        {
            clocked.push(c);
        }
        edges.extend(mine);
        processes.push(process);
    }
    let clocks: FxHashSet<DeclId> = edges.iter().map(|e| e.clock).collect();
    if u.on(codes::CLOCK_AS_DATA) && !testbench {
        clock_as_data(u, &clocks, out);
    }
    if u.on(codes::MIXED_CLOCK_EDGES) {
        mixed_edges(u, &edges, out);
    }
    let concurrent = concurrent_assignments(u);
    if u.on(codes::GATED_CLOCK) && !testbench {
        gated_clocks(u, &clocks, &concurrent, out);
    }
    let register_clock: FxHashMap<DeclId, DeclId> = clocked
        .iter()
        .flat_map(|c| c.registers.iter().map(move |r| (*r, c.clock)))
        .collect();
    if u.on(codes::RESET_SOURCE) && !testbench {
        reset_sources(u, &clocked, &concurrent, &register_clock, out);
    }
    if u.on(codes::SUPERFLUOUS_RESET) {
        superfluous_resets(u, &clocked, out);
    }
    if u.on(codes::MULTI_BIT_SYNCHRONIZER) && !testbench {
        multi_bit(u, &clocked, &register_clock, out);
    }
    if u.on(codes::UNREGISTERED_OUTPUT) && !testbench {
        unregistered_outputs(u, &clocked, &concurrent, &processes, out);
    }
    if (u.on(codes::INTERNAL_TRISTATE) || u.on(codes::METAVALUE_ASSIGNMENT)) && !testbench {
        assigned_values(u, out);
    }
    if (u.on(codes::UNSAFE_FSM) || u.on(codes::STATE_TYPE)) && !testbench {
        state_machines(u, &clocked, out);
    }
}

/// Whether the unit is a testbench's architecture: its entity has no
/// ports.
fn is_testbench(u: &Unit) -> bool {
    let info = &u.analysis.units()[u.unit as usize];
    let Some(entity) = info.primary.as_deref() else {
        return false;
    };
    let Some(index) =
        u.analysis.units().iter().position(|i| {
            i.kind == DeclKind::Entity && i.name == entity && i.library == info.library
        })
    else {
        return false;
    };
    !u.analysis
        .unit_decls(index as u32)
        .iter()
        .any(|d| matches!(d.kind, DeclKind::Port { .. }))
}

fn edge_of(u: &Unit, edge: &SyntaxNode) -> Option<Edge> {
    let (name, rising) = match edge.kind() {
        SyntaxKind::CALL_EXPR => {
            let function = edge
                .children()
                .next()?
                .text()
                .to_string()
                .to_ascii_lowercase();
            let name = edge
                .children()
                .find(|n| n.kind() == SyntaxKind::ASSOCIATION_LIST)?
                .descendants()
                .find(|n| matches!(n.kind(), SyntaxKind::NAME | SyntaxKind::SELECTED_NAME))?;
            (name, Some(function.trim().ends_with("rising_edge")))
        }
        _ => {
            // `clk'event and clk = '1'`.
            let name = edge.children().next()?;
            let rising = edge.parent().and_then(|p| {
                (p.kind() == SyntaxKind::BINARY_EXPR).then(|| {
                    p.descendants_with_tokens()
                        .filter_map(|e| e.into_token())
                        .find(|t| t.kind() == SyntaxKind::CHAR_LITERAL)
                        .map(|t| t.text() == "'1'")
                })?
            });
            (name, rising)
        }
    };
    let (id, decl) = u.named(&name)?;
    decl.kind.is_signal().then_some(Edge {
        clock: id,
        rising,
        range: trimmed(edge),
    })
}

/// The signals an assignment target writes, and the target node.
fn written(u: &Unit, statement: &SyntaxNode) -> Option<DeclId> {
    let (target, _) = crate::rtl::assignment_target(statement)?;
    let (id, decl) = u.target_object(&target)?;
    decl.kind.is_signal().then_some(id)
}

fn clocked_process(u: &Unit, process: &SyntaxNode, clock: DeclId) -> Option<Clocked> {
    let body = statements(process);
    let mut c = Clocked {
        node: process.clone(),
        clock,
        registers: FxHashSet::default(),
        async_resets: Vec::new(),
        reset_part: Vec::new(),
        clock_part: Vec::new(),
    };
    // `if rst then ... elsif rising_edge(clk) then ... end if;`
    let first_if = body.iter().find(|n| n.kind() == SyntaxKind::IF_STMT);
    let mut clock_part = Vec::new();
    if let Some(stmt) = first_if {
        let branches = if_branches(stmt);
        let has_edge = |c: &Option<SyntaxNode>| {
            c.as_ref()
                .is_some_and(|c| c.descendants().any(|n| is_clock_edge(&n)) || is_clock_edge(c))
        };
        if branches.len() >= 2
            && !has_edge(&branches[0].0)
            && branches.iter().skip(1).any(|b| has_edge(&b.0))
        {
            if let Some(cond) = &branches[0].0 {
                for name in cond
                    .descendants()
                    .filter(|n| matches!(n.kind(), SyntaxKind::NAME | SyntaxKind::SELECTED_NAME))
                {
                    if let Some((id, d)) = u.named(&name)
                        && d.kind.is_signal()
                    {
                        c.async_resets.push((id, trimmed(cond)));
                    }
                }
            }
            c.reset_part = branches[0].1.clone();
            for b in branches.iter().skip(1) {
                if has_edge(&b.0) {
                    clock_part.extend(b.1.iter().cloned());
                }
            }
        } else if branches.len() == 1 && has_edge(&branches[0].0) {
            clock_part = branches[0].1.clone();
            // `if rst then ... else ... end if;` inside: a synchronous reset.
            if let Some(inner) = clock_part
                .first()
                .filter(|n| n.kind() == SyntaxKind::IF_STMT)
            {
                let inner_branches = if_branches(inner);
                let reset = inner_branches
                    .first()
                    .and_then(|b| b.0.as_ref())
                    .is_some_and(|cond| {
                        cond.descendants()
                            .filter(|n| n.kind() == SyntaxKind::NAME)
                            .filter_map(|n| u.named(&n))
                            .any(|(_, d)| {
                                d.kind.is_signal() && crate::resets::is_reset_name(&d.name)
                            })
                    });
                if reset && inner_branches.len() >= 2 && clock_part.len() == 1 {
                    c.reset_part = inner_branches[0].1.clone();
                    clock_part = inner_branches
                        .iter()
                        .skip(1)
                        .flat_map(|b| b.1.iter().cloned())
                        .collect();
                }
            }
        }
    }
    if clock_part.is_empty() && c.reset_part.is_empty() {
        // `wait until rising_edge(clk)` and other forms: every statement.
        clock_part = body.clone();
    }
    for part in clock_part.iter().chain(&c.reset_part) {
        for n in std::iter::once(part.clone()).chain(part.descendants()) {
            if matches!(n.kind(), SyntaxKind::SIGNAL_ASSIGN_STMT)
                && let Some(id) = written(u, &n)
            {
                c.registers.insert(id);
            }
        }
    }
    c.clock_part = clock_part;
    Some(c)
}

/// Whether `range` is inside a node of one of `kinds`, below `stop`.
fn inside(u: &Unit, range: TextRange, ok: impl Fn(&SyntaxNode) -> bool) -> bool {
    let element = u.node.covering_element(range);
    let start = match element {
        rowan::NodeOrToken::Node(n) => n,
        rowan::NodeOrToken::Token(t) => match t.parent() {
            Some(p) => p,
            None => return false,
        },
    };
    start.ancestors().any(|n| ok(&n))
}

fn clock_as_data(u: &Unit, clocks: &FxHashSet<DeclId>, out: &mut Vec<Diagnostic>) {
    let mut reported = FxHashSet::default();
    for r in u.refs {
        if !r.access.reads() {
            continue;
        }
        let Some(id) = super::single_decl(r) else {
            continue;
        };
        if !clocks.contains(&id) {
            continue;
        }
        let allowed = inside(u, r.range, |n| match n.kind() {
            SyntaxKind::SENSITIVITY_LIST
            | SyntaxKind::WAIT_STMT
            | SyntaxKind::ASSOCIATION_ELEMENT
                if n.ancestors().any(|a| {
                    matches!(
                        a.kind(),
                        SyntaxKind::PORT_MAP_ASPECT
                            | SyntaxKind::SENSITIVITY_LIST
                            | SyntaxKind::WAIT_STMT
                    )
                }) =>
            {
                true
            }
            SyntaxKind::SENSITIVITY_LIST
            | SyntaxKind::WAIT_STMT
            | SyntaxKind::PROCEDURE_CALL_STMT => true,
            SyntaxKind::CALL_EXPR | SyntaxKind::ATTRIBUTE_NAME => is_clock_edge(n),
            // `clk'event and clk = '1'`.
            SyntaxKind::BINARY_EXPR => n.children().any(|c| is_clock_edge(&c)),
            // A copy (`clk_out <= clk;`), a clock generator, a delay.
            SyntaxKind::SIGNAL_ASSIGN_STMT => {
                n.descendants_with_tokens()
                    .any(|e| e.kind() == SyntaxKind::AFTER_KW)
                    || n.children()
                        .find(|c| c.kind() == SyntaxKind::WAVEFORM)
                        .is_some_and(|w| {
                            w.children().count() == 1
                                && w.first_child()
                                    .and_then(|e| e.first_child())
                                    .is_some_and(|v| v.kind() == SyntaxKind::NAME)
                        })
            }
            _ => false,
        });
        if !allowed && reported.insert(r.range) {
            let name = u.decl(id).map_or_else(String::new, |d| d.spelling.clone());
            u.push(
                out,
                codes::CLOCK_AS_DATA,
                r.range,
                format!("clock `{name}` is used as data: its value feeds logic instead of clocking registers"),
            );
        }
    }
}

fn mixed_edges(u: &Unit, edges: &[Edge], out: &mut Vec<Diagnostic>) {
    let mut by_clock: FxHashMap<DeclId, (Vec<&Edge>, Vec<&Edge>)> = FxHashMap::default();
    for e in edges {
        match e.rising {
            Some(true) => by_clock.entry(e.clock).or_default().0.push(e),
            Some(false) => by_clock.entry(e.clock).or_default().1.push(e),
            None => {}
        }
    }
    let mut clocks: Vec<_> = by_clock.into_iter().collect();
    clocks.sort_by_key(|(id, _)| *id);
    for (clock, (rising, falling)) in clocks {
        if rising.is_empty() || falling.is_empty() {
            continue;
        }
        let (fewer, other, edge, other_edge) = if falling.len() <= rising.len() {
            (falling, rising, "falling", "rising")
        } else {
            (rising, falling, "rising", "falling")
        };
        let name = u
            .decl(clock)
            .map_or_else(String::new, |d| d.spelling.clone());
        for e in fewer {
            u.push(
                out,
                codes::MIXED_CLOCK_EDGES,
                e.range,
                format!(
                    "`{name}` is used on its {edge} edge here and on its {other_edge} edge at line {}",
                    u.line(other[0].range)
                ),
            );
        }
    }
}

/// Concurrent signal assignments of the architecture, by the signal they
/// drive.
fn concurrent_assignments(u: &Unit) -> FxHashMap<DeclId, SyntaxNode> {
    let mut map = FxHashMap::default();
    for n in u.node.descendants() {
        if !matches!(
            n.kind(),
            SyntaxKind::SIGNAL_ASSIGN_STMT | SyntaxKind::SELECTED_ASSIGN_STMT
        ) {
            continue;
        }
        if n.ancestors().any(|a| {
            matches!(
                a.kind(),
                SyntaxKind::PROCESS_STMT | SyntaxKind::SUBPROGRAM_BODY
            )
        }) {
            continue;
        }
        if let Some(id) = written(u, &n) {
            map.entry(id).or_insert(n);
        }
    }
    map
}

/// Whether an assignment computes its value with logic: a logical
/// operator, a condition or a selection.
fn is_logic(statement: &SyntaxNode) -> bool {
    statement.kind() == SyntaxKind::SELECTED_ASSIGN_STMT
        || statement.has_child(|k| k == SyntaxKind::CONDITIONAL_WAVEFORMS)
        || statement.descendants().any(|n| {
            n.kind() == SyntaxKind::BINARY_EXPR
                && n.child_token(|k| !k.is_trivia()).is_some_and(|t| {
                    matches!(
                        t.kind(),
                        SyntaxKind::AND_KW
                            | SyntaxKind::OR_KW
                            | SyntaxKind::NAND_KW
                            | SyntaxKind::NOR_KW
                            | SyntaxKind::XOR_KW
                            | SyntaxKind::XNOR_KW
                    )
                })
        })
}

fn gated_clocks(
    u: &Unit,
    clocks: &FxHashSet<DeclId>,
    concurrent: &FxHashMap<DeclId, SyntaxNode>,
    out: &mut Vec<Diagnostic>,
) {
    let mut ids: Vec<_> = clocks.iter().copied().collect();
    ids.sort();
    for clock in ids {
        let Some(statement) = concurrent.get(&clock) else {
            continue;
        };
        if statement
            .descendants_with_tokens()
            .any(|e| e.kind() == SyntaxKind::AFTER_KW)
        {
            continue;
        }
        if is_logic(statement) {
            let name = u
                .decl(clock)
                .map_or_else(String::new, |d| d.spelling.clone());
            u.push(
                out,
                codes::GATED_CLOCK,
                trimmed(statement),
                format!(
                    "clock `{name}` is made by logic: a gated clock can glitch and skips the clock network; use a clock enable or a clock buffer"
                ),
            );
        }
    }
}

fn reset_sources(
    u: &Unit,
    clocked: &[Clocked],
    concurrent: &FxHashMap<DeclId, SyntaxNode>,
    register_clock: &FxHashMap<DeclId, DeclId>,
    out: &mut Vec<Diagnostic>,
) {
    let mut reported = FxHashSet::default();
    for c in clocked {
        for &(reset, range) in &c.async_resets {
            if !reported.insert((reset, c.clock)) {
                continue;
            }
            let name = u
                .decl(reset)
                .map_or_else(String::new, |d| d.spelling.clone());
            if let Some(statement) = concurrent.get(&reset)
                && is_logic(statement)
            {
                u.push(
                    out,
                    codes::RESET_SOURCE,
                    range,
                    format!(
                        "asynchronous reset `{name}` comes from logic (line {}): a glitch resets the registers",
                        u.line(statement.text_range())
                    ),
                );
            } else if let Some(&other) = register_clock.get(&reset)
                && other != c.clock
            {
                let clock = |id| u.decl(id).map_or_else(String::new, |d| d.spelling.clone());
                u.push(
                    out,
                    codes::RESET_SOURCE,
                    range,
                    format!(
                        "asynchronous reset `{name}` is registered on `{}` but resets registers clocked by `{}`: its release needs a reset synchronizer in that domain",
                        clock(other),
                        clock(c.clock)
                    ),
                );
            }
        }
    }
}

fn written_in(u: &Unit, part: &[SyntaxNode]) -> Vec<(DeclId, TextRange)> {
    let mut out = Vec::new();
    for p in part {
        for n in std::iter::once(p.clone()).chain(p.descendants()) {
            if n.kind() == SyntaxKind::SIGNAL_ASSIGN_STMT
                && let Some(id) = written(u, &n)
            {
                out.push((id, trimmed(&n)));
            }
        }
    }
    out
}

fn superfluous_resets(u: &Unit, clocked: &[Clocked], out: &mut Vec<Diagnostic>) {
    for c in clocked {
        if c.reset_part.is_empty() {
            continue;
        }
        let clocked_writes: FxHashSet<DeclId> = written_in(u, &c.clock_part)
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        let mut seen = FxHashSet::default();
        for (id, range) in written_in(u, &c.reset_part) {
            if clocked_writes.contains(&id) || !seen.insert(id) {
                continue;
            }
            // Written elsewhere (another process, a concurrent statement):
            // a multiple-driver problem, not this one.
            let elsewhere = u.refs.iter().any(|r| {
                r.access.writes()
                    && super::single_decl(r) == Some(id)
                    && !c.node.text_range().contains_range(r.range)
            });
            if elsewhere {
                continue;
            }
            let name = u.decl(id).map_or_else(String::new, |d| d.spelling.clone());
            u.push(
                out,
                codes::SUPERFLUOUS_RESET,
                range,
                format!("`{name}` is reset but never assigned on the clock edge: it is a constant, and needs no register or reset"),
            );
        }
    }
}

fn multi_bit(
    u: &Unit,
    clocked: &[Clocked],
    register_clock: &FxHashMap<DeclId, DeclId>,
    out: &mut Vec<Diagnostic>,
) {
    for c in clocked {
        for part in &c.clock_part {
            for n in std::iter::once(part.clone()).chain(part.descendants()) {
                if n.kind() != SyntaxKind::SIGNAL_ASSIGN_STMT {
                    continue;
                }
                // A plain copy `x <= y;` of a register of another domain.
                let Some(waveform) = n.children().find(|w| w.kind() == SyntaxKind::WAVEFORM) else {
                    continue;
                };
                if waveform.children().count() != 1 {
                    continue;
                }
                let Some(value) = waveform.first_child().and_then(|e| e.first_child()) else {
                    continue;
                };
                if value.kind() != SyntaxKind::NAME {
                    continue;
                }
                let Some((source, decl)) = u.named(&value) else {
                    continue;
                };
                let Some(&from) = register_clock.get(&source) else {
                    continue;
                };
                if from == c.clock || decl.name.contains("gray") {
                    continue;
                }
                let wide = decl.width.is_some_and(|w| w > 1)
                    || decl.width.is_none() && is_array(u, source);
                if !wide {
                    continue;
                }
                let clock = |id| u.decl(id).map_or_else(String::new, |d| d.spelling.clone());
                u.push(
                    out,
                    codes::MULTI_BIT_SYNCHRONIZER,
                    trimmed(&n),
                    format!(
                        "the bits of `{}` cross from `{}` to `{}` through one register each: they can arrive in different cycles; use a handshake, a FIFO or a Gray code",
                        decl.spelling,
                        clock(from),
                        clock(c.clock)
                    ),
                );
            }
        }
    }
}

fn is_array(u: &Unit, id: DeclId) -> bool {
    u.decl(id)
        .and_then(|d| u.analysis.base_type(d.subtype?))
        .and_then(|b| u.decl(b))
        .and_then(|b| b.type_def.as_deref())
        .is_some_and(|t| matches!(t, TypeDef::Array { .. } | TypeDef::Record(_)))
}

fn unregistered_outputs(
    u: &Unit,
    clocked: &[Clocked],
    concurrent: &FxHashMap<DeclId, SyntaxNode>,
    processes: &[SyntaxNode],
    out: &mut Vec<Diagnostic>,
) {
    let registers: FxHashSet<DeclId> = clocked
        .iter()
        .flat_map(|c| c.registers.iter().copied())
        .collect();
    let clocked_nodes: FxHashSet<TextRange> = clocked.iter().map(|c| c.node.text_range()).collect();
    // Output ports driven by a combinational process or by logic of
    // something else than registers.
    let mut drivers: Vec<(DeclId, SyntaxNode)> =
        concurrent.iter().map(|(k, v)| (*k, v.clone())).collect();
    for p in processes {
        if clocked_nodes.contains(&p.text_range()) {
            continue;
        }
        for n in p
            .descendants()
            .filter(|n| n.kind() == SyntaxKind::SIGNAL_ASSIGN_STMT)
        {
            if let Some(id) = written(u, &n) {
                drivers.push((id, n));
            }
        }
    }
    drivers.sort_by_key(|(_, n)| n.text_range().start());
    let mut seen = FxHashSet::default();
    for (id, statement) in drivers {
        let Some(decl) = u.decl(id) else { continue };
        if !matches!(
            decl.kind,
            DeclKind::Port {
                mode: Mode::Out | Mode::Buffer,
                ..
            }
        ) || registers.contains(&id)
        {
            continue;
        }
        // From registers only (`q <= q_reg;`) counts as registered.
        let sources: Vec<DeclId> = u
            .refs_in(statement.text_range())
            .iter()
            .filter(|r| r.access.reads())
            .filter_map(super::single_decl)
            .filter(|s| u.decl(*s).is_some_and(|d| d.kind.is_signal()))
            .collect();
        if sources.is_empty() || sources.iter().all(|s| registers.contains(s)) {
            continue;
        }
        if seen.insert(id) {
            u.push(
                out,
                codes::UNREGISTERED_OUTPUT,
                trimmed(&statement),
                format!(
                    "output `{}` is driven by combinational logic, not by a register",
                    decl.spelling
                ),
            );
        }
    }
}

/// The literal values of an assignment that are all metavalues: `'X'`,
/// `"ZZZZ"`, `(others => '-')`.
fn metavalue(value: &SyntaxNode) -> Option<char> {
    let token = match value.kind() {
        SyntaxKind::LITERAL => value.first_token()?,
        SyntaxKind::AGGREGATE => {
            let element = value
                .children()
                .find(|n| n.kind() == SyntaxKind::ELEMENT_ASSOCIATION)?;
            if value
                .children()
                .filter(|n| n.kind() == SyntaxKind::ELEMENT_ASSOCIATION)
                .count()
                != 1
                || !element
                    .descendants_with_tokens()
                    .any(|e| e.kind() == SyntaxKind::OTHERS_KW)
            {
                return None;
            }
            let inner = element.children().last()?;
            return metavalue(&inner);
        }
        _ => return None,
    };
    let text = token.text();
    let inner = match token.kind() {
        SyntaxKind::CHAR_LITERAL => text.trim_matches('\''),
        SyntaxKind::STRING_LITERAL => text.trim_matches('"'),
        _ => return None,
    };
    let mut chars = inner.chars();
    let first = chars.next()?.to_ascii_uppercase();
    (matches!(first, 'X' | 'U' | 'W' | '-' | 'Z')
        && inner.chars().all(|c| c.to_ascii_uppercase() == first))
    .then_some(first)
}

fn assigned_values(u: &Unit, out: &mut Vec<Diagnostic>) {
    for n in u.node.descendants() {
        if !matches!(
            n.kind(),
            SyntaxKind::SIGNAL_ASSIGN_STMT | SyntaxKind::SELECTED_ASSIGN_STMT
        ) {
            continue;
        }
        // Simulation code: delays and waits.
        if n.descendants_with_tokens()
            .any(|e| e.kind() == SyntaxKind::AFTER_KW)
            || n.ancestors()
                .find(|a| a.kind() == SyntaxKind::PROCESS_STMT)
                .is_some_and(|p| {
                    p.descendants().any(|d| {
                        d.kind() == SyntaxKind::WAIT_STMT
                            && d.has_child(|k| k == SyntaxKind::FOR_KW)
                    })
                })
        {
            continue;
        }
        let Some((target, _)) = crate::rtl::assignment_target(&n) else {
            continue;
        };
        let Some((_, decl)) = u.target_object(&target) else {
            continue;
        };
        let mut values = Vec::new();
        for child in n.children() {
            crate::rtl::assigned_values(&child, &mut values);
        }
        for value in values {
            let Some(c) = metavalue(&value) else { continue };
            if c == 'Z' {
                if decl.kind == DeclKind::Signal && u.on(codes::INTERNAL_TRISTATE) {
                    u.push(
                        out,
                        codes::INTERNAL_TRISTATE,
                        trimmed(&value),
                        format!(
                            "internal tri-state on `{}`: FPGAs have none inside; synthesis makes multiplexers or fails",
                            decl.spelling
                        ),
                    );
                }
            } else if u.on(codes::METAVALUE_ASSIGNMENT) {
                u.push(
                    out,
                    codes::METAVALUE_ASSIGNMENT,
                    trimmed(&value),
                    format!(
                        "`{}` gets the metavalue '{c}': simulation propagates it while synthesis picks any value",
                        decl.spelling
                    ),
                );
            }
        }
    }
}

fn state_machines(u: &Unit, clocked: &[Clocked], out: &mut Vec<Diagnostic>) {
    let mut reported = FxHashSet::default();
    for c in clocked {
        for case in c
            .node
            .descendants()
            .filter(|n| n.kind() == SyntaxKind::CASE_STMT)
        {
            let Some(selector) = case.children().next() else {
                continue;
            };
            if selector.kind() != SyntaxKind::NAME {
                continue;
            }
            let Some((id, decl)) = u.named(&selector) else {
                continue;
            };
            if !c.registers.contains(&id) || !reported.insert(id) {
                continue;
            }
            let def = u
                .analysis
                .base_type(decl.subtype.unwrap_or(id))
                .and_then(|b| u.decl(b))
                .and_then(|b| b.type_def.clone());
            match def.as_deref() {
                Some(TypeDef::Enum(literals)) if u.on(codes::UNSAFE_FSM) => {
                    // A safe-state attribute anywhere on the type or the
                    // signal.
                    let text = u.text.to_ascii_lowercase();
                    let safe = [
                        "fsm_safe_state",
                        "safe_state",
                        "syn_encoding",
                        "fsm_encoding",
                        "enum_encoding",
                    ]
                    .iter()
                    .any(|a| text.contains(a));
                    let user_type = u
                        .analysis
                        .base_type(decl.subtype.unwrap_or(id))
                        .and_then(|b| u.decl(b))
                        .is_some_and(|b| !u.analysis.files()[b.file as usize].builtin);
                    if !safe && user_type && literals.len() > 2 {
                        u.push(
                            out,
                            codes::UNSAFE_FSM,
                            decl.range,
                            format!(
                                "state machine `{}` has no safe encoding: an upset can leave it in an illegal state it never leaves; set your synthesis tool's safe-state attribute",
                                decl.spelling
                            ),
                        );
                    }
                }
                Some(TypeDef::Array { .. }) if u.on(codes::STATE_TYPE) => {
                    // Choices that are named constants: an encoded state.
                    let named_choices = case
                        .descendants()
                        .filter(|n| n.kind() == SyntaxKind::CHOICES)
                        .flat_map(|ch| ch.children())
                        .filter(|n| n.kind() == SyntaxKind::NAME)
                        .filter(|n| {
                            u.named(n)
                                .is_some_and(|(_, d)| d.kind == DeclKind::Constant)
                        })
                        .count();
                    if named_choices >= 2 {
                        u.push(
                            out,
                            codes::STATE_TYPE,
                            decl.range,
                            format!(
                                "state register `{}` is a vector of named constants: an enumeration type lets tools check and encode the states",
                                decl.spelling
                            ),
                        );
                    }
                }
                _ => {}
            }
        }
    }
}

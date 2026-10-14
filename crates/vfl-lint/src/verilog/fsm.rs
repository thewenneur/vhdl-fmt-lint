//! State machines: a `case` on a state variable whose items assign the
//! next state. A state is an enumeration member, or a constant used as a
//! `case` label (`localparam IDLE = 0;`). States no transition reaches,
//! and states nothing but a reset leaves, are reported.

use std::collections::{HashMap, HashSet};

use vfl_sema::codes;
use vfl_verilog_syntax::{SyntaxKind as K, SyntaxNode, TextRange};

use super::Unit;
use super::procedures::{assignment_targets, case_item, case_selector, if_parts};
use super::scope::{self, Kind, ident};
use crate::Finding;

/// The members of the enumeration `expr` (a variable) has, if it has one
/// the unit or its file declares.
pub fn enum_members(unit: &Unit, expr: &SyntaxNode) -> Option<Vec<String>> {
    let name = simple_name(expr)?;
    let decl = unit.decl(&name)?;
    let enumeration = enum_type(unit, decl.data_type.as_ref()?)?;
    Some(
        enumeration
            .children()
            .filter(|n| n.kind() == K::ENUM_MEMBER)
            .filter_map(|m| scope::name_token(&m))
            .map(|t| ident(&t))
            .collect(),
    )
}

/// The `ENUM_TYPE` of a data type, through a type name the unit or its
/// file declares.
fn enum_type(unit: &Unit, data_type: &SyntaxNode) -> Option<SyntaxNode> {
    if let Some(e) = data_type.children().find(|n| n.kind() == K::ENUM_TYPE) {
        return Some(e);
    }
    let type_name = data_type
        .children()
        .find(|n| n.kind() == K::NAME_REF && scope::is_simple_ref(n))
        .and_then(|n| scope::ref_name(&n))
        .map(|t| ident(&t))?;
    let typedef = unit
        .scope
        .named(&type_name)
        .find(|d| d.kind == Kind::Type)
        .cloned()
        .or_else(|| {
            scope::Scope::collect(unit.ctx.root)
                .named(&type_name)
                .find(|d| d.kind == Kind::Type)
                .cloned()
        })?;
    typedef
        .data_type?
        .children()
        .find(|n| n.kind() == K::ENUM_TYPE)
}

fn simple_name(expr: &SyntaxNode) -> Option<String> {
    let expr = if expr.kind() == K::PAREN_EXPR {
        expr.first_child()?
    } else {
        expr.clone()
    };
    (expr.kind() == K::NAME_REF && scope::is_simple_ref(&expr))
        .then(|| scope::ref_name(&expr))
        .flatten()
        .map(|t| ident(&t))
}

/// The states an expression assigns: `Some(vec![])` for the state
/// variables themselves (no transition), `None` for anything else.
fn state_values(
    expr: &SyntaxNode,
    states: &[String],
    variables: &HashSet<String>,
) -> Option<Vec<String>> {
    match expr.kind() {
        K::PAREN_EXPR => state_values(&expr.first_child()?, states, variables),
        K::CONDITIONAL_EXPR => {
            let mut out = Vec::new();
            for branch in expr.children().skip(1) {
                out.extend(state_values(&branch, states, variables)?);
            }
            Some(out)
        }
        _ => {
            let name = simple_name(expr)?;
            if states.contains(&name) {
                Some(vec![name])
            } else if variables.contains(&name) {
                Some(Vec::new())
            } else {
                None
            }
        }
    }
}

struct Machine {
    /// The state variable and the variables holding its next value.
    variables: HashSet<String>,
    states: Vec<String>,
    /// The enumeration's type name, for messages.
    type_name: Option<String>,
    cases: Vec<SyntaxNode>,
}

pub fn check(unit: &Unit, enabled: &impl Fn(&str) -> bool, out: &mut Vec<Finding>) {
    if !enabled(codes::FSM_UNREACHABLE_STATE) && !enabled(codes::FSM_STUCK_STATE) {
        return;
    }
    let procedures: Vec<SyntaxNode> = unit
        .node
        .descendants()
        .filter(|n| n.kind() == K::PROCEDURE)
        .collect();
    // `state <= next;`: `next` holds the state's next value.
    let mut copies: Vec<(String, String)> = Vec::new();
    let assigns = procedures
        .iter()
        .flat_map(|p| p.descendants())
        .filter(|n| matches!(n.kind(), K::BLOCKING_ASSIGN | K::NONBLOCKING_ASSIGN));
    for assign in assigns {
        let mut children = assign.children();
        if let (Some(target), Some(value)) = (children.next(), children.next())
            && let (Some(t), Some(v)) = (simple_name(&target), simple_name(&value))
        {
            copies.push((t, v));
        }
    }
    let mut machines: Vec<Machine> = Vec::new();
    for case in procedures
        .iter()
        .flat_map(|p| p.descendants())
        .filter(|n| n.kind() == K::CASE_STMT)
    {
        let Some(selector) = case_selector(&case) else {
            continue;
        };
        let Some(variable) = simple_name(&selector) else {
            continue;
        };
        if let Some(machine) = machines
            .iter_mut()
            .find(|m| m.variables.contains(&variable))
        {
            machine.cases.push(case);
            continue;
        }
        let (states, type_name) = match enum_members(unit, &selector) {
            Some(members) => {
                let type_name = unit
                    .decl(&variable)
                    .and_then(|d| d.data_type.clone())
                    .and_then(|t| t.children().find(|n| n.kind() == K::NAME_REF))
                    .map(|n| n.text().to_string());
                (members, type_name)
            }
            None => {
                // `localparam IDLE = 0;` constants as labels.
                let labels: Vec<String> = case
                    .children()
                    .filter(|n| n.kind() == K::CASE_ITEM)
                    .flat_map(|i| case_item(&i).0)
                    .map(|l| simple_name(&l))
                    .collect::<Option<_>>()
                    .unwrap_or_default();
                let constants = labels.iter().all(|l| {
                    unit.decl(l)
                        .is_some_and(|d| matches!(d.kind, Kind::LocalParam | Kind::Param))
                });
                if labels.len() < 2 || !constants {
                    continue;
                }
                (labels, None)
            }
        };
        let mut variables: HashSet<String> = HashSet::from([variable.clone()]);
        for (target, value) in &copies {
            if *target == variable {
                variables.insert(value.clone());
            }
        }
        machines.push(Machine {
            variables,
            states,
            type_name,
            cases: vec![case],
        });
    }
    let mut reported: HashSet<TextRange> = HashSet::new();
    for machine in machines {
        analyze(unit, &procedures, &machine, enabled, &mut reported, out);
    }
}

fn analyze(
    unit: &Unit,
    procedures: &[SyntaxNode],
    machine: &Machine,
    enabled: &impl Fn(&str) -> bool,
    reported: &mut HashSet<TextRange>,
    out: &mut Vec<Finding>,
) {
    let states = &machine.states;
    let items: Vec<SyntaxNode> = machine
        .cases
        .iter()
        .flat_map(|c| c.children().filter(|n| n.kind() == K::CASE_ITEM))
        .collect();
    if items
        .iter()
        .any(|i| i.descendants().any(|n| n.kind() == K::MACRO_ITEM))
    {
        return;
    }
    let inside_item = |node: &SyntaxNode| {
        items
            .iter()
            .any(|i| i.text_range().contains_range(node.text_range()))
    };
    // Every assignment to the state variables, with the states it assigns.
    let mut transitions: Vec<(SyntaxNode, Vec<String>)> = Vec::new();
    for assign in procedures.iter().flat_map(|p| p.descendants()) {
        let targets = assignment_targets(&assign);
        if !targets.iter().any(|(t, _)| {
            scope::ref_name(t).is_some_and(|n| machine.variables.contains(&ident(&n)))
        }) {
            continue;
        }
        let Some(value) = assign.children().nth(1) else {
            return;
        };
        let Some(values) = state_values(&value, states, &machine.variables) else {
            // A next state computed some other way: transitions unknown.
            return;
        };
        transitions.push((assign, values));
    }
    // A `case` on a variable nothing assigns states to decodes it.
    if !transitions
        .iter()
        .any(|(assign, values)| inside_item(assign) && !values.is_empty())
    {
        return;
    }
    let mut resets: Vec<String> = Vec::new();
    let mut always: Vec<String> = Vec::new();
    for (assign, values) in &transitions {
        if inside_item(assign) {
            continue;
        }
        if in_reset(assign) {
            resets.extend(values.iter().cloned());
        } else {
            always.extend(values.iter().cloned());
        }
    }
    for decl in machine.variables.iter().filter_map(|v| unit.decl(v)) {
        if decl.init
            && let Some(init) = decl.node.children().last()
            && let Some(values) = state_values(&init, states, &machine.variables)
        {
            resets.extend(values);
        }
    }
    // The items naming each state, and the `default` items.
    let mut by_state: HashMap<&str, Vec<&SyntaxNode>> = HashMap::new();
    let mut defaults: Vec<&SyntaxNode> = Vec::new();
    for item in &items {
        let (labels, _) = case_item(item);
        if labels.is_empty() {
            defaults.push(item);
        }
        for label in labels {
            if let Some(name) = simple_name(&label)
                && let Some(state) = states.iter().find(|s| **s == name)
            {
                by_state.entry(state.as_str()).or_default().push(item);
            }
        }
    }
    let next = |state: &str| -> HashSet<String> {
        let items = by_state
            .get(state)
            .cloned()
            .unwrap_or_else(|| defaults.clone());
        let mut out: HashSet<String> = always.iter().cloned().collect();
        for (assign, values) in &transitions {
            if items
                .iter()
                .any(|i| i.text_range().contains_range(assign.text_range()))
            {
                out.extend(values.iter().cloned());
            }
        }
        out
    };
    let state_range = |state: &str| -> Option<TextRange> {
        unit.scope
            .named(state)
            .find(|d| matches!(d.kind, Kind::EnumMember | Kind::LocalParam | Kind::Param))
            .map(|d| d.range)
    };
    let variable = machine
        .cases
        .first()
        .and_then(case_selector)
        .and_then(|s| simple_name(&s))
        .unwrap_or_default();
    let of_type = machine
        .type_name
        .as_ref()
        .map(|t| format!(" of `{t}`"))
        .unwrap_or_default();
    if enabled(codes::FSM_UNREACHABLE_STATE) && (!resets.is_empty() || !always.is_empty()) {
        let mut reached: HashSet<String> = resets.iter().chain(&always).cloned().collect();
        let mut queue: Vec<String> = reached.iter().cloned().collect();
        while let Some(state) = queue.pop() {
            for n in next(&state) {
                if reached.insert(n.clone()) {
                    queue.push(n);
                }
            }
        }
        for state in states.iter().filter(|s| !reached.contains(*s)) {
            let Some(range) = state_range(state) else {
                continue;
            };
            if reported.insert(range) {
                out.push(Finding::new(
                    codes::FSM_UNREACHABLE_STATE,
                    range,
                    format!("no transition of `{variable}` reaches state `{state}`{of_type}"),
                ));
            }
        }
    }
    if enabled(codes::FSM_STUCK_STATE) {
        for (state, items) in &by_state {
            if next(state).iter().any(|n| n != state) {
                continue;
            }
            let Some(label) = items.first().and_then(|i| {
                case_item(i)
                    .0
                    .into_iter()
                    .find(|l| simple_name(l).as_deref() == Some(*state))
            }) else {
                continue;
            };
            if reported.insert(label.text_range()) {
                out.push(Finding::new(
                    codes::FSM_STUCK_STATE,
                    label.text_range(),
                    format!(
                        "`{variable}` never leaves state `{state}` once there, except by a reset"
                    ),
                ));
            }
        }
    }
}

/// Whether an assignment is in the branch of an `if` that tests a reset.
fn in_reset(assign: &SyntaxNode) -> bool {
    assign.ancestors().any(|a| {
        a.kind() == K::IF_STMT
            && if_parts(&a).is_some_and(|(condition, then, _)| {
                then.is_some_and(|t| t.text_range().contains_range(assign.text_range()))
                    && condition.descendants().any(|n| {
                        n.kind() == K::NAME_REF
                            && scope::ref_name(&n).is_some_and(|t| {
                                let name = t.text().to_ascii_lowercase();
                                name.contains("rst") || name.contains("reset")
                            })
                    })
            })
    })
}

//! State machine detection: a `case` on an object of an enumeration type
//! whose alternatives assign literals of that type to it (one-process
//! style) or to a next-state object of the same type (two-process style).

use std::collections::HashMap;

use vfl_syntax::{SyntaxKind, SyntaxNode, identifier_key};

use crate::comments::CommentStyle;
use crate::model::{after_token, compact, label, own_name};

/// A detected state machine.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Fsm {
    /// The state object, as written.
    pub signal: String,
    /// The next-state object of a two-process machine.
    pub next: Option<String>,
    /// The state type, as written.
    pub type_name: String,
    /// Every literal of the state type, in declaration order.
    pub states: Vec<String>,
    /// The state the reset branch or the declaration's default value sets.
    pub initial: Option<String>,
    pub transitions: Vec<Transition>,
    /// The label of the process with the `case`.
    pub process: Option<String>,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transition {
    pub from: String,
    pub to: String,
    /// The conditions of the `if` branches around the assignment, joined
    /// with `and`.
    pub condition: Option<String>,
}

/// The types a state machine's state can have, and the record types that
/// can hold such a state in an element (`engine.state`).
#[derive(Debug, Clone, Default)]
pub struct StateTypes {
    /// Type key -> literals as written.
    enums: HashMap<String, Vec<String>>,
    /// Type key -> (element name as written, element type as written).
    records: HashMap<String, Vec<(String, String)>>,
}

impl StateTypes {
    fn enum_literals<'a>(&'a self, outer: &'a StateTypes, key: &str) -> Option<&'a Vec<String>> {
        self.enums.get(key).or_else(|| outer.enums.get(key))
    }

    fn record<'a>(&'a self, outer: &'a StateTypes, key: &str) -> Option<&'a Vec<(String, String)>> {
        self.records.get(key).or_else(|| outer.records.get(key))
    }
}

/// Adds the enumeration and record types declared directly in `node`.
pub fn collect_types(node: &SyntaxNode, types: &mut StateTypes) {
    for decl in node
        .children()
        .filter(|n| n.kind() == SyntaxKind::TYPE_DECL)
    {
        if let Some(def) = decl
            .children()
            .find(|n| n.kind() == SyntaxKind::RECORD_TYPE_DEF)
        {
            let mut elements = Vec::new();
            for element in def
                .children()
                .filter(|n| n.kind() == SyntaxKind::ELEMENT_DECL)
            {
                let Some(ty) = element
                    .children()
                    .find(|n| n.kind() == SyntaxKind::SUBTYPE_INDICATION)
                    .map(|n| compact(&n))
                else {
                    continue;
                };
                for name in element
                    .children()
                    .filter(|n| n.kind() == SyntaxKind::IDENT_LIST)
                    .flat_map(|n| idents(&n))
                {
                    elements.push((name, ty.clone()));
                }
            }
            if let Some(name) = own_name(&decl) {
                types.records.insert(identifier_key(&name), elements);
            }
            continue;
        }
        let Some(def) = decl
            .children()
            .find(|n| n.kind() == SyntaxKind::ENUM_TYPE_DEF)
        else {
            continue;
        };
        let literals: Vec<String> = def
            .children_with_tokens()
            .filter_map(|e| e.into_token())
            .filter(|t| matches!(t.kind(), SyntaxKind::IDENT | SyntaxKind::EXT_IDENT))
            .map(|t| t.text().to_owned())
            .collect();
        // Character literal types (`bit`-like) are not state types.
        if literals.len() < 2 {
            continue;
        }
        if let Some(name) = own_name(&decl) {
            types.enums.insert(identifier_key(&name), literals);
        }
    }
}

/// An object whose type is an enumeration type.
struct StateObject {
    spelling: String,
    type_key: String,
    type_name: String,
    default: Option<String>,
    description: String,
}

/// The state machines of an architecture.
pub fn detect(arch: &SyntaxNode, types: &StateTypes, style: &CommentStyle) -> Vec<Fsm> {
    // The architecture's own types, looked up before the packages' ones.
    let mut local = StateTypes::default();
    collect_types(arch, &mut local);
    let enum_literals = |key: &str| local.enum_literals(types, key);
    let mut objects: HashMap<String, StateObject> = HashMap::new();
    for decl in arch.descendants().filter(|n| {
        matches!(
            n.kind(),
            SyntaxKind::SIGNAL_DECL | SyntaxKind::VARIABLE_DECL
        )
    }) {
        let Some(type_name) = decl
            .children()
            .find(|n| n.kind() == SyntaxKind::SUBTYPE_INDICATION)
            .map(|n| compact(&n))
        else {
            continue;
        };
        let type_key = identifier_key(&type_name);
        // The object itself, or its record elements, with a state type.
        let mut fields: Vec<(String, String)> = Vec::new();
        if enum_literals(&type_key).is_some() {
            fields.push((String::new(), type_name.clone()));
        } else if let Some(elements) = local.record(types, &type_key) {
            for (element, ty) in elements {
                if enum_literals(&identifier_key(ty)).is_some() {
                    fields.push((format!(".{element}"), ty.clone()));
                }
            }
        }
        if fields.is_empty() {
            continue;
        }
        let description = style.doc(&decl);
        let default = after_token(&decl, SyntaxKind::COLON_EQ);
        let names: Vec<String> = decl
            .children()
            .filter(|n| n.kind() == SyntaxKind::IDENT_LIST)
            .flat_map(|n| idents(&n))
            .collect();
        for name in names {
            for (suffix, ty) in &fields {
                let spelling = format!("{name}{suffix}");
                objects.insert(
                    identifier_key(&spelling),
                    StateObject {
                        spelling,
                        type_key: identifier_key(ty),
                        type_name: ty.clone(),
                        default: if suffix.is_empty() {
                            default.clone()
                        } else {
                            None
                        },
                        description: description.clone(),
                    },
                );
            }
        }
    }
    if objects.is_empty() {
        return Vec::new();
    }

    let mut machines: Vec<(String, Fsm)> = Vec::new();
    // The machine whose state object is `key`, created on first use.
    let machine =
        |machines: &mut Vec<(String, Fsm)>, key: &str, at: &SyntaxNode| -> Option<usize> {
            let object = objects.get(key)?;
            let states = enum_literals(&object.type_key)?;
            if let Some(index) = machines.iter().position(|(k, _)| k == key) {
                return Some(index);
            }
            machines.push((
                key.to_owned(),
                Fsm {
                    signal: object.spelling.clone(),
                    type_name: object.type_name.clone(),
                    states: states.clone(),
                    process: at
                        .ancestors()
                        .find(|n| n.kind() == SyntaxKind::PROCESS_STMT)
                        .and_then(|p| label(&p)),
                    description: object.description.clone(),
                    ..Fsm::default()
                },
            ));
            Some(machines.len() - 1)
        };
    // Records that the machine `key` goes from each of `from` to `to`
    // under `condition`, through the object `target`.
    let add = |fsm: &mut Fsm,
               key: &str,
               target_key: &str,
               from: &[String],
               to: &str,
               condition: Option<String>| {
        if target_key != key && fsm.next.is_none() {
            fsm.next = objects.get(target_key).map(|o| o.spelling.clone());
        }
        for from in from {
            let transition = Transition {
                from: from.clone(),
                to: to.to_owned(),
                condition: condition.clone(),
            };
            if !fsm.transitions.contains(&transition) {
                fsm.transitions.push(transition);
            }
        }
    };
    // The state object an assignment targets, if it has the type of `key`.
    let target_of = |assign: &SyntaxNode, key: &str| -> Option<String> {
        let target = match assign.kind() {
            SyntaxKind::SELECTED_ASSIGN_STMT => {
                let mut after_select = assign
                    .children_with_tokens()
                    .skip_while(|e| e.kind() != SyntaxKind::SELECT_KW)
                    .filter_map(|e| e.into_node());
                object_name(&after_select.next()?)?
            }
            _ => object_name(&assign.children().next()?)?,
        };
        let target_key = identifier_key(&target);
        let object = objects.get(&target_key)?;
        (object.type_key == objects.get(key)?.type_key).then_some(target_key)
    };

    // `case state is when S => state <= T; ...`, in clocked, asynchronous
    // reset, combinational or `wait` processes alike.
    for case in arch
        .descendants()
        .filter(|n| n.kind() == SyntaxKind::CASE_STMT)
    {
        let Some(key) = selector(&case).map(|s| identifier_key(&s)) else {
            continue;
        };
        let Some(index) = machine(&mut machines, &key, &case) else {
            continue;
        };
        let states = machines[index].1.states.clone();
        let mut covered: Vec<String> = Vec::new();
        for alternative in case
            .children()
            .filter(|n| n.kind() == SyntaxKind::CASE_ALTERNATIVE)
        {
            let from = choices(&alternative, &states, &covered);
            covered.extend(from.iter().cloned());
            for assign in alternative.descendants().filter(|n| {
                matches!(
                    n.kind(),
                    SyntaxKind::SIGNAL_ASSIGN_STMT | SyntaxKind::VARIABLE_ASSIGN_STMT
                )
            }) {
                let Some(target_key) = target_of(&assign, &key) else {
                    continue;
                };
                let outer = conditions(&assign, &alternative);
                for (to, inner) in assigned_values(&assign, &states) {
                    let condition = match (&outer, inner) {
                        (Some(a), Some(b)) => Some(format!("{a} and {b}")),
                        (a, b) => a.clone().or(b),
                    };
                    add(
                        &mut machines[index].1,
                        &key,
                        &target_key,
                        &from,
                        &to,
                        condition,
                    );
                }
            }
        }
    }

    // `with state select state_next <= T when S, ...`, concurrent or
    // sequential.
    for select in arch
        .descendants()
        .filter(|n| n.kind() == SyntaxKind::SELECTED_ASSIGN_STMT)
    {
        let Some(key) = select
            .children()
            .next()
            .and_then(|n| object_name(&n))
            .map(|s| identifier_key(&s))
        else {
            continue;
        };
        let Some(target_key) = target_of(&select, &key) else {
            continue;
        };
        let Some(index) = machine(&mut machines, &key, &select) else {
            continue;
        };
        let states = machines[index].1.states.clone();
        let Some(waveforms) = select
            .children()
            .find(|n| n.kind() == SyntaxKind::SELECTED_WAVEFORMS)
        else {
            continue;
        };
        let mut covered: Vec<String> = Vec::new();
        let mut to: Option<String> = None;
        for part in waveforms.children() {
            match part.kind() {
                SyntaxKind::WAVEFORM => to = waveform_literal(&part, &states),
                SyntaxKind::CHOICES => {
                    let from = choices_of(&part, &states, &covered);
                    covered.extend(from.iter().cloned());
                    if let Some(to) = &to {
                        add(&mut machines[index].1, &key, &target_key, &from, to, None);
                    }
                }
                _ => {}
            }
        }
    }

    // `state_next <= T when state = S and c else ...` outside of a `case`:
    // the branches whose condition tests the state.
    for assign in arch.descendants().filter(|n| {
        n.kind() == SyntaxKind::SIGNAL_ASSIGN_STMT
            && n.children()
                .any(|c| c.kind() == SyntaxKind::CONDITIONAL_WAVEFORMS)
            && !n
                .ancestors()
                .any(|a| a.kind() == SyntaxKind::CASE_ALTERNATIVE)
    }) {
        let Some(waveforms) = assign
            .children()
            .find(|n| n.kind() == SyntaxKind::CONDITIONAL_WAVEFORMS)
        else {
            continue;
        };
        // The branches: (value, condition), the last one without.
        let mut branches: Vec<(SyntaxNode, Option<SyntaxNode>)> = Vec::new();
        for element in waveforms.children_with_tokens() {
            match element.kind() {
                SyntaxKind::WAVEFORM => {
                    if let Some(node) = element.into_node() {
                        branches.push((node, None));
                    }
                }
                SyntaxKind::WHEN_KW | SyntaxKind::ELSE_KW => {}
                _ => {
                    if let (Some(node), Some(last)) = (element.into_node(), branches.last_mut()) {
                        last.1 = Some(node);
                    }
                }
            }
        }
        // States the branches so far leave unconditionally, or under a
        // further condition.
        let mut machine_key: Option<String> = None;
        let mut full: Vec<String> = Vec::new();
        let mut partial: Vec<String> = Vec::new();
        for (value, condition) in &branches {
            let Some(condition) = condition else {
                // `else T`: from every state not left yet.
                let Some(key) = &machine_key else { break };
                let Some(index) = machines.iter().position(|(k, _)| k == key) else {
                    break;
                };
                let Some(target_key) = target_of(&assign, key) else {
                    break;
                };
                let states = machines[index].1.states.clone();
                let Some(to) = waveform_literal(value, &states) else {
                    break;
                };
                let untested: Vec<String> = states
                    .iter()
                    .filter(|s| !full.contains(s) && !partial.contains(s))
                    .cloned()
                    .collect();
                add(
                    &mut machines[index].1,
                    key,
                    &target_key,
                    &untested,
                    &to,
                    None,
                );
                let rest: Vec<String> = partial
                    .iter()
                    .filter(|s| !full.contains(s))
                    .cloned()
                    .collect();
                add(
                    &mut machines[index].1,
                    key,
                    &target_key,
                    &rest,
                    &to,
                    Some("else".into()),
                );
                break;
            };
            let mut terms = Vec::new();
            conjuncts(condition, &mut terms);
            // The term `state = S` names the state it leaves from.
            let found = terms.iter().enumerate().find_map(|(i, term)| {
                let (key, literal_text) = state_test(term)?;
                Some((i, key, literal_text))
            });
            let Some((i, key, literal_text)) = found else {
                continue;
            };
            let Some(target_key) = target_of(&assign, &key) else {
                continue;
            };
            let Some(index) = machine(&mut machines, &key, &assign) else {
                continue;
            };
            machine_key = Some(key.clone());
            let states = machines[index].1.states.clone();
            let (Some(from), Some(to)) = (
                literal(&literal_text, &states),
                waveform_literal(value, &states),
            ) else {
                continue;
            };
            let rest: Vec<String> = terms
                .iter()
                .enumerate()
                .filter(|(j, _)| *j != i)
                .map(|(_, t)| compact(t))
                .collect();
            if rest.is_empty() {
                full.push(from.clone());
            } else {
                partial.push(from.clone());
            }
            let condition = (!rest.is_empty()).then(|| rest.join(" and "));
            add(
                &mut machines[index].1,
                &key,
                &target_key,
                &[from],
                &to,
                condition,
            );
        }
    }

    // The initial state: a literal assigned to the state object outside of
    // the `case` (the reset branch), else its default value.
    for (key, fsm) in &mut machines {
        let states = fsm.states.clone();
        let reset = arch
            .descendants()
            .filter(|n| {
                n.kind() == SyntaxKind::SIGNAL_ASSIGN_STMT
                    || n.kind() == SyntaxKind::VARIABLE_ASSIGN_STMT
            })
            .filter(|assign| {
                assign
                    .children()
                    .next()
                    .and_then(|t| object_name(&t))
                    .is_some_and(|t| identifier_key(&t) == *key)
            })
            .filter(|assign| {
                !assign.ancestors().any(|a| {
                    a.kind() == SyntaxKind::CASE_STMT
                        && selector(&a).is_some_and(|s| identifier_key(&s) == *key)
                })
            })
            .find_map(|assign| {
                assigned_values(&assign, &states)
                    .into_iter()
                    .next()
                    .map(|(value, _)| value)
            });
        let default = objects[key.as_str()]
            .default
            .as_deref()
            .and_then(|d| literal(d, &states));
        fsm.initial = reset.or(default);
    }

    machines
        .into_iter()
        .map(|(_, fsm)| fsm)
        .filter(|fsm| !fsm.transitions.is_empty())
        .collect()
}

/// The object a `case` selects on, when it is a simple or selected name.
fn selector(case: &SyntaxNode) -> Option<String> {
    object_name(&case.children().next()?)
}

/// The text of a simple name (`state`) or a selected name of record
/// elements (`engine.state`).
fn object_name(node: &SyntaxNode) -> Option<String> {
    match node.kind() {
        SyntaxKind::NAME => Some(compact(node)),
        SyntaxKind::SELECTED_NAME => {
            let prefix = object_name(&node.children().next()?)?;
            let last = node
                .children_with_tokens()
                .filter_map(|e| e.into_token())
                .filter(|t| matches!(t.kind(), SyntaxKind::IDENT | SyntaxKind::EXT_IDENT))
                .last()?;
            Some(format!("{prefix}.{}", last.text()))
        }
        _ => None,
    }
}

fn idents(node: &SyntaxNode) -> Vec<String> {
    node.children_with_tokens()
        .filter_map(|e| e.into_token())
        .filter(|t| matches!(t.kind(), SyntaxKind::IDENT | SyntaxKind::EXT_IDENT))
        .map(|t| t.text().to_owned())
        .collect()
}

/// The states an alternative's choices name; `others` is every state no
/// earlier alternative covered.
fn choices(alternative: &SyntaxNode, states: &[String], covered: &[String]) -> Vec<String> {
    alternative
        .children()
        .find(|n| n.kind() == SyntaxKind::CHOICES)
        .map(|c| choices_of(&c, states, covered))
        .unwrap_or_default()
}

/// The states a `CHOICES` node names; `others` is every state not covered.
fn choices_of(choices: &SyntaxNode, states: &[String], covered: &[String]) -> Vec<String> {
    if choices
        .children_with_tokens()
        .any(|e| e.kind() == SyntaxKind::OTHERS_KW)
    {
        return states
            .iter()
            .filter(|s| !covered.contains(s))
            .cloned()
            .collect();
    }
    choices
        .children()
        .filter_map(|n| literal(&compact(&n), states))
        .collect()
}

/// The literal of `states` that `text` names, as declared.
fn literal(text: &str, states: &[String]) -> Option<String> {
    let key = identifier_key(text.trim());
    states.iter().find(|s| identifier_key(s) == key).cloned()
}

/// The state literal a waveform is, when it is just one.
fn waveform_literal(waveform: &SyntaxNode, states: &[String]) -> Option<String> {
    let elements: Vec<SyntaxNode> = waveform.children().collect();
    if elements.len() != 1 {
        return None;
    }
    literal(&compact(&elements[0].children().next()?), states)
}

/// The values an assignment can assign, each with the condition of its
/// branch: one for `s <= T`, one per branch for `s <= A when c else B`.
fn assigned_values(assign: &SyntaxNode, states: &[String]) -> Vec<(String, Option<String>)> {
    let Some(waveforms) = assign
        .children()
        .find(|n| n.kind() == SyntaxKind::CONDITIONAL_WAVEFORMS)
    else {
        return assigned_literal(assign, states)
            .map(|l| vec![(l, None)])
            .unwrap_or_default();
    };
    let mut out = Vec::new();
    let mut previous: Vec<String> = Vec::new();
    let mut value: Option<String> = None;
    let mut expect_condition = false;
    let flush = |value: &mut Option<String>, condition: Option<String>, out: &mut Vec<_>| {
        if let Some(v) = value.take() {
            out.push((v, condition));
        }
    };
    for element in waveforms.children_with_tokens() {
        match element.kind() {
            SyntaxKind::WHEN_KW => expect_condition = true,
            SyntaxKind::WAVEFORM => {
                value = element
                    .as_node()
                    .and_then(|w| waveform_literal(w, states))
                    .or(Some(String::new()));
            }
            _ => {
                if let Some(node) = element.as_node().filter(|_| expect_condition) {
                    expect_condition = false;
                    let condition = compact(node);
                    flush(&mut value, Some(condition.clone()), &mut out);
                    previous.push(condition);
                }
            }
        }
    }
    // The final `else` value.
    let otherwise = match previous.as_slice() {
        [] => None,
        [one] => Some(format!("not ({one})")),
        many => Some(format!("not ({})", many.join(" or "))),
    };
    flush(&mut value, otherwise, &mut out);
    out.retain(|(v, _)| !v.is_empty());
    out
}

/// The terms of a chain of `and`s.
fn conjuncts(node: &SyntaxNode, out: &mut Vec<SyntaxNode>) {
    let is_and = node.kind() == SyntaxKind::BINARY_EXPR
        && node
            .children_with_tokens()
            .any(|e| e.kind() == SyntaxKind::AND_KW);
    if is_and {
        for child in node.children() {
            conjuncts(&child, out);
        }
    } else if node.kind() == SyntaxKind::PAREN_EXPR {
        match node.children().next() {
            Some(inner) if inner.kind() == SyntaxKind::BINARY_EXPR => conjuncts(&inner, out),
            _ => out.push(node.clone()),
        }
    } else {
        out.push(node.clone());
    }
}

/// `state = S`: the state object's key and the literal's text.
fn state_test(term: &SyntaxNode) -> Option<(String, String)> {
    if term.kind() != SyntaxKind::BINARY_EXPR
        || !term
            .children_with_tokens()
            .any(|e| e.kind() == SyntaxKind::EQ)
    {
        return None;
    }
    let mut sides = term.children();
    let left = sides.next()?;
    let right = sides.next()?;
    Some((identifier_key(&object_name(&left)?), compact(&right)))
}

/// The state literal an assignment assigns, when its value is just one.
fn assigned_literal(assign: &SyntaxNode, states: &[String]) -> Option<String> {
    let value = match assign.kind() {
        SyntaxKind::SIGNAL_ASSIGN_STMT => {
            let waveform = assign
                .children()
                .find(|n| n.kind() == SyntaxKind::WAVEFORM)?;
            let elements: Vec<SyntaxNode> = waveform.children().collect();
            if elements.len() != 1 {
                return None;
            }
            let value = elements[0].children().next()?;
            compact(&value)
        }
        _ => after_token(assign, SyntaxKind::COLON_EQ)?,
    };
    literal(&value, states)
}

/// The conditions under which `node` runs inside `scope`: the branches of
/// the `if` statements and `case` alternatives between them.
fn conditions(node: &SyntaxNode, scope: &SyntaxNode) -> Option<String> {
    let mut parts = Vec::new();
    let mut child = node.clone();
    while let Some(parent) = child.parent() {
        if parent == *scope {
            break;
        }
        match parent.kind() {
            SyntaxKind::IF_STMT => {
                if let Some(condition) = branch_condition(&parent, &child) {
                    parts.push(condition);
                }
            }
            SyntaxKind::CASE_ALTERNATIVE => {
                let case = parent.parent();
                let selector = case
                    .as_ref()
                    .and_then(|c| c.children().next())
                    .map(|n| compact(&n));
                let choices = parent
                    .children()
                    .find(|n| n.kind() == SyntaxKind::CHOICES)
                    .map(|n| compact(&n));
                if let (Some(selector), Some(choices)) = (selector, choices)
                    && !choices.eq_ignore_ascii_case("others")
                {
                    parts.push(format!("{selector} = {choices}"));
                }
            }
            _ => {}
        }
        child = parent;
    }
    parts.reverse();
    (!parts.is_empty()).then(|| parts.join(" and "))
}

/// The condition of the branch of `if_stmt` that holds `child`.
fn branch_condition(if_stmt: &SyntaxNode, child: &SyntaxNode) -> Option<String> {
    let mut conditions: Vec<String> = Vec::new();
    let mut current: Option<String> = None;
    let mut expect_condition = false;
    for element in if_stmt.children_with_tokens() {
        match element.kind() {
            SyntaxKind::IF_KW | SyntaxKind::ELSIF_KW => expect_condition = true,
            SyntaxKind::ELSE_KW => {
                current = Some(match conditions.as_slice() {
                    [one] => format!("not ({one})"),
                    many => format!("not ({})", many.join(" or ")),
                });
            }
            SyntaxKind::END_KW => break,
            _ => {
                if let Some(node) = element.as_node() {
                    if expect_condition {
                        let text = compact(node);
                        conditions.push(text.clone());
                        current = Some(text);
                        expect_condition = false;
                    } else if node == child {
                        return current;
                    }
                }
            }
        }
    }
    None
}

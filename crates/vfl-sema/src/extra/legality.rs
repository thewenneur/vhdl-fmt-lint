//! Statements and names the language does not allow, which compilers
//! reject and the rest of the analysis does not report.

use rustc_hash::{FxHashMap, FxHashSet};
use vfl_syntax::green_walk::ChildTokens;
use vfl_syntax::{SyntaxKind, SyntaxNode, TextRange};

use super::{Enabled, Unit, extra_codes as codes, token, trimmed};

use crate::analysis::Analysis;
use crate::decl::{Decl, DeclId, DeclKind, FileId, Mode, Target};
use crate::diagnostic::{Diagnostic, Edit, Fix};

pub(super) fn check(u: &Unit, out: &mut Vec<Diagnostic>) {
    if u.on(codes::READ_ONLY_TARGET) {
        read_only_targets(u, out);
    }
    if u.on(codes::STANDARD_FEATURE) && u.standard < vfl_syntax::Standard::Vhdl2008 {
        output_reads(u, out);
    }
    let kind = u.on(codes::ASSIGNMENT_KIND);
    let misplaced = u.on(codes::MISPLACED_STATEMENT);
    let returns = u.on(codes::RETURN_VALUE);
    let impure = u.on(codes::IMPURE_FUNCTION);
    let association = u.on(codes::ASSOCIATION);
    let attributes = u.on(codes::ATTRIBUTE_SPECIFICATION);
    let constraint = u.on(codes::CONSTRAINT);
    let view = u.on(codes::MODE_VIEW);
    if kind || misplaced || returns || impure || association || attributes || constraint || view {
        use SyntaxKind::*;
        if kind {
            for node in u
                .nodes(VARIABLE_ASSIGN_STMT)
                .iter()
                .chain(u.nodes(SIGNAL_ASSIGN_STMT))
            {
                assignment_kind(u, node, out);
            }
        }
        if misplaced || returns {
            for node in u.nodes(RETURN_STMT) {
                return_statement(u, node, misplaced, returns, out);
            }
        }
        if misplaced {
            for node in u.nodes(EXIT_STMT).iter().chain(u.nodes(NEXT_STMT)) {
                loop_control(u, node, out);
            }
        }
        for node in u.nodes(SUBPROGRAM_BODY) {
            if returns {
                function_returns(u, node, out);
            }
            if impure {
                pure_function(u, node, out);
            }
        }
        if association {
            for node in u
                .nodes(PORT_MAP_ASPECT)
                .iter()
                .chain(u.nodes(GENERIC_MAP_ASPECT))
            {
                if let Some(list) = node.children().find(|n| n.kind() == ASSOCIATION_LIST) {
                    associations(u, &list, out);
                }
            }
        }
        if attributes {
            for node in u.nodes(ATTRIBUTE_SPEC) {
                attribute_spec(u, node, out);
            }
        }
        if constraint {
            for node in u.nodes(SUBTYPE_INDICATION) {
                constraints(u, node, out);
            }
        }
        if view {
            for node in u.nodes(MODE_VIEW_DECL) {
                mode_view(u, node, out);
            }
        }
    }
    if u.on(codes::COMPONENT_MISMATCH) {
        components(u, out);
    }
    if u.on(codes::MISSING_BODY) {
        missing_bodies(u, out);
    }
    if u.on(codes::CONFLICTING_USE) {
        conflicting_uses(u, out);
    }
    if u.on(codes::CONFIGURATION) {
        configuration(u, out);
    }
}

// ---- E1: read-only targets ---------------------------------------------------

fn read_only_targets(u: &Unit, out: &mut Vec<Diagnostic>) {
    // Assignment targets only: the resolver also marks names inside a
    // target's index and the actuals of unresolved calls as writes.
    let assignments = u
        .nodes(SyntaxKind::SIGNAL_ASSIGN_STMT)
        .iter()
        .chain(u.nodes(SyntaxKind::VARIABLE_ASSIGN_STMT))
        .chain(u.nodes(SyntaxKind::SELECTED_ASSIGN_STMT));
    for statement in assignments {
        let Some((target, _)) = crate::rtl::assignment_target(statement) else {
            continue;
        };
        let Some((_, decl)) = u.target_object(&target) else {
            continue;
        };
        let what = match decl.kind {
            DeclKind::Port { mode: Mode::In, .. } => "an input port",
            DeclKind::Parameter { mode: Mode::In, .. } => "a parameter of mode `in`",
            DeclKind::Constant => "a constant",
            DeclKind::Generic { .. } => "a generic",
            DeclKind::LoopParameter => "a loop parameter",
            DeclKind::GenerateParameter => "a generate parameter",
            _ => continue,
        };
        u.push(
            out,
            codes::READ_ONLY_TARGET,
            trimmed(&target),
            format!("`{}` is {what} and cannot be assigned", decl.spelling),
        );
    }
}

// ---- E16: features of a later standard ---------------------------------------

/// Reads of `out` ports, which VHDL-2008 allows and earlier standards
/// do not.
fn output_reads(u: &Unit, out: &mut Vec<Diagnostic>) {
    for r in u.refs {
        if !r.access.reads() {
            continue;
        }
        let Some(decl) = super::single_decl(r).and_then(|id| u.decl(id)) else {
            continue;
        };
        if matches!(
            decl.kind,
            DeclKind::Port {
                mode: Mode::Out,
                ..
            }
        ) {
            u.push(
                out,
                codes::STANDARD_FEATURE,
                r.range,
                format!(
                    "`{}` is an output port: reading it requires VHDL-2008",
                    decl.spelling
                ),
            );
        }
    }
}

// ---- E2: `:=` to a signal, `<=` to a variable --------------------------------

fn assignment_kind(u: &Unit, node: &SyntaxNode, out: &mut Vec<Diagnostic>) {
    let Some(target) = node.children().find(|n| n.kind() != SyntaxKind::LABEL) else {
        return;
    };
    let Some((_, decl)) = u.target_object(&target) else {
        return;
    };
    let variable = node.kind() == SyntaxKind::VARIABLE_ASSIGN_STMT;
    let (wrong, right, what) = match decl.kind {
        DeclKind::Signal | DeclKind::Port { .. } if variable => {
            (SyntaxKind::COLON_EQ, "<=", "a signal")
        }
        DeclKind::Variable if !variable => (SyntaxKind::LTE, ":=", "a variable"),
        _ => return,
    };
    let Some(op) = token(node, wrong) else {
        return;
    };
    // A signal assignment with a delay or several waveform elements has
    // no variable form.
    let fixable = variable
        || node
            .children()
            .find(|n| n.kind() == SyntaxKind::WAVEFORM)
            .is_some_and(|w| {
                w.children().count() == 1
                    && !w
                        .descendants_with_tokens()
                        .any(|e| e.kind() == SyntaxKind::AFTER_KW)
            })
            && !node.has_child(|k| {
                matches!(
                    k,
                    SyntaxKind::DELAY_MECHANISM | SyntaxKind::CONDITIONAL_WAVEFORMS
                )
            });
    let message = format!("`{}` is {what}: assign it with `{right}`", decl.spelling);
    if fixable {
        u.push_fix(
            out,
            codes::ASSIGNMENT_KIND,
            op.text_range(),
            message,
            Fix {
                title: format!("Use `{right}`"),
                edits: vec![Edit {
                    range: op.text_range(),
                    text: right.into(),
                }],
                safe: true,
            },
        );
    } else {
        u.push(out, codes::ASSIGNMENT_KIND, op.text_range(), message);
    }
}

// ---- E3, E4: return, exit and next -------------------------------------------

/// The innermost subprogram body or process around a statement.
fn enclosing_body(node: &SyntaxNode) -> Option<SyntaxNode> {
    node.ancestors().skip(1).find(|n| {
        matches!(
            n.kind(),
            SyntaxKind::SUBPROGRAM_BODY | SyntaxKind::PROCESS_STMT
        )
    })
}

fn is_function(body: &SyntaxNode) -> bool {
    body.has_child(|k| k == SyntaxKind::FUNCTION_SPEC)
}

fn return_statement(
    u: &Unit,
    node: &SyntaxNode,
    misplaced: bool,
    returns: bool,
    out: &mut Vec<Diagnostic>,
) {
    let range = trimmed(node);
    match enclosing_body(node) {
        Some(body) if body.kind() == SyntaxKind::SUBPROGRAM_BODY => {
            if !returns {
                return;
            }
            let value = node.children().next().is_some();
            if is_function(&body) && !value {
                u.push(
                    out,
                    codes::RETURN_VALUE,
                    range,
                    "a function's `return` must give a value".into(),
                );
            } else if !is_function(&body) && value {
                u.push(
                    out,
                    codes::RETURN_VALUE,
                    range,
                    "a procedure's `return` cannot give a value".into(),
                );
            }
        }
        _ if misplaced => u.push(
            out,
            codes::MISPLACED_STATEMENT,
            range,
            "`return` outside a function or procedure".into(),
        ),
        _ => {}
    }
}

fn loop_control(u: &Unit, node: &SyntaxNode, out: &mut Vec<Diagnostic>) {
    let in_loop = node
        .ancestors()
        .skip(1)
        .take_while(|n| {
            !matches!(
                n.kind(),
                SyntaxKind::SUBPROGRAM_BODY | SyntaxKind::PROCESS_STMT
            )
        })
        .any(|n| n.kind() == SyntaxKind::LOOP_STMT);
    if !in_loop {
        let what = if node.kind() == SyntaxKind::EXIT_STMT {
            "exit"
        } else {
            "next"
        };
        u.push(
            out,
            codes::MISPLACED_STATEMENT,
            trimmed(node),
            format!("`{what}` outside a loop"),
        );
    }
}

/// A function whose statements have no `return`.
fn function_returns(u: &Unit, body: &SyntaxNode, out: &mut Vec<Diagnostic>) {
    if !is_function(body) {
        return;
    }
    let has_return = u
        .nodes_in(SyntaxKind::RETURN_STMT, body.text_range())
        .iter()
        .any(|n| enclosing_body(n).as_ref() == Some(body));
    if !has_return {
        let range = body
            .children()
            .find(|n| n.kind() == SyntaxKind::FUNCTION_SPEC)
            .map_or_else(|| trimmed(body), |s| trimmed(&s));
        u.push(
            out,
            codes::RETURN_VALUE,
            range,
            "this function has no `return` statement".into(),
        );
    }
}

// ---- E5: pure functions --------------------------------------------------------

fn pure_function(u: &Unit, body: &SyntaxNode, out: &mut Vec<Diagnostic>) {
    let Some(spec) = body
        .children()
        .find(|n| n.kind() == SyntaxKind::FUNCTION_SPEC)
    else {
        return;
    };
    if spec.has_child(|k| k == SyntaxKind::IMPURE_KW) {
        return;
    }
    // The function's own declaration, and the declarations inside it.
    let Some(name) = spec
        .children_with_tokens()
        .filter_map(|e| e.into_token())
        .find(|t| matches!(t.kind(), SyntaxKind::IDENT | SyntaxKind::EXT_IDENT))
    else {
        return;
    };
    let Some(function) = u.decl_at(name.text_range()) else {
        return;
    };
    let inside = |mut id: DeclId| {
        for _ in 0..32 {
            if id == function {
                return true;
            }
            let Some(parent) = u.decl(id).and_then(|d| d.parent) else {
                return false;
            };
            id = parent;
        }
        false
    };
    let mut reported = FxHashSet::default();
    // Not the parameter list: defaults are evaluated by the caller.
    let after_spec = TextRange::new(spec.text_range().end(), body.text_range().end());
    for r in u.refs_in(after_spec) {
        let Some(id) = super::single_decl(r) else {
            continue;
        };
        let Some(decl) = u.decl(id) else {
            continue;
        };
        let what = match decl.kind {
            DeclKind::Signal | DeclKind::Port { .. } => "signal",
            DeclKind::Variable => "variable",
            DeclKind::File => "file",
            DeclKind::Function => {
                // Calls of impure functions.
                let impure = u.analysis.files().get(decl.file as usize).is_some_and(|f| {
                    let start = usize::from(decl.full_range.start());
                    let end = usize::from(decl.range.start());
                    f.text
                        .get(start..end)
                        .is_some_and(|t| t.to_ascii_lowercase().contains("impure"))
                });
                if impure && !inside(id) && reported.insert(id) {
                    u.push(
                        out,
                        codes::IMPURE_FUNCTION,
                        r.range,
                        format!(
                            "pure function `{}` calls the impure function `{}`",
                            function_name(u, function),
                            decl.spelling
                        ),
                    );
                }
                continue;
            }
            _ => continue,
        };
        if inside(id) || decl.kind == DeclKind::Variable && is_protected_member(u, decl) {
            continue;
        }
        if reported.insert(id) {
            u.push(
                out,
                codes::IMPURE_FUNCTION,
                r.range,
                format!(
                    "pure function `{}` uses the {what} `{}` declared outside it; declare it `impure`",
                    function_name(u, function),
                    decl.spelling
                ),
            );
        }
    }
}

fn function_name(u: &Unit, id: DeclId) -> String {
    u.decl(id).map_or_else(String::new, |d| d.spelling.clone())
}

fn is_protected_member(u: &Unit, decl: &Decl) -> bool {
    let mut parent = decl.parent;
    while let Some(id) = parent {
        let Some(p) = u.decl(id) else {
            return false;
        };
        if p.kind == DeclKind::ProtectedBody {
            return true;
        }
        parent = p.parent;
    }
    false
}

// ---- E8: associations ----------------------------------------------------------

fn associations(u: &Unit, list: &SyntaxNode, out: &mut Vec<Diagnostic>) {
    let Some(parent) = list.parent() else {
        return;
    };
    // Port and generic maps, and calls of subprograms.
    let interface = match parent.kind() {
        SyntaxKind::PORT_MAP_ASPECT | SyntaxKind::GENERIC_MAP_ASPECT => map_interface(u, &parent),
        _ => return,
    };
    let elements: Vec<SyntaxNode> = list
        .children()
        .filter(|n| n.kind() == SyntaxKind::ASSOCIATION_ELEMENT)
        .collect();
    let mut named_seen = false;
    let mut positional = 0;
    for element in &elements {
        let named = element.has_child(|k| k == SyntaxKind::ARROW);
        if named {
            named_seen = true;
        } else if named_seen {
            u.push(
                out,
                codes::ASSOCIATION,
                trimmed(element),
                "a positional association cannot follow a named one".into(),
            );
            return;
        } else {
            positional += 1;
        }
    }
    if let Some(interface) = interface
        && positional > interface.len()
    {
        let what = if parent.kind() == SyntaxKind::PORT_MAP_ASPECT {
            "ports"
        } else {
            "generics"
        };
        u.push(
            out,
            codes::ASSOCIATION,
            trimmed(&elements[interface.len()]),
            format!(
                "{positional} positional associations, but the unit has {} {what}",
                interface.len()
            ),
        );
    }
}

/// The ports or generics of the unit a map's instantiation names.
pub(super) fn map_interface(u: &Unit, map: &SyntaxNode) -> Option<Vec<DeclId>> {
    let statement = map.parent()?;
    if statement.kind() != SyntaxKind::COMPONENT_INSTANTIATION_STMT {
        return None;
    }
    let ports = map.kind() == SyntaxKind::PORT_MAP_ASPECT;
    let unit = statement
        .descendants_with_tokens()
        .filter_map(|e| e.into_token())
        .take_while(|t| t.text_range().end() <= map.text_range().start())
        .filter(|t| matches!(t.kind(), SyntaxKind::IDENT | SyntaxKind::EXT_IDENT))
        .filter_map(|t| u.target(t.text_range()))
        .find(|&id| {
            u.decl(id)
                .is_some_and(|d| matches!(d.kind, DeclKind::Entity | DeclKind::Component))
        })?;
    let decl = u.decl(unit)?;
    if u.analysis.files()[decl.file as usize].builtin {
        return None;
    }
    Some(
        decl.members
            .as_ref()?
            .iter()
            .filter(|&id| {
                u.decl(id).is_some_and(|d| match d.kind {
                    DeclKind::Port { .. } => ports,
                    DeclKind::Generic { .. }
                    | DeclKind::GenericType
                    | DeclKind::GenericSubprogram
                    | DeclKind::GenericPackage => !ports,
                    _ => false,
                })
            })
            .collect(),
    )
}

// ---- E10: components and entities -------------------------------------------

/// The entity a component binds to by default: the one of its name in
/// the libraries of the file.
fn default_entity(u: &Unit, name: &str) -> Option<DeclId> {
    let libraries = &u.analysis.files()[u.file as usize].libraries;
    u.analysis
        .units()
        .iter()
        .enumerate()
        .find(|(_, info)| {
            info.kind == DeclKind::Entity
                && info.name == name
                && (libraries.contains(&info.library) || info.library == "work")
        })
        .map(|(i, _)| DeclId {
            unit: i as u32,
            index: 0,
        })
}

fn interface(u: &Unit, unit: &Decl) -> Vec<(DeclId, &'static str)> {
    unit.members
        .iter()
        .flat_map(|m| m.iter())
        .filter_map(|id| {
            let kind = match u.decl(id)?.kind {
                DeclKind::Port { .. } => "port",
                DeclKind::Generic { .. } => "generic",
                _ => return None,
            };
            Some((id, kind))
        })
        .collect()
}

fn components(u: &Unit, out: &mut Vec<Diagnostic>) {
    for component in u.decls.iter().filter(|d| d.kind == DeclKind::Component) {
        let Some(entity) = default_entity(u, &component.name) else {
            continue;
        };
        let Some(entity) = u.decl(entity) else {
            continue;
        };
        if u.analysis.files()[entity.file as usize].builtin {
            continue;
        }
        let theirs = interface(u, entity);
        let ours = interface(u, component);
        let find = |list: &[(DeclId, &'static str)], name: &str, kind: &str| {
            list.iter()
                .find(|(id, k)| *k == kind && u.decl(*id).is_some_and(|d| d.name == name))
                .map(|(id, _)| *id)
        };
        let mut problems = Vec::new();
        for &(id, kind) in &ours {
            let Some(decl) = u.decl(id) else { continue };
            let Some(other) = find(&theirs, &decl.name, kind).and_then(|o| u.decl(o)) else {
                problems.push(format!("{kind} `{}` is not in the entity", decl.spelling));
                continue;
            };
            if let (DeclKind::Port { mode: a, .. }, DeclKind::Port { mode: b, .. }) =
                (decl.kind, other.kind)
                && a != b
            {
                problems.push(format!(
                    "port `{}` is `{}` here but `{}` in the entity",
                    decl.spelling,
                    a.as_str(),
                    b.as_str()
                ));
            }
            if let (Some(a), Some(b)) = (u.base_type(decl), u.base_type(other))
                && a != b
            {
                let name = |id| u.decl(id).map_or_else(String::new, |d| d.spelling.clone());
                problems.push(format!(
                    "{kind} `{}` has type `{}` here but `{}` in the entity",
                    decl.spelling,
                    name(a),
                    name(b)
                ));
            }
        }
        for &(id, kind) in &theirs {
            let Some(decl) = u.decl(id) else { continue };
            let has_default = matches!(
                decl.kind,
                DeclKind::Port {
                    has_default: true,
                    ..
                } | DeclKind::Generic { has_default: true }
            );
            let output = matches!(decl.kind, DeclKind::Port { mode, .. } if mode != Mode::In);
            if !has_default && !output && find(&ours, &decl.name, kind).is_none() {
                problems.push(format!(
                    "the entity's {kind} `{}` is missing and has no default",
                    decl.spelling
                ));
            }
        }
        if !problems.is_empty() {
            u.push(
                out,
                codes::COMPONENT_MISMATCH,
                component.range,
                format!(
                    "component `{}` does not match its entity: {}",
                    component.spelling,
                    problems.join("; ")
                ),
            );
        }
    }
}

// ---- E7: missing bodies --------------------------------------------------------

fn missing_bodies(u: &Unit, out: &mut Vec<Diagnostic>) {
    let info = &u.analysis.units()[u.unit as usize];
    if info.kind != DeclKind::Package {
        return;
    }
    let root = u.decls.first();
    // Generic packages are instantiated, and their body may be elsewhere.
    let package = DeclId {
        unit: u.unit,
        index: 0,
    };
    let top: Vec<(usize, &Decl)> = u
        .decls
        .iter()
        .enumerate()
        .skip(1)
        .filter(|(_, d)| d.parent == Some(package))
        .collect();
    // What needs a body: subprogram declarations, deferred constants,
    // protected types.
    let mut needs: Vec<(&Decl, &'static str)> = Vec::new();
    for (_, decl) in &top {
        let node = u
            .node
            .covering_element(decl.full_range)
            .into_node()
            .filter(|n| n.text_range() == decl.full_range);
        match decl.kind {
            DeclKind::Function | DeclKind::Procedure => {
                let declared = node
                    .as_ref()
                    .is_some_and(|n| n.kind() == SyntaxKind::SUBPROGRAM_DECL)
                    || u.text
                        .get(
                            usize::from(decl.full_range.start())
                                ..usize::from(decl.full_range.end()),
                        )
                        .is_some_and(|t| !t.to_ascii_lowercase().contains(" is"));
                if declared {
                    needs.push((decl, "subprogram"));
                }
            }
            DeclKind::Constant => {
                let text = u
                    .text
                    .get(usize::from(decl.full_range.start())..usize::from(decl.full_range.end()))
                    .unwrap_or("");
                if !text.contains(":=") {
                    needs.push((decl, "constant"));
                }
            }
            DeclKind::Type
                if node.is_some_and(|n| {
                    n.descendants()
                        .any(|d| d.kind() == SyntaxKind::PROTECTED_TYPE_DEF)
                }) =>
            {
                needs.push((decl, "protected type"));
            }
            _ => {}
        }
    }
    if needs.is_empty() || root.is_none() {
        return;
    }
    // A generic package: its body is checked where it is.
    if u.decls.iter().any(|d| {
        matches!(d.kind, DeclKind::Generic { .. } | DeclKind::GenericType)
            && d.parent == Some(package)
    }) {
        return;
    }
    // Projects may hold several bodies of one package (alternatives
    // chosen at compile time): a definition in any of them counts.
    let bodies: Vec<usize> = u
        .analysis
        .units()
        .iter()
        .enumerate()
        .filter(|(_, b)| {
            b.kind == DeclKind::PackageBody
                && b.primary.as_deref() == Some(info.name.as_str())
                && b.library == info.library
        })
        .map(|(index, _)| index)
        .collect();
    let body = bodies.first();
    let mut defined: FxHashMap<(String, &'static str), usize> = FxHashMap::default();
    for &index in &bodies {
        let body_id = DeclId {
            unit: index as u32,
            index: 0,
        };
        for d in u
            .analysis
            .unit_decls(index as u32)
            .iter()
            .skip(1)
            .filter(|d| d.parent == Some(body_id))
        {
            let kind = match d.kind {
                DeclKind::Function | DeclKind::Procedure => "subprogram",
                DeclKind::Constant => "constant",
                DeclKind::ProtectedBody => "protected type",
                _ => continue,
            };
            *defined.entry((d.name.clone(), kind)).or_default() += 1;
        }
    }
    // Subprograms given by a `foreign` attribute have no VHDL body.
    let foreign: FxHashSet<String> = u
        .node
        .descendants()
        .filter(|n| n.kind() == SyntaxKind::ATTRIBUTE_SPEC)
        .filter(|n| {
            n.text()
                .to_string()
                .to_ascii_lowercase()
                .contains("foreign")
        })
        .filter_map(|n| {
            n.descendants_with_tokens()
                .filter_map(|e| e.into_token())
                .filter(|t| t.kind() == SyntaxKind::IDENT)
                .nth(1)
                .map(|t| vfl_syntax::identifier_key(t.text()))
        })
        .collect();
    for (decl, kind) in needs {
        if defined.contains_key(&(decl.name.clone(), kind)) || foreign.contains(&decl.name) {
            continue;
        }
        let message = match (kind, body.is_some()) {
            ("subprogram", true) => format!("the package body does not define `{}`", decl.spelling),
            ("subprogram", false) => format!(
                "`{}` is declared but the package `{}` has no body",
                decl.spelling, info.name
            ),
            ("constant", _) => format!(
                "deferred constant `{}` has no full declaration in a package body",
                decl.spelling
            ),
            _ => format!("protected type `{}` has no body", decl.spelling),
        };
        u.push(out, codes::MISSING_BODY, decl.range, message);
    }
}

// ---- E12: attribute specifications ---------------------------------------

fn attribute_spec(u: &Unit, node: &SyntaxNode, out: &mut Vec<Diagnostic>) {
    let Some(spec) = node
        .children()
        .find(|n| n.kind() == SyntaxKind::ENTITY_SPEC)
    else {
        return;
    };
    let tokens: Vec<_> = spec
        .children_with_tokens()
        .filter_map(|e| e.into_token())
        .filter(|t| !t.kind().is_trivia())
        .collect();
    let Some(colon) = tokens.iter().position(|t| t.kind() == SyntaxKind::COLON) else {
        return;
    };
    let Some(class) = tokens.get(colon + 1) else {
        return;
    };
    let class = class.text().to_ascii_lowercase();
    for name in &tokens[..colon] {
        if !matches!(name.kind(), SyntaxKind::IDENT | SyntaxKind::EXT_IDENT) {
            continue;
        }
        let Some(id) = u.target(name.text_range()) else {
            continue;
        };
        let Some(decl) = u.decl(id) else {
            continue;
        };
        let actual = match decl.kind {
            DeclKind::Entity => "entity",
            DeclKind::Architecture => "architecture",
            DeclKind::Package => "package",
            DeclKind::Configuration => "configuration",
            DeclKind::Component => "component",
            DeclKind::Signal => "signal",
            DeclKind::Port { .. } => "signal",
            DeclKind::Variable => "variable",
            DeclKind::Constant | DeclKind::Generic { .. } => "constant",
            DeclKind::Type | DeclKind::IncompleteType => "type",
            DeclKind::Subtype => "subtype",
            DeclKind::Function => "function",
            DeclKind::Procedure => "procedure",
            DeclKind::EnumLiteral => "literal",
            DeclKind::PhysicalUnit => "units",
            DeclKind::Label => "label",
            DeclKind::File => "file",
            DeclKind::Group => "group",
            _ => continue,
        };
        let ok = actual == class
            || (class == "label" && decl.kind == DeclKind::Label)
            || (class == "constant" && matches!(decl.kind, DeclKind::Generic { .. }));
        if !ok {
            u.push(
                out,
                codes::ATTRIBUTE_SPECIFICATION,
                name.text_range(),
                format!("`{}` is a {actual}, not a {class}", decl.spelling),
            );
            continue;
        }
        // The specification must be in the region that declares the name
        // (or, for a design unit, in it).
        let here = node
            .ancestors()
            .skip(1)
            .find(|n| {
                matches!(
                    n.kind(),
                    SyntaxKind::ENTITY_DECL
                        | SyntaxKind::ARCHITECTURE_BODY
                        | SyntaxKind::PACKAGE_DECL
                        | SyntaxKind::PACKAGE_BODY
                        | SyntaxKind::PROCESS_STMT
                        | SyntaxKind::SUBPROGRAM_BODY
                        | SyntaxKind::BLOCK_STMT
                        | SyntaxKind::GENERATE_BODY
                        | SyntaxKind::COMPONENT_DECL
                )
            })
            .map(|n| n.text_range());
        let declared_in = if super::is_unit_kind(decl.kind) {
            None
        } else {
            decl.parent.and_then(|p| u.decl(p))
        };
        if let (Some(here), Some(parent)) = (here, declared_in)
            && parent.file == u.file
            && id.unit == u.unit
            && !parent.full_range.contains_range(here)
            && !matches!(parent.kind, DeclKind::Entity)
            && !matches!(decl.kind, DeclKind::Port { .. })
        {
            u.push(
                out,
                codes::ATTRIBUTE_SPECIFICATION,
                name.text_range(),
                format!(
                    "the attribute of `{}` must be specified where it is declared",
                    decl.spelling
                ),
            );
        }
    }
}

// ---- E13: constraints ----------------------------------------------------------

fn constraints(u: &Unit, indication: &SyntaxNode, out: &mut Vec<Diagnostic>) {
    // `t(a to b, c to d)`: as many index ranges as the array type has
    // indexes; a range constraint on an array, an index constraint on a
    // scalar.
    let Some(mark) = indication.children().find(|n| {
        matches!(
            n.kind(),
            SyntaxKind::CALL_EXPR | SyntaxKind::NAME | SyntaxKind::SELECTED_NAME
        )
    }) else {
        return;
    };
    let Some((id, decl)) = u.named(&mark) else {
        return;
    };
    if !matches!(decl.kind, DeclKind::Type | DeclKind::Subtype) {
        return;
    }
    let Some(def) = u
        .analysis
        .base_type(id)
        .and_then(|b| u.decl(b))
        .and_then(|b| b.type_def.clone())
    else {
        return;
    };
    let range_constraint = indication.has_child(|k| k == SyntaxKind::RANGE_CONSTRAINT);
    match &*def {
        crate::decl::TypeDef::Array { indexes, .. } => {
            if range_constraint {
                u.push(
                    out,
                    codes::CONSTRAINT,
                    trimmed(indication),
                    format!(
                        "`{}` is an array type: constrain it with an index range in parentheses",
                        mark_text(&mark)
                    ),
                );
            }
            if mark.kind() == SyntaxKind::CALL_EXPR
                && let Some(list) = mark
                    .children()
                    .find(|n| n.kind() == SyntaxKind::ASSOCIATION_LIST)
                && mark
                    .children()
                    .next()
                    .is_some_and(|p| p.kind() != SyntaxKind::CALL_EXPR)
            {
                let count = list
                    .children()
                    .filter(|n| n.kind() == SyntaxKind::ASSOCIATION_ELEMENT)
                    .count();
                // A subtype already constrained cannot be constrained again
                // (bar VHDL-2008 element constraints, not checked here).
                if count != indexes.len() && decl.kind == DeclKind::Type {
                    u.push(
                        out,
                        codes::CONSTRAINT,
                        trimmed(&list),
                        format!(
                            "`{}` has {} index{}, but {count} index range{} given",
                            mark_text(&mark),
                            indexes.len(),
                            if indexes.len() == 1 { "" } else { "es" },
                            if count == 1 { " is" } else { "s are" }
                        ),
                    );
                }
            }
        }
        crate::decl::TypeDef::Enum(_)
        | crate::decl::TypeDef::Integer
        | crate::decl::TypeDef::Real
        | crate::decl::TypeDef::Physical
            if mark.kind() == SyntaxKind::CALL_EXPR =>
        {
            u.push(
                out,
                codes::CONSTRAINT,
                trimmed(&mark),
                format!(
                    "`{}` is a scalar type: constrain it with `range`",
                    mark_text(&mark)
                ),
            );
        }
        _ => {}
    }
}

/// A type mark as written.
fn mark_text(mark: &SyntaxNode) -> String {
    crate::resolver::mark_token(mark).map_or_else(String::new, |t| t.text().to_string())
}

// ---- E44: mode views --------------------------------------------------------------

fn mode_view(u: &Unit, node: &SyntaxNode, out: &mut Vec<Diagnostic>) {
    // `view v of rec is a : in; b : out; end view;`
    let Some(record) = node.children().find(|n| {
        matches!(
            n.kind(),
            SyntaxKind::NAME | SyntaxKind::SELECTED_NAME | SyntaxKind::SUBTYPE_INDICATION
        )
    }) else {
        return;
    };
    let record = if record.kind() == SyntaxKind::SUBTYPE_INDICATION {
        match record.children().next() {
            Some(r) => r,
            None => return,
        }
    } else {
        record
    };
    let Some((id, _)) = u.named(&record) else {
        return;
    };
    let Some(def) = u
        .analysis
        .base_type(id)
        .and_then(|b| u.decl(b))
        .and_then(|b| b.type_def.clone())
    else {
        return;
    };
    let crate::decl::TypeDef::Record(elements) = &*def else {
        return;
    };
    let mut named = FxHashSet::default();
    for element in node
        .children()
        .filter(|n| n.kind() == SyntaxKind::MODE_VIEW_ELEMENT)
    {
        for t in element
            .children()
            .filter(|n| n.kind() == SyntaxKind::IDENT_LIST)
            .flat_map(|l| l.children_with_tokens())
            .filter_map(|e| e.into_token())
            .filter(|t| matches!(t.kind(), SyntaxKind::IDENT | SyntaxKind::EXT_IDENT))
        {
            named.insert(vfl_syntax::identifier_key(t.text()));
        }
        if let Some(mode) = element
            .children_with_tokens()
            .filter_map(|e| e.into_token())
            .find(|t| matches!(t.kind(), SyntaxKind::LINKAGE_KW))
        {
            u.push(
                out,
                codes::MODE_VIEW,
                mode.text_range(),
                "a mode view element cannot have mode `linkage`".into(),
            );
        }
    }
    let missing: Vec<&str> = elements
        .iter()
        .filter(|e| !named.contains(&e.name))
        .map(|e| e.spelling.as_str())
        .collect();
    if !missing.is_empty() {
        u.push(
            out,
            codes::MODE_VIEW,
            trimmed(&record),
            format!(
                "the mode view gives no mode to {}",
                missing
                    .iter()
                    .map(|m| format!("`{m}`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        );
    }
}

// ---- E14: conflicting use clauses -----------------------------------------------

fn conflicting_uses(u: &Unit, out: &mut Vec<Diagnostic>) {
    // Two `use p.all` clauses make two non-overloadable declarations of
    // the same name visible: neither is, and the resolver gives both.
    let mut reported = FxHashSet::default();
    for r in u.refs {
        if r.targets.len() < 2 || r.access == crate::decl::Access::Other {
            continue;
        }
        let mut owners: Vec<(u32, &Decl)> = Vec::new();
        for t in &r.targets {
            let Target::Decl(id) = t else { continue };
            let Some(decl) = u.decl(*id) else { continue };
            // Use-visible: declared in a package, not in this unit.
            let in_package =
                u.analysis.units().get(id.unit as usize).is_some_and(|i| {
                    matches!(i.kind, DeclKind::Package | DeclKind::PackageInstance)
                });
            if decl.kind.is_overloadable() || id.unit == u.unit || !in_package {
                continue;
            }
            if !owners.iter().any(|(unit, _)| *unit == id.unit) {
                owners.push((id.unit, decl));
            }
        }
        let [(a, decl), (b, _), ..] = owners.as_slice() else {
            continue;
        };
        if !reported.insert(decl.name.clone()) {
            continue;
        }
        let name = |unit: u32| u.analysis.units()[unit as usize].name.clone();
        let text = u
            .text
            .get(usize::from(r.range.start())..usize::from(r.range.end()))
            .unwrap_or(&decl.spelling);
        u.push(
            out,
            codes::CONFLICTING_USE,
            r.range,
            format!(
                "`{text}` is not visible: both `{}` and `{}` declare it; select it (`{}.{text}`)",
                name(*a),
                name(*b),
                name(*a),
            ),
        );
    }
}

// ---- E11: configurations ----------------------------------------------------------

fn configuration(u: &Unit, out: &mut Vec<Diagnostic>) {
    let info = &u.analysis.units()[u.unit as usize];
    if info.kind != DeclKind::Configuration {
        return;
    }
    let Some(decl) = u
        .node
        .descendants()
        .find(|n| n.kind() == SyntaxKind::CONFIGURATION_DECL)
    else {
        return;
    };
    let Some(entity) = decl
        .children()
        .find(|n| matches!(n.kind(), SyntaxKind::NAME | SyntaxKind::SELECTED_NAME))
        .and_then(|n| crate::resolver::mark_token(&n))
        .map(|t| t.text().to_ascii_lowercase())
    else {
        return;
    };
    let entity = entity.as_str();
    // The configured entity must be in the configuration's library.
    let entity_unit =
        u.analysis.units().iter().position(|i| {
            i.kind == DeclKind::Entity && i.name == entity && i.library == info.library
        });
    let Some(entity_unit) = entity_unit else {
        if u.analysis
            .units()
            .iter()
            .any(|i| i.kind == DeclKind::Entity && i.name == entity)
        {
            u.push(
                out,
                codes::CONFIGURATION,
                info.name_range,
                format!(
                    "entity `{entity}` is not in library `{}`, the configuration's",
                    info.library
                ),
            );
        }
        return;
    };
    // `for arch` names an architecture of the entity; `for label : comp`
    // names an instance of that architecture.
    let Some(block) = u
        .node
        .descendants()
        .find(|n| n.kind() == SyntaxKind::BLOCK_CONFIGURATION)
    else {
        return;
    };
    let Some(arch_token) = block
        .children()
        .find(|n| matches!(n.kind(), SyntaxKind::NAME | SyntaxKind::SELECTED_NAME))
        .and_then(|n| crate::resolver::mark_token(&n))
    else {
        return;
    };
    let arch = arch_token.text().to_ascii_lowercase();
    let arch_unit = u.analysis.units().iter().position(|i| {
        i.kind == DeclKind::Architecture
            && i.name == arch
            && i.primary.as_deref() == Some(entity)
            && i.library == info.library
    });
    let Some(arch_unit) = arch_unit else {
        u.push(
            out,
            codes::CONFIGURATION,
            arch_token.text_range(),
            format!(
                "entity `{entity}` has no architecture `{}`",
                arch_token.text()
            ),
        );
        return;
    };
    let _ = entity_unit;
    let arch_info = &u.analysis.units()[arch_unit];
    let arch_root = u.analysis.files()[arch_info.file as usize].syntax();
    let Some(arch_node) = arch_root
        .children()
        .filter(|n| n.kind() == SyntaxKind::DESIGN_UNIT)
        .nth(arch_info.index)
    else {
        return;
    };
    // Instances of the architecture, by label, with the unit they name.
    let mut instances: FxHashMap<String, String> = FxHashMap::default();
    for inst in arch_node
        .descendants()
        .filter(|n| n.kind() == SyntaxKind::COMPONENT_INSTANTIATION_STMT)
    {
        let label = inst
            .children()
            .find(|n| n.kind() == SyntaxKind::LABEL)
            .and_then(|l| l.first_token())
            .map(|t| t.text().to_ascii_lowercase());
        let unit = inst
            .children()
            .find(|n| matches!(n.kind(), SyntaxKind::NAME | SyntaxKind::SELECTED_NAME))
            .and_then(|n| crate::resolver::mark_token(&n))
            .map(|t| t.text().to_ascii_lowercase());
        if let (Some(label), Some(unit)) = (label, unit) {
            instances.insert(label, unit);
        }
    }
    for component in block
        .children()
        .filter(|n| n.kind() == SyntaxKind::COMPONENT_CONFIGURATION)
    {
        let tokens: Vec<_> = component
            .children()
            .find(|n| n.kind() == SyntaxKind::IDENT_LIST)
            .map(|l| {
                l.children_with_tokens()
                    .filter_map(|e| e.into_token())
                    .filter(|t| matches!(t.kind(), SyntaxKind::IDENT | SyntaxKind::EXT_IDENT))
                    .collect()
            })
            .unwrap_or_default();
        let colon = tokens.len();
        let comp = component
            .children()
            .find(|n| matches!(n.kind(), SyntaxKind::NAME | SyntaxKind::SELECTED_NAME))
            .and_then(|n| crate::resolver::mark_token(&n));
        let Some(comp) = comp else { continue };
        let comp_name = comp.text().to_ascii_lowercase();
        for label in &tokens[..colon] {
            if !matches!(label.kind(), SyntaxKind::IDENT | SyntaxKind::EXT_IDENT) {
                continue;
            }
            let key = label.text().to_ascii_lowercase();
            match instances.get(&key) {
                None => u.push(
                    out,
                    codes::CONFIGURATION,
                    label.text_range(),
                    format!(
                        "architecture `{}` has no instance labeled `{}`",
                        arch_token.text(),
                        label.text()
                    ),
                ),
                Some(unit) if *unit != comp_name => u.push(
                    out,
                    codes::CONFIGURATION,
                    comp.text_range(),
                    format!(
                        "instance `{}` is of `{unit}`, not `{}`",
                        label.text(),
                        comp.text()
                    ),
                ),
                Some(_) => {}
            }
        }
    }
}

// ---- E15, E6: unit order in a file and circular dependencies ----------------

pub(super) fn file_order(
    analysis: &Analysis,
    file: FileId,
    enabled: Enabled,
    out: &mut Vec<Diagnostic>,
) {
    let units = analysis.units();
    let mine: Vec<usize> = analysis.file_units(file).map(|u| u as usize).collect();
    if enabled(codes::DECLARED_BEFORE) {
        for &i in &mine {
            let info = &units[i];
            let Some(primary) = info.primary.as_deref() else {
                continue;
            };
            let kind = match info.kind {
                DeclKind::Architecture => DeclKind::Entity,
                DeclKind::PackageBody => DeclKind::Package,
                _ => continue,
            };
            // Its primary unit later in the same file, and nowhere else.
            let later = mine.iter().any(|&j| {
                units[j].kind == kind
                    && units[j].name == primary
                    && units[j].library == info.library
                    && units[j].index > info.index
            });
            let elsewhere = units.iter().any(|j| {
                j.kind == kind && j.name == primary && j.library == info.library && j.file != file
            });
            if later && !elsewhere {
                out.push(Diagnostic {
                    code: codes::DECLARED_BEFORE,
                    file,
                    range: info.name_range,
                    message: format!(
                        "`{}` comes before its {} `{primary}` in this file; it must be analyzed after it",
                        info.name,
                        if kind == DeclKind::Entity { "entity" } else { "package" }
                    ),
                    fix: None,
                });
            }
        }
    }
    if enabled(codes::CIRCULAR_DEPENDENCY) {
        // Packages that use each other, through use clauses of their
        // declarations (bodies may depend on anything).
        let package_deps = |unit: usize| -> Vec<usize> {
            let info = &units[unit];
            let root = analysis.files()[info.file as usize].syntax();
            let Some(node) = root
                .children()
                .filter(|n| n.kind() == SyntaxKind::DESIGN_UNIT)
                .nth(info.index)
            else {
                return Vec::new();
            };
            let mut deps = Vec::new();
            for clause in node
                .children()
                .filter(|n| n.kind() == SyntaxKind::CONTEXT_CLAUSE)
                .flat_map(|c| c.children())
                .filter(|n| n.kind() == SyntaxKind::USE_CLAUSE)
            {
                for item in clause.children() {
                    let text = item.text().to_string().to_ascii_lowercase();
                    let mut parts = text.split('.').map(str::trim);
                    let (Some(library), Some(name)) = (parts.next(), parts.next()) else {
                        continue;
                    };
                    let library = if library == "work" {
                        info.library.as_str()
                    } else {
                        library
                    };
                    if let Some(dep) = units.iter().position(|u| {
                        u.kind == DeclKind::Package && u.name == name && u.library == library
                    }) && dep != unit
                        && !deps.contains(&dep)
                    {
                        deps.push(dep);
                    }
                }
            }
            deps
        };
        for &start in &mine {
            if units[start].kind != DeclKind::Package {
                continue;
            }
            // Depth-first search back to `start`.
            let mut stack = vec![(start, vec![start])];
            let mut seen = FxHashSet::default();
            let mut cycle = None;
            while let Some((unit, path)) = stack.pop() {
                for dep in package_deps(unit) {
                    if dep == start {
                        cycle = Some(path.clone());
                        break;
                    }
                    if seen.insert(dep) && path.len() < 32 {
                        let mut p = path.clone();
                        p.push(dep);
                        stack.push((dep, p));
                    }
                }
                if cycle.is_some() {
                    break;
                }
            }
            if let Some(path) = cycle {
                let names: Vec<String> = path
                    .iter()
                    .chain(std::iter::once(&start))
                    .map(|&i| format!("`{}`", units[i].name))
                    .collect();
                out.push(Diagnostic {
                    code: codes::CIRCULAR_DEPENDENCY,
                    file,
                    range: units[start].name_range,
                    message: format!("circular dependency: {}", names.join(" -> ")),
                    fix: None,
                });
            }
        }
    }
}

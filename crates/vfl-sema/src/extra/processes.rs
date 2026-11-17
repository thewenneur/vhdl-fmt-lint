//! Processes and loops that hang or break a simulation, and conditions
//! whose value is known before it runs.

use rustc_hash::FxHashSet;
use vfl_syntax::green_walk::ChildTokens;
use vfl_syntax::{SyntaxKind, SyntaxNode};

use super::{Unit, extra_codes as codes, statements, token, trimmed};
use crate::decl::{Decl, DeclId, DeclKind};
use crate::diagnostic::{Diagnostic, Edit, Fix};
use crate::rtl::static_int;

pub(super) fn check(u: &Unit, out: &mut Vec<Diagnostic>) {
    let missing = u.on(codes::MISSING_WAIT);
    let sensitized = u.on(codes::WAIT_IN_SENSITIZED_PROCESS);
    let list = u.on(codes::SENSITIVITY_LIST_FORM);
    let postponed = u.on(codes::POSTPONED_DELTA);
    let infinite = u.on(codes::INFINITE_LOOP);
    let condition = u.on(codes::STATIC_CONDITION);
    let bounds = u.on(codes::PORT_BOUNDS);
    if !(missing || sensitized || list || postponed || infinite || condition || bounds) {
        return;
    }
    for node in u.node.descendants() {
        match node.kind() {
            SyntaxKind::PROCESS_STMT => {
                if missing || sensitized {
                    waits(u, &node, missing, sensitized, out);
                }
                if list {
                    sensitivity_form(u, &node, out);
                }
                if postponed && node.has_child(|k| k == SyntaxKind::POSTPONED_KW) {
                    postponed_delta(u, &node, out);
                }
            }
            SyntaxKind::LOOP_STMT if infinite => infinite_loop(u, &node, out),
            SyntaxKind::IF_STMT | SyntaxKind::ASSERT_STMT | SyntaxKind::WHILE_SCHEME
                if condition =>
            {
                static_condition(u, &node, out)
            }
            SyntaxKind::PORT_MAP_ASPECT if bounds => port_bounds(u, &node, out),
            _ => {}
        }
    }
}

/// The statements of a process, and the `wait`s among them (not in the
/// subprograms it declares).
fn waits_of(process: &SyntaxNode) -> (Vec<SyntaxNode>, bool) {
    let mut waits = Vec::new();
    let mut calls = false;
    for statement in statements(process) {
        for n in statement.descendants() {
            match n.kind() {
                SyntaxKind::WAIT_STMT => waits.push(n),
                SyntaxKind::PROCEDURE_CALL_STMT => calls = true,
                _ => {}
            }
        }
    }
    (waits, calls)
}

fn waits(
    u: &Unit,
    process: &SyntaxNode,
    missing: bool,
    sensitized: bool,
    out: &mut Vec<Diagnostic>,
) {
    let has_list = process.has_child(|k| k == SyntaxKind::SENSITIVITY_LIST);
    let (waits, calls) = waits_of(process);
    let keyword =
        token(process, SyntaxKind::PROCESS_KW).map_or_else(|| trimmed(process), |t| t.text_range());
    if has_list && sensitized {
        for wait in &waits {
            u.push(
                out,
                codes::WAIT_IN_SENSITIZED_PROCESS,
                trimmed(wait),
                "a process with a sensitivity list cannot contain `wait`".into(),
            );
        }
    }
    // A called procedure may wait.
    if !has_list && waits.is_empty() && !calls && missing {
        u.push(
            out,
            codes::MISSING_WAIT,
            keyword,
            "process without a sensitivity list or a `wait`: it loops forever at time 0".into(),
        );
    }
}

fn sensitivity_form(u: &Unit, process: &SyntaxNode, out: &mut Vec<Diagnostic>) {
    let Some(list) = process
        .children()
        .find(|n| n.kind() == SyntaxKind::SENSITIVITY_LIST)
    else {
        return;
    };
    let names: Vec<SyntaxNode> = list.children().collect();
    if names.is_empty() && !list.has_child(|k| k == SyntaxKind::ALL_KW) {
        u.push(
            out,
            codes::SENSITIVITY_LIST_FORM,
            trimmed(&list),
            "empty sensitivity list: the process runs once and never again".into(),
        );
        return;
    }
    let mut seen: FxHashSet<String> = FxHashSet::default();
    for name in names {
        let key = name
            .text()
            .to_string()
            .to_ascii_lowercase()
            .replace(char::is_whitespace, "");
        if !seen.insert(key) {
            let range = trimmed(&name);
            let removal = crate::diagnostic::list_item_removal(u.text, range);
            let message = format!(
                "`{}` is named twice in the sensitivity list",
                name.text().to_string().trim()
            );
            match removal {
                Some(r) => u.push_fix(
                    out,
                    codes::SENSITIVITY_LIST_FORM,
                    range,
                    message,
                    Fix {
                        title: "Remove the duplicate".into(),
                        edits: vec![Edit {
                            range: r,
                            text: String::new(),
                        }],
                        safe: true,
                    },
                ),
                None => u.push(out, codes::SENSITIVITY_LIST_FORM, range, message),
            }
        }
    }
}

fn postponed_delta(u: &Unit, process: &SyntaxNode, out: &mut Vec<Diagnostic>) {
    for statement in statements(process) {
        for n in statement
            .descendants_with_tokens()
            .filter_map(|e| e.into_node())
        {
            if n.kind() != SyntaxKind::SIGNAL_ASSIGN_STMT {
                continue;
            }
            let delayed = n
                .descendants_with_tokens()
                .any(|e| e.kind() == SyntaxKind::AFTER_KW);
            if !delayed {
                u.push(
                    out,
                    codes::POSTPONED_DELTA,
                    trimmed(&n),
                    "a postponed process cannot assign a signal without a delay: it would start a delta cycle".into(),
                );
            }
        }
    }
}

fn infinite_loop(u: &Unit, node: &SyntaxNode, out: &mut Vec<Diagnostic>) {
    let keyword =
        token(node, SyntaxKind::LOOP_KW).map_or_else(|| trimmed(node), |t| t.text_range());
    let body: Vec<SyntaxNode> = node
        .children_with_tokens()
        .skip_while(|e| e.kind() != SyntaxKind::LOOP_KW)
        .filter_map(|e| e.into_node())
        .collect();
    // `while` conditions may call impure functions; `for` loops end.
    if node.has_child(|k| matches!(k, SyntaxKind::WHILE_SCHEME | SyntaxKind::FOR_SCHEME)) {
        return;
    }
    if body.is_empty() {
        u.push(out, codes::INFINITE_LOOP, keyword, "empty loop".into());
        return;
    }
    let label = node
        .children()
        .find(|n| n.kind() == SyntaxKind::LABEL)
        .and_then(|l| l.first_token())
        .map(|t| t.text().to_ascii_lowercase());
    let leaves = body
        .iter()
        .flat_map(|b| b.descendants())
        .any(|n| match n.kind() {
            SyntaxKind::WAIT_STMT | SyntaxKind::RETURN_STMT | SyntaxKind::PROCEDURE_CALL_STMT => {
                true
            }
            SyntaxKind::EXIT_STMT => {
                // An unlabeled `exit` leaves the innermost loop: this one only
                // when no loop is in between.
                let target = n
                    .children_with_tokens()
                    .filter_map(|e| e.into_token())
                    .find(|t| matches!(t.kind(), SyntaxKind::IDENT | SyntaxKind::EXT_IDENT))
                    .map(|t| t.text().to_ascii_lowercase());
                match target {
                    Some(t) => Some(t) == label,
                    None => {
                        n.ancestors()
                            .skip(1)
                            .find(|a| a.kind() == SyntaxKind::LOOP_STMT)
                            .as_ref()
                            == Some(node)
                    }
                }
            }
            _ => false,
        });
    if !leaves {
        u.push(
            out,
            codes::INFINITE_LOOP,
            keyword,
            "this loop has no `exit`, `return` or `wait`: it never ends, and simulation hangs"
                .into(),
        );
    }
}

/// The value of a static integer expression of constants (not generics,
/// whose value depends on the instance).
fn constant_int(u: &Unit, expr: &SyntaxNode) -> Option<i64> {
    static_int(expr, &|name| {
        let (_, decl) = u.named(name)?;
        (decl.kind == DeclKind::Constant)
            .then_some(decl.value)
            .flatten()
    })
}

fn static_condition(u: &Unit, node: &SyntaxNode, out: &mut Vec<Diagnostic>) {
    let conditions: Vec<SyntaxNode> = match node.kind() {
        SyntaxKind::IF_STMT => {
            // The node after each `if` and `elsif`.
            let mut conds = Vec::new();
            let mut take = false;
            for e in node.children_with_tokens() {
                match e {
                    rowan::NodeOrToken::Token(t) if !t.kind().is_trivia() => {
                        take = matches!(t.kind(), SyntaxKind::IF_KW | SyntaxKind::ELSIF_KW)
                    }
                    rowan::NodeOrToken::Node(n) if take => {
                        conds.push(n);
                        take = false;
                    }
                    _ => {}
                }
            }
            conds
        }
        _ => node.children().next().into_iter().collect(),
    };
    for cond in conditions {
        if cond.kind() != SyntaxKind::BINARY_EXPR {
            continue;
        }
        let Some(op) = cond.child_token(|k| !k.is_trivia()).map(|t| t.kind()) else {
            continue;
        };
        let mut sides = cond.children();
        let (Some(l), Some(r)) = (sides.next(), sides.next()) else {
            continue;
        };
        // Literal-only comparisons are `sema/constant-condition`'s or
        // deliberate; one side at least is a named constant.
        let named = |n: &SyntaxNode| {
            n.descendants()
                .any(|d| matches!(d.kind(), SyntaxKind::NAME | SyntaxKind::SELECTED_NAME))
        };
        if !named(&l) && !named(&r) {
            continue;
        }
        // Constants of this unit only: testing those of a package (or
        // generics) is how configuration switches work.
        let foreign = [&l, &r].into_iter().any(|side| {
            std::iter::once((*side).clone())
                .chain(side.descendants())
                .filter(|n| n.kind() == SyntaxKind::NAME)
                .any(|n| u.named(&n).is_some_and(|(id, _)| id.unit != u.unit))
        });
        if foreign {
            continue;
        }
        let (Some(a), Some(b)) = (constant_int(u, &l), constant_int(u, &r)) else {
            continue;
        };
        let value = match op {
            SyntaxKind::EQ => a == b,
            SyntaxKind::NEQ => a != b,
            SyntaxKind::LT => a < b,
            SyntaxKind::LTE => a <= b,
            SyntaxKind::GT => a > b,
            SyntaxKind::GTE => a >= b,
            _ => continue,
        };
        let message = if node.kind() == SyntaxKind::ASSERT_STMT {
            if value {
                "this assertion is always true: it never fires".to_string()
            } else {
                "this assertion is always false: it always fires".to_string()
            }
        } else {
            format!(
                "this condition is always {}: its constants make it `{} {} {}`",
                value,
                a,
                cond.child_token(|k| !k.is_trivia())
                    .map_or_else(String::new, |t| t.text().to_string()),
                b
            )
        };
        u.push(out, codes::STATIC_CONDITION, trimmed(&cond), message);
    }
}

/// The static bounds of an integer object's subtype: its range
/// constraint, its subtype's, `natural`, `positive`.
pub(super) fn int_bounds(u: &Unit, decl: &Decl) -> Option<(i64, i64)> {
    let file = u.analysis.files().get(decl.file as usize)?;
    let root = file.syntax();
    let node = crate::rtl::cases::declaration_node(&root, decl.range)?;
    let indication = node
        .children()
        .find(|n| n.kind() == SyntaxKind::SUBTYPE_INDICATION)?;
    indication_bounds(u, &indication, decl.subtype?, 0)
}

fn indication_bounds(
    u: &Unit,
    indication: &SyntaxNode,
    mark: DeclId,
    depth: u32,
) -> Option<(i64, i64)> {
    if depth > 8 {
        return None;
    }
    if let Some(range) = indication
        .children()
        .find(|n| n.kind() == SyntaxKind::RANGE_CONSTRAINT)
        .and_then(|c| c.children().find(|n| n.kind() == SyntaxKind::RANGE_EXPR))
    {
        let mut b = range.children();
        let left = constant_int(u, &b.next()?)?;
        let right = constant_int(u, &b.next()?)?;
        return Some(if range.has_child(|k| k == SyntaxKind::TO_KW) {
            (left, right)
        } else {
            (right, left)
        });
    }
    if u.is_standard(mark, &["natural"]) {
        return Some((0, i32::MAX as i64));
    }
    if u.is_standard(mark, &["positive"]) {
        return Some((1, i32::MAX as i64));
    }
    if u.is_standard(mark, &["integer"]) {
        return Some((i32::MIN as i64, i32::MAX as i64));
    }
    let decl = u.decl(mark)?;
    if decl.kind != DeclKind::Subtype {
        return None;
    }
    let file = u.analysis.files().get(decl.file as usize)?;
    let node = crate::rtl::cases::declaration_node(&file.syntax(), decl.range)?;
    let inner = node
        .children()
        .find(|n| n.kind() == SyntaxKind::SUBTYPE_INDICATION)?;
    indication_bounds(u, &inner, decl.subtype?, depth + 1)
}

fn port_bounds(u: &Unit, map: &SyntaxNode, out: &mut Vec<Diagnostic>) {
    let Some(list) = map
        .children()
        .find(|n| n.kind() == SyntaxKind::ASSOCIATION_LIST)
    else {
        return;
    };
    let interface = super::legality::map_interface(u, map);
    for (position, element) in list
        .children()
        .filter(|n| n.kind() == SyntaxKind::ASSOCIATION_ELEMENT)
        .enumerate()
    {
        let named = element.has_child(|k| k == SyntaxKind::ARROW);
        let mut parts = element.children();
        let formal = if named {
            parts.next().and_then(|f| u.named(&f)).map(|(_, d)| d)
        } else {
            interface
                .as_ref()
                .and_then(|i| i.get(position))
                .and_then(|&id| u.decl(id))
        };
        let actual = parts.next();
        let (Some(formal), Some(actual)) = (formal, actual) else {
            continue;
        };
        if actual.kind() != SyntaxKind::NAME {
            continue;
        }
        let Some((_, actual_decl)) = u.named(&actual) else {
            continue;
        };
        if !actual_decl.kind.is_signal() {
            continue;
        }
        let (Some(f), Some(a)) = (int_bounds(u, formal), int_bounds(u, actual_decl)) else {
            continue;
        };
        let DeclKind::Port { mode, .. } = formal.kind else {
            continue;
        };
        // Values flow from the actual into an input, out of an output.
        let (from, to, what) = if mode == crate::decl::Mode::In {
            (a, f, "port")
        } else {
            (f, a, "signal")
        };
        if from.0 < to.0 || from.1 > to.1 {
            u.push(
                out,
                codes::PORT_BOUNDS,
                trimmed(&actual),
                format!(
                    "`{}` ({} to {}) can take values outside the range of the {what} it is associated with ({} to {})",
                    if mode == crate::decl::Mode::In { &actual_decl.spelling } else { &formal.spelling },
                    from.0,
                    from.1,
                    to.0,
                    to.1
                ),
            );
        }
    }
}

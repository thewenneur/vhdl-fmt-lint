//! Declarations and associations: hidden names, associations out of
//! order or left to defaults, integers without a range, `others` choices
//! nothing reaches, calls two homographs make ambiguous.

use rustc_hash::{FxHashMap, FxHashSet};
use vfl_syntax::green_walk::ChildTokens;
use vfl_syntax::{SyntaxKind, SyntaxNode};

use super::{Unit, extra_codes as codes, trimmed};
use crate::decl::{Decl, DeclId, DeclKind, Mode, Target, TypeDef};
use crate::diagnostic::Diagnostic;

pub(super) fn check(u: &Unit, out: &mut Vec<Diagnostic>) {
    if u.on(codes::HIDDEN_DECLARATION) {
        hidden(u, out);
    }
    if u.on(codes::UNCONSTRAINED_INTEGER) {
        unconstrained_integers(u, out);
    }
    if u.on(codes::AMBIGUOUS_CALL) {
        ambiguous_calls(u, out);
    }
    let order = u.on(codes::ASSOCIATION_ORDER);
    let defaults = u.on(codes::DEFAULT_ASSOCIATION);
    let others = u.on(codes::UNNEEDED_OTHERS);
    if !(order || defaults || others) {
        return;
    }
    for node in u.node.descendants() {
        match node.kind() {
            SyntaxKind::PORT_MAP_ASPECT | SyntaxKind::GENERIC_MAP_ASPECT => {
                if order {
                    association_order(u, &node, out);
                }
                if defaults && node.kind() == SyntaxKind::PORT_MAP_ASPECT {
                    default_associations(u, &node, out);
                }
            }
            SyntaxKind::CASE_STMT if others => unneeded_others(u, &node, out),
            _ => {}
        }
    }
}

// ---- E24: hidden declarations ------------------------------------------------

fn is_hiding_kind(kind: DeclKind) -> bool {
    matches!(
        kind,
        DeclKind::Signal
            | DeclKind::Variable
            | DeclKind::Constant
            | DeclKind::Port { .. }
            | DeclKind::Generic { .. }
            | DeclKind::GenerateParameter
            | DeclKind::Alias
            | DeclKind::File
    )
}

fn hidden(u: &Unit, out: &mut Vec<Diagnostic>) {
    // The objects of each region of the unit, by name.
    let mut regions: FxHashMap<Option<DeclId>, FxHashMap<&str, &Decl>> = FxHashMap::default();
    for d in u.decls.iter().skip(1) {
        if is_hiding_kind(d.kind) {
            regions
                .entry(d.parent)
                .or_default()
                .entry(d.name.as_str())
                .or_insert(d);
        }
    }
    // An architecture also sees its entity's ports and generics.
    let info = &u.analysis.units()[u.unit as usize];
    let mut outer: FxHashMap<&str, &Decl> = FxHashMap::default();
    if info.kind == DeclKind::Architecture
        && let Some(entity) = info.primary.as_deref()
        && let Some(index) = u.analysis.units().iter().position(|i| {
            i.kind == DeclKind::Entity && i.name == entity && i.library == info.library
        })
    {
        for d in u.analysis.unit_decls(index as u32).iter().skip(1) {
            if is_hiding_kind(d.kind) {
                outer.entry(d.name.as_str()).or_insert(d);
            }
        }
    }
    let root = DeclId {
        unit: u.unit,
        index: 0,
    };
    for d in u.decls.iter().skip(1) {
        // Loop and subprogram parameters are local by nature, and a
        // component repeats its entity's ports.
        if !is_hiding_kind(d.kind)
            || d.parent
                .and_then(|p| u.decl(p))
                .is_some_and(|p| p.kind == DeclKind::Component)
        {
            continue;
        }
        // The regions around the one that declares `d`.
        let mut found = None;
        let mut region = d.parent;
        for _ in 0..64 {
            let Some(r) = region else { break };
            let Some(owner) = u.decl(r) else { break };
            region = owner.parent;
            if let Some(o) = regions.get(&region).and_then(|m| m.get(d.name.as_str())) {
                found = Some(*o);
                break;
            }
        }
        if found.is_none() && d.parent.is_some_and(|p| p != root) {
            found = outer.get(d.name.as_str()).copied();
        }
        let Some(o) = found else { continue };
        if o.full_range == d.full_range {
            continue;
        }
        u.push(
            out,
            codes::HIDDEN_DECLARATION,
            d.range,
            format!(
                "{} `{}` hides the {} of the same name declared at line {}",
                d.kind.describe(),
                d.spelling,
                o.kind.describe(),
                u.analysis.files().get(o.file as usize).map_or(0, |f| f
                    .line_index
                    .line_col(&f.text, o.range.start())
                    .line
                    + 1)
            ),
        );
    }
}

// ---- E36: integers without a range ----------------------------------------------

fn unconstrained_integers(u: &Unit, out: &mut Vec<Diagnostic>) {
    let info = &u.analysis.units()[u.unit as usize];
    let entity_has_ports = |entity: &str| {
        u.analysis
            .units()
            .iter()
            .position(|i| {
                i.kind == DeclKind::Entity && i.name == entity && i.library == info.library
            })
            .is_some_and(|index| {
                u.analysis
                    .unit_decls(index as u32)
                    .iter()
                    .any(|d| matches!(d.kind, DeclKind::Port { .. }))
            })
    };
    let rtl = match info.kind {
        DeclKind::Entity => true,
        DeclKind::Architecture => info.primary.as_deref().is_some_and(entity_has_ports),
        _ => false,
    };
    if !rtl {
        return;
    }
    let root = u.node.clone();
    for d in u.decls.iter().skip(1) {
        if !matches!(d.kind, DeclKind::Signal | DeclKind::Port { .. }) {
            continue;
        }
        let Some(mark) = d.subtype else { continue };
        if !u.is_standard(mark, &["integer", "natural", "positive"]) {
            continue;
        }
        let Some(node) = crate::rtl::cases::declaration_node(&root, d.range) else {
            continue;
        };
        let Some(indication) = node
            .children()
            .find(|n| n.kind() == SyntaxKind::SUBTYPE_INDICATION)
        else {
            continue;
        };
        if indication.children().count() != 1 {
            continue;
        }
        let kind = if d.kind.is_signal() && matches!(d.kind, DeclKind::Port { .. }) {
            "port"
        } else {
            "signal"
        };
        u.push(
            out,
            codes::UNCONSTRAINED_INTEGER,
            d.range,
            format!(
                "{kind} `{}` is an integer without a range: synthesis makes it 32 bits; give it a range",
                d.spelling
            ),
        );
    }
}

// ---- E9: ambiguous calls ------------------------------------------------------------

fn ambiguous_calls(u: &Unit, out: &mut Vec<Diagnostic>) {
    // Two homographs made visible by two use clauses: the same name and
    // the same parameter and result types, from different packages.
    for r in u.refs {
        if r.targets.len() < 2 || !r.access.reads() {
            continue;
        }
        let decls: Vec<(DeclId, &Decl)> = r
            .targets
            .iter()
            .filter_map(|t| match t {
                Target::Decl(id) => u.decl(*id).map(|d| (*id, d)),
                Target::Library(_) => None,
            })
            .filter(|(_, d)| matches!(d.kind, DeclKind::Function | DeclKind::Procedure))
            .collect();
        if decls.len() < 2 {
            continue;
        }
        let signature = |d: &Decl| -> Option<Vec<Option<DeclId>>> {
            let mut sig: Vec<Option<DeclId>> = d
                .params
                .as_ref()?
                .iter()
                .map(|p| u.decl(*p).and_then(|p| u.analysis.base_type(p.subtype?)))
                .collect();
            sig.push(d.subtype.and_then(|s| u.analysis.base_type(s)));
            if sig.iter().any(Option::is_none) {
                return None;
            }
            Some(sig)
        };
        // The package a subprogram is declared in (a body counts as its
        // package); local declarations hide, they are not ambiguous.
        let package = |id: DeclId| -> Option<(&str, &str)> {
            let info = u.analysis.units().get(id.unit as usize)?;
            match info.kind {
                DeclKind::Package => Some((info.library.as_str(), info.name.as_str())),
                DeclKind::PackageBody => Some((info.library.as_str(), info.primary.as_deref()?)),
                _ => None,
            }
        };
        // Inside a package or its body, its own declarations are
        // directly visible and hide the others.
        let here = u.analysis.units().get(u.unit as usize).map(|info| {
            let name = match info.kind {
                DeclKind::PackageBody => info.primary.as_deref().unwrap_or(""),
                _ => info.name.as_str(),
            };
            (info.library.as_str(), name)
        });
        if decls
            .iter()
            .any(|(id, _)| package(*id).is_none() || package(*id) == here)
        {
            continue;
        }
        let mut seen: FxHashMap<Vec<Option<DeclId>>, DeclId> = FxHashMap::default();
        for (id, d) in &decls {
            let Some(sig) = signature(d) else { continue };
            if let Some(&other) = seen.get(&sig)
                && package(other) != package(*id)
            {
                let name =
                    |id: DeclId| package(id).map_or_else(String::new, |(_, n)| n.to_string());
                u.push(
                    out,
                    codes::AMBIGUOUS_CALL,
                    r.range,
                    format!(
                        "`{}` is ambiguous: `{}` and `{}` declare it with the same parameter and result types",
                        d.spelling,
                        name(other),
                        name(*id)
                    ),
                );
                break;
            }
            seen.insert(sig, *id);
        }
    }
}

// ---- E28: inputs left to their default -------------------------------------------

fn default_associations(u: &Unit, map: &SyntaxNode, out: &mut Vec<Diagnostic>) {
    let Some(interface) = super::legality::map_interface(u, map) else {
        return;
    };
    let Some(list) = map
        .children()
        .find(|n| n.kind() == SyntaxKind::ASSOCIATION_LIST)
    else {
        return;
    };
    let mut associated: FxHashSet<DeclId> = FxHashSet::default();
    let mut open: FxHashSet<DeclId> = FxHashSet::default();
    for (position, element) in list
        .children()
        .filter(|n| n.kind() == SyntaxKind::ASSOCIATION_ELEMENT)
        .enumerate()
    {
        let formal = if element.has_child(|k| k == SyntaxKind::ARROW) {
            element
                .children()
                .next()
                .and_then(|f| u.named(&f))
                .map(|(id, _)| id)
        } else {
            interface.get(position).copied()
        };
        let Some(formal) = formal else { continue };
        // `p(0) => x` associates part of `p`.
        let target = u
            .decl(formal)
            .filter(|d| matches!(d.kind, DeclKind::Port { .. }))
            .map(|_| formal);
        let Some(target) = target else { continue };
        if element.has_child(|k| k == SyntaxKind::OPEN_KW) && element.children().nth(1).is_none() {
            open.insert(target);
        } else {
            associated.insert(target);
        }
    }
    for id in interface {
        let Some(d) = u.decl(id) else { continue };
        if !matches!(
            d.kind,
            DeclKind::Port {
                mode: Mode::In,
                has_default: true
            }
        ) || associated.contains(&id)
        {
            continue;
        }
        u.push(
            out,
            codes::DEFAULT_ASSOCIATION,
            trimmed(map),
            format!(
                "input port `{}` is {} and takes its default value",
                d.spelling,
                if open.contains(&id) {
                    "`open`"
                } else {
                    "not associated"
                }
            ),
        );
    }
}

// ---- E38: unneeded `others` --------------------------------------------------------

fn unneeded_others(u: &Unit, case: &SyntaxNode, out: &mut Vec<Diagnostic>) {
    let Some(selector) = case.children().next() else {
        return;
    };
    if !matches!(
        selector.kind(),
        SyntaxKind::NAME | SyntaxKind::SELECTED_NAME
    ) {
        return;
    }
    let Some((id, decl)) = u.named(&selector) else {
        return;
    };
    if !decl.kind.is_object() {
        return;
    }
    let Some(def) = u
        .analysis
        .base_type(decl.subtype.unwrap_or(id))
        .and_then(|b| u.decl(b))
        .and_then(|b| b.type_def.clone())
    else {
        return;
    };
    let TypeDef::Enum(literals) = &*def else {
        return;
    };
    // Only a whole-type selector: a subtype may have fewer values.
    if decl
        .subtype
        .and_then(|s| u.decl(s))
        .is_some_and(|s| s.kind == DeclKind::Subtype)
    {
        return;
    }
    let mut covered: FxHashSet<String> = FxHashSet::default();
    let mut others = None;
    for alternative in case
        .children()
        .filter(|n| n.kind() == SyntaxKind::CASE_ALTERNATIVE)
    {
        let Some(choices) = alternative
            .children()
            .find(|n| n.kind() == SyntaxKind::CHOICES)
        else {
            continue;
        };
        for e in choices.children_with_tokens() {
            match e {
                rowan::NodeOrToken::Token(t) if t.kind() == SyntaxKind::OTHERS_KW => {
                    others = Some(t.text_range())
                }
                rowan::NodeOrToken::Node(n) => {
                    let key = match n.kind() {
                        SyntaxKind::NAME => n.first_token().map(|t| t.text().to_ascii_lowercase()),
                        SyntaxKind::LITERAL => n.first_token().map(|t| t.text().to_string()),
                        // A range of values: give up.
                        _ => return,
                    };
                    if let Some(k) = key {
                        covered.insert(k);
                    }
                }
                _ => {}
            }
        }
    }
    if let Some(range) = others
        && literals.iter().all(|l| covered.contains(l))
    {
        u.push(
            out,
            codes::UNNEEDED_OTHERS,
            range,
            "the other choices already cover every value: `when others` is never taken".into(),
        );
    }
}

// ---- E27: associations in another order than the declarations -------------

fn association_order(u: &Unit, map: &SyntaxNode, out: &mut Vec<Diagnostic>) {
    let Some(interface) = super::legality::map_interface(u, map) else {
        return;
    };
    let Some(list) = map
        .children()
        .find(|n| n.kind() == SyntaxKind::ASSOCIATION_LIST)
    else {
        return;
    };
    let mut last: Option<(usize, String)> = None;
    for element in list
        .children()
        .filter(|n| n.kind() == SyntaxKind::ASSOCIATION_ELEMENT)
    {
        if !element.has_child(|k| k == SyntaxKind::ARROW) {
            continue;
        }
        let Some(formal) = element.children().next() else {
            continue;
        };
        let Some((id, d)) = u.named(&formal) else {
            continue;
        };
        let Some(position) = interface.iter().position(|&i| i == id) else {
            continue;
        };
        if let Some((before, name)) = &last
            && position < *before
        {
            u.push(
                out,
                codes::ASSOCIATION_ORDER,
                trimmed(&element),
                format!(
                    "`{}` is associated after `{name}` but declared before it",
                    d.spelling
                ),
            );
            return;
        }
        if last.as_ref().is_none_or(|(p, _)| position > *p) {
            last = Some((position, d.spelling.clone()));
        }
    }
}

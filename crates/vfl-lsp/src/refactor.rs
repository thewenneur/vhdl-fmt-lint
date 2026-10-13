//! What rename and find references work on: the declarations a name
//! stands for, with the ones that are the same thing (a package and its
//! body, a subprogram and its body, an entity and the components named
//! after it), or a record element; where they are named; and whether a
//! new name would clash with another.

use std::collections::{HashMap, HashSet};

use vfl_sema::{Access, Decl, DeclId, DeclKind, FileId, Target, TypeDef};
use vfl_syntax::{SyntaxKind, TextRange, TextSize, identifier_key};

use crate::handlers::{Hit, hit_at};
use crate::world::World;

/// What a name at a position stands for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Symbol {
    /// Declarations, with every declaration that is the same thing.
    Decls(Vec<DeclId>),
    /// Element `name` (its key) of record type `record`.
    Element { record: DeclId, name: String },
}

/// The name at `offset` and what it stands for.
pub(crate) fn symbol_at(
    world: &World,
    file: FileId,
    offset: TextSize,
) -> Option<(TextRange, Symbol)> {
    let contains = |r: TextRange| r.start() <= offset && offset <= r.end();
    let units: Vec<_> = world.analysis.file_units(file).collect();
    for e in units
        .iter()
        .flat_map(|&u| world.analysis.unit_element_references(u))
    {
        if contains(e.range) {
            return Some((
                e.range,
                Symbol::Element {
                    record: e.record,
                    name: e.name.clone(),
                },
            ));
        }
    }
    if let Some(found) = element_decl_at(world, file, offset) {
        return Some(found);
    }
    let Hit { range, targets, .. } = hit_at(world, file, offset)?;
    if targets.is_empty() {
        return None;
    }
    Some((range, Symbol::Decls(linked(world, &targets))))
}

/// A record element named where it is declared.
fn element_decl_at(world: &World, file: FileId, offset: TextSize) -> Option<(TextRange, Symbol)> {
    let root = world.analysis.files().get(file as usize)?.syntax();
    let token = root
        .token_at_offset(offset)
        .find(|t| matches!(t.kind(), SyntaxKind::IDENT | SyntaxKind::EXT_IDENT))?;
    let element = token
        .parent_ancestors()
        .find(|n| n.kind() == SyntaxKind::ELEMENT_DECL)?;
    // The element's names come before its colon.
    let colon = element
        .children_with_tokens()
        .filter_map(|e| e.into_token())
        .find(|t| t.kind() == SyntaxKind::COLON)?;
    if token.text_range().start() > colon.text_range().start() {
        return None;
    }
    let type_decl = element
        .ancestors()
        .find(|n| n.kind() == SyntaxKind::TYPE_DECL)?;
    let record = world
        .analysis
        .file_units(file)
        .flat_map(|u| {
            world
                .analysis
                .unit_decls(u)
                .iter()
                .enumerate()
                .map(move |(i, d)| {
                    (
                        DeclId {
                            unit: u,
                            index: i as u32,
                        },
                        d,
                    )
                })
        })
        .find_map(|(id, d)| {
            (d.file == file
                && d.full_range == type_decl.text_range()
                && matches!(d.type_def.as_deref(), Some(TypeDef::Record(_))))
            .then_some(id)
        })?;
    Some((
        token.text_range(),
        Symbol::Element {
            record,
            name: identifier_key(token.text()),
        },
    ))
}

/// `targets` and every declaration that is the same thing: a package and
/// its body, a protected type and its body, a deferred constant and its
/// full declaration, a subprogram and its body (each body's name refers to
/// what it completes), and an entity and the components named after it,
/// with their generics and ports of the same names.
pub(crate) fn linked(world: &World, targets: &[DeclId]) -> Vec<DeclId> {
    let analysis = &world.analysis;
    // Everything linked shares a name with a target.
    let names: HashSet<&str> = targets
        .iter()
        .filter_map(|&id| analysis.decl(id))
        .map(|d| d.name.as_str())
        .collect();
    let candidates: Vec<(DeclId, &Decl)> = user_decls(world)
        .filter(|(_, d)| names.contains(d.name.as_str()))
        .collect();
    // A body's name refers to what it completes.
    let mut edges: HashMap<DeclId, Vec<DeclId>> = HashMap::new();
    for &(id, decl) in &candidates {
        let Some(reference) = crate::types::reference_at(analysis, decl.file, decl.range) else {
            continue;
        };
        for target in &reference.targets {
            if let Target::Decl(t) = target
                && *t != id
            {
                edges.entry(id).or_default().push(*t);
                edges.entry(*t).or_default().push(id);
            }
        }
    }
    let is_unit = |d: &Decl| matches!(d.kind, DeclKind::Entity | DeclKind::Component);
    let is_interface =
        |d: &Decl| matches!(d.kind, DeclKind::Port { .. } | DeclKind::Generic { .. });
    let mut out: Vec<DeclId> = Vec::new();
    let mut seen: HashSet<DeclId> = HashSet::new();
    let mut queue: Vec<DeclId> = targets.to_vec();
    while let Some(id) = queue.pop() {
        if !seen.insert(id) {
            continue;
        }
        out.push(id);
        queue.extend(edges.get(&id).into_iter().flatten().copied());
        let Some(decl) = analysis.decl(id) else {
            continue;
        };
        if is_unit(decl) {
            // An entity and the components named after it.
            queue.extend(
                candidates
                    .iter()
                    .filter(|(_, d)| is_unit(d) && d.name == decl.name)
                    .map(|(id, _)| *id),
            );
        } else if is_interface(decl)
            && let Some(parent) = decl.parent.and_then(|p| analysis.decl(p))
            && is_unit(parent)
        {
            // Their generics and ports of the same name.
            queue.extend(candidates.iter().filter_map(|&(c, d)| {
                let owner = analysis.decl(d.parent?)?;
                (is_interface(d) && is_unit(owner) && owner.name == parent.name).then_some(c)
            }));
        }
    }
    out.sort();
    out
}

/// The declarations of the project's own files.
fn user_decls(world: &World) -> impl Iterator<Item = (DeclId, &Decl)> {
    let analysis = &world.analysis;
    (0..analysis.units().len() as vfl_sema::UnitId)
        .filter(|&u| !analysis.files()[analysis.units()[u as usize].file as usize].builtin)
        .flat_map(move |unit| {
            analysis
                .unit_decls(unit)
                .iter()
                .enumerate()
                .map(move |(index, decl)| {
                    (
                        DeclId {
                            unit,
                            index: index as u32,
                        },
                        decl,
                    )
                })
        })
}

/// Every place that names `symbol`, declarations included, once each, in
/// file order.
pub(crate) fn occurrences(
    world: &World,
    symbol: &Symbol,
    declarations: bool,
) -> Vec<(FileId, TextRange, Access)> {
    match symbol {
        Symbol::Decls(ids) => crate::handlers::occurrences(world, ids, declarations),
        Symbol::Element { record, name } => {
            let mut out: Vec<(FileId, TextRange, Access)> = Vec::new();
            if declarations {
                out.extend(
                    element_decls(world, *record, name)
                        .into_iter()
                        .map(|(f, r)| (f, r, Access::Other)),
                );
            }
            for (file, e) in world.analysis.element_references() {
                if e.record == *record && e.name == *name {
                    out.push((file, e.range, Access::Other));
                }
            }
            out.sort_by_key(|(f, r, _)| (*f, r.start(), r.end()));
            out.dedup_by_key(|(f, r, _)| (*f, *r));
            out
        }
    }
}

/// Where element `name` of `record` is declared.
pub(crate) fn element_decls(world: &World, record: DeclId, name: &str) -> Vec<(FileId, TextRange)> {
    let Some(decl) = world.analysis.decl(record) else {
        return Vec::new();
    };
    let root = world.analysis.files()[decl.file as usize].syntax();
    let Some(node) = root
        .covering_element(decl.full_range)
        .into_node()
        .or_else(|| root.covering_element(decl.full_range).parent())
    else {
        return Vec::new();
    };
    node.descendants()
        .filter(|n| n.kind() == SyntaxKind::ELEMENT_DECL)
        .flat_map(|element| {
            element
                .descendants_with_tokens()
                .filter_map(|e| e.into_token())
                .take_while(|t| t.kind() != SyntaxKind::COLON)
                .filter(|t| matches!(t.kind(), SyntaxKind::IDENT | SyntaxKind::EXT_IDENT))
                .filter(|t| identifier_key(t.text()) == name)
                .map(|t| (decl.file, t.text_range()))
                .collect::<Vec<_>>()
        })
        .collect()
}

/// Why renaming `symbol` to `new_name` would change what a name refers
/// to, if it would: another declaration of that name in the same place,
/// or one that a renamed use would then see instead, or a use of another
/// declaration that the renamed one would then hide.
pub(crate) fn rename_conflict(world: &World, symbol: &Symbol, new_name: &str) -> Option<String> {
    let analysis = &world.analysis;
    let key = identifier_key(new_name);
    let ids = match symbol {
        Symbol::Element { record, name } => {
            if *name == key {
                return None;
            }
            let Some(TypeDef::Record(elements)) =
                analysis.decl(*record).and_then(|d| d.type_def.as_deref())
            else {
                return None;
            };
            return elements.iter().find(|e| e.name == key).map(|e| {
                format!(
                    "`{}` already has an element named `{}`",
                    analysis.decl(*record).map_or("", |d| &d.spelling),
                    e.spelling
                )
            });
        }
        Symbol::Decls(ids) => ids,
    };
    if ids
        .iter()
        .filter_map(|&id| analysis.decl(id))
        .all(|d| d.name == key)
    {
        return None;
    }
    let overloadable = ids
        .iter()
        .filter_map(|&id| analysis.decl(id))
        .all(|d| d.kind.is_overloadable());
    let places = occurrences(world, symbol, true);
    let mut target_scopes: Vec<(FileId, TextRange)> = Vec::new();
    for &id in ids {
        if let Some(parent) = analysis.decl(id).and_then(|d| d.parent) {
            target_scopes.extend(scopes(world, parent));
        }
    }
    let inside = |scopes: &[(FileId, TextRange)], file: FileId, range: TextRange| {
        scopes
            .iter()
            .any(|(f, scope)| *f == file && scope.contains_range(range))
    };
    for (id, other) in user_decls(world) {
        if other.name != key || ids.contains(&id) || analysis.files()[other.file as usize].builtin {
            continue;
        }
        if overloadable && other.kind.is_overloadable() {
            continue;
        }
        let what = || format!("{} `{}`", other.kind.describe(), other.spelling);
        match other.parent {
            Some(parent) => {
                // A renamed name inside the other's scope would find it.
                let scope = scopes(world, parent);
                if places.iter().any(|(f, r, _)| inside(&scope, *f, *r)) {
                    return Some(format!("{} is already visible there", what()));
                }
            }
            None => {
                // Library units share their library's namespace.
                let primary = |kind: DeclKind| {
                    matches!(
                        kind,
                        DeclKind::Entity
                            | DeclKind::Package
                            | DeclKind::PackageInstance
                            | DeclKind::Context
                            | DeclKind::Configuration
                    )
                };
                let library = |id: DeclId| analysis.units()[id.unit as usize].library.clone();
                if primary(other.kind)
                    && ids.iter().any(|&t| {
                        analysis
                            .decl(t)
                            .is_some_and(|d| d.parent.is_none() && primary(d.kind))
                            && library(t) == library(id)
                    })
                {
                    return Some(format!("{} already exists", what()));
                }
            }
        }
    }
    // A use of something else by the new name, where the renamed
    // declaration would hide it.
    if !target_scopes.is_empty() {
        for (file, reference) in analysis.references() {
            let text = &world.doc(file).text[std::ops::Range::<usize>::from(reference.range)];
            let last = text.rsplit('.').next().unwrap_or(text);
            if identifier_key(last) != key || !inside(&target_scopes, file, reference.range) {
                continue;
            }
            if text.contains('.') {
                continue;
            }
            let others: Vec<DeclId> = reference
                .targets
                .iter()
                .filter_map(|t| match t {
                    Target::Decl(d) if !ids.contains(d) => Some(*d),
                    _ => None,
                })
                .collect();
            if let Some(other) = others.first().and_then(|&d| analysis.decl(d))
                && !(overloadable && other.kind.is_overloadable())
            {
                return Some(format!(
                    "the new name would hide {} `{}` where it is used",
                    other.kind.describe(),
                    other.spelling
                ));
            }
        }
    }
    None
}

/// The text a declaration's names are visible in: its own, and that of
/// the bodies that continue it (architectures of an entity, the body of a
/// package or protected type).
fn scopes(world: &World, id: DeclId) -> Vec<(FileId, TextRange)> {
    let analysis = &world.analysis;
    let Some(decl) = analysis.decl(id) else {
        return Vec::new();
    };
    let mut out = vec![(decl.file, decl.full_range)];
    if decl.parent.is_none() && matches!(decl.kind, DeclKind::Entity | DeclKind::Package) {
        let library = &analysis.units()[id.unit as usize].library;
        for (unit, info) in analysis.units().iter().enumerate() {
            if info.primary.as_deref() == Some(decl.name.as_str())
                && info.library == *library
                && let Some(body) = analysis.unit_decls(unit as vfl_sema::UnitId).first()
            {
                out.push((body.file, body.full_range));
            }
        }
    }
    if matches!(decl.type_def.as_deref(), Some(TypeDef::Protected)) {
        for (_, body) in user_decls(world) {
            if body.kind == DeclKind::ProtectedBody
                && body.name == decl.name
                && crate::types::reference_at(analysis, body.file, body.range)
                    .is_some_and(|r| r.targets.contains(&Target::Decl(id)))
            {
                out.push((body.file, body.full_range));
            }
        }
    }
    out
}

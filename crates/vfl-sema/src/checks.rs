//! Checks that run on resolved units: unused and undriven objects, unused
//! use clauses, unused ports and generics, duplicate design units.

use rayon::prelude::*;
use rustc_hash::{FxHashMap, FxHashSet};

use vfl_syntax::{TextRange, TextSize};

use crate::analysis::{Ctx, UnitResult};
use crate::decl::{Decl, DeclId, DeclKind, Mode, Target, UnitId};
use crate::diagnostic::{Diagnostic, Edit, Fix, codes};

#[derive(Default, Clone, Copy)]
struct Usage {
    refs: u32,
    reads: u32,
    writes: u32,
}

fn usage_of<'r>(
    refs: impl Iterator<Item = &'r crate::decl::Reference>,
) -> FxHashMap<DeclId, Usage> {
    let mut usage: FxHashMap<DeclId, Usage> = FxHashMap::default();
    for r in refs {
        for target in &r.targets {
            if let Target::Decl(id) = target {
                let u = usage.entry(*id).or_default();
                u.refs += 1;
                u.reads += u32::from(r.access.reads());
                u.writes += u32::from(r.access.writes());
            }
        }
    }
    usage
}

/// Checks within one unit, run right after it is resolved.
pub(crate) fn unit_checks(ctx: &Ctx, unit: UnitId, mut result: UnitResult) -> UnitResult {
    let info = &ctx.units[unit as usize];
    if ctx.files[info.file as usize].builtin {
        return result;
    }
    let usage = usage_of(result.refs.iter());
    // Declarations directly in an entity or package are visible elsewhere;
    // only report the ones nothing outside the unit can see.
    let exported_parent = matches!(info.kind, DeclKind::Entity | DeclKind::Package);
    let mut diags = Vec::new();
    for (index, decl) in result.decls.iter().enumerate().skip(1) {
        let id = DeclId {
            unit,
            index: index as u32,
        };
        if exported_parent && decl.parent == Some(DeclId { unit, index: 0 }) {
            continue;
        }
        if in_protected_type(&result.decls, decl) {
            continue;
        }
        let u = usage.get(&id).copied().unwrap_or_default();
        let name = &decl.spelling;
        let (code, message) = match decl.kind {
            DeclKind::Signal if u.refs == 0 => (
                codes::UNUSED_SIGNAL,
                format!("signal `{name}` is never used"),
            ),
            DeclKind::Signal if u.reads == 0 && u.writes > 0 => (
                codes::UNREAD_SIGNAL,
                format!("signal `{name}` is assigned but never read"),
            ),
            DeclKind::Signal if u.writes == 0 && u.reads > 0 => (
                codes::UNDRIVEN_SIGNAL,
                format!("signal `{name}` is read but never assigned"),
            ),
            DeclKind::Variable if u.refs == 0 => (
                codes::UNUSED_VARIABLE,
                format!("variable `{name}` is never used"),
            ),
            DeclKind::Constant if u.refs == 0 => (
                codes::UNUSED_CONSTANT,
                format!("constant `{name}` is never used"),
            ),
            _ => continue,
        };
        let fix = matches!(
            code,
            codes::UNUSED_SIGNAL | codes::UNUSED_VARIABLE | codes::UNUSED_CONSTANT
        )
        .then(|| remove_declaration(&ctx.files[info.file as usize].text, &result.decls, decl))
        .flatten();
        diags.push(Diagnostic {
            code,
            file: decl.file,
            range: decl.range,
            message,
            fix,
        });
    }
    result.diagnostics.extend(diags);
    result
}

/// Removes an unused object: its name from a declaration of several, or
/// the whole declaration and its line.
fn remove_declaration(text: &str, decls: &[Decl], decl: &Decl) -> Option<Fix> {
    let shared = decls
        .iter()
        .filter(|d| d.full_range == decl.full_range && d.file == decl.file)
        .count();
    let range = if shared > 1 {
        crate::diagnostic::list_item_removal(text, decl.range)?
    } else {
        whole_lines(text, decl.full_range)
    };
    Some(Fix {
        title: format!(
            "Remove the unused {} `{}`",
            decl.kind.describe(),
            decl.spelling
        ),
        edits: vec![Edit {
            range,
            text: String::new(),
        }],
        safe: false,
    })
}

/// Members of protected types are used through method calls on objects,
/// which are not resolved.
fn in_protected_type(decls: &[Decl], decl: &Decl) -> bool {
    let mut parent = decl.parent;
    while let Some(id) = parent {
        let Some(p) = decls.get(id.index as usize) else {
            return false;
        };
        if p.kind == DeclKind::ProtectedBody || (p.kind == DeclKind::Type && p.members.is_some()) {
            return true;
        }
        parent = p.parent;
    }
    false
}

/// Checks across units, run once everything is resolved.
pub(crate) fn project_checks(ctx: &Ctx) -> Vec<Diagnostic> {
    let mut diags = Vec::new();
    duplicate_units(ctx, &mut diags);

    // Secondary units by (library, primary name).
    let mut secondaries: FxHashMap<(&str, &str), Vec<UnitId>> = FxHashMap::default();
    for (id, unit) in ctx.units.iter().enumerate() {
        if let Some(primary) = &unit.primary {
            secondaries
                .entry((unit.library.as_str(), primary.as_str()))
                .or_default()
                .push(id as UnitId);
        }
    }

    // Each unit's findings in parallel; a use clause of a file in two
    // libraries is then reported once.
    let per_unit: Vec<Vec<(bool, Diagnostic)>> = (0..ctx.units.len())
        .into_par_iter()
        .map(|id| unit_project_checks(ctx, &secondaries, id as UnitId))
        .collect();
    let mut reported_use_items: FxHashSet<(u32, TextSize)> = FxHashSet::default();
    for (is_use, diag) in per_unit.into_iter().flatten() {
        if is_use && !reported_use_items.insert((diag.file, diag.range.start())) {
            continue;
        }
        diags.push(diag);
    }
    diags
}

/// The cross-unit findings about one unit: unused use clauses (marked
/// `true`), unused ports and generics, output ports never driven, inout
/// ports used in one direction only.
fn unit_project_checks(
    ctx: &Ctx,
    secondaries: &FxHashMap<(&str, &str), Vec<UnitId>>,
    id: UnitId,
) -> Vec<(bool, Diagnostic)> {
    let mut diags = Vec::new();
    let unit = &ctx.units[id as usize];
    if ctx.files[unit.file as usize].builtin {
        return diags;
    }
    let Some(result) = ctx.result(id) else {
        return diags;
    };
    let related: Vec<UnitId> = match unit.kind {
        DeclKind::Entity | DeclKind::Package => secondaries
            .get(&(unit.library.as_str(), unit.name.as_str()))
            .cloned()
            .unwrap_or_default(),
        _ => Vec::new(),
    };

    // Unused use clauses.
    if unit.kind != DeclKind::Context && !result.use_items.is_empty() {
        let mut used: FxHashSet<u32> = FxHashSet::default();
        for &u in std::iter::once(&id).chain(&related) {
            if let Some(r) = ctx.result(u) {
                used.extend(
                    r.used_imports
                        .iter()
                        .filter(|i| i.unit == id)
                        .map(|i| i.index),
                );
            }
        }
        for (index, item) in result.use_items.iter().enumerate() {
            if !item.resolved || used.contains(&(index as u32)) {
                continue;
            }
            let text = &ctx.files[unit.file as usize].text;
            let item_text = &text[item.range];
            diags.push((
                true,
                Diagnostic {
                    code: codes::UNUSED_USE,
                    file: unit.file,
                    range: item.range,
                    message: format!("nothing from `{item_text}` is used"),
                    fix: Some(remove_use_item(text, item)),
                },
            ));
        }
    }

    // Unused ports and generics, and output ports never driven.
    if unit.kind == DeclKind::Entity && !related.is_empty() {
        let mut refs = result.refs.iter().collect::<Vec<_>>();
        for &arch in &related {
            if let Some(r) = ctx.result(arch) {
                refs.extend(r.refs.iter());
            }
        }
        let usage = usage_of(refs.into_iter());
        for (index, decl) in result.decls.iter().enumerate() {
            let decl_id = DeclId {
                unit: id,
                index: index as u32,
            };
            let u = usage.get(&decl_id).copied().unwrap_or_default();
            let (code, message) = match decl.kind {
                DeclKind::Port { .. } if u.refs == 0 => (
                    codes::UNUSED_PORT,
                    format!("port `{}` is never used", decl.spelling),
                ),
                DeclKind::Port {
                    mode: Mode::Out | Mode::Buffer,
                    ..
                } if u.writes == 0 => (
                    codes::UNDRIVEN_SIGNAL,
                    format!("output port `{}` is never assigned", decl.spelling),
                ),
                DeclKind::Port {
                    mode: Mode::InOut, ..
                } if u.reads > 0 && u.writes == 0 => (
                    codes::INOUT_PORT,
                    format!(
                        "inout port `{}` is never driven, so it could be `in`",
                        decl.spelling
                    ),
                ),
                DeclKind::Port {
                    mode: Mode::InOut, ..
                } if u.writes > 0 && u.reads == 0 => (
                    codes::INOUT_PORT,
                    format!(
                        "inout port `{}` is never read, so it could be `out`",
                        decl.spelling
                    ),
                ),
                DeclKind::Generic { .. } if u.refs == 0 => (
                    codes::UNUSED_GENERIC,
                    format!("generic `{}` is never used", decl.spelling),
                ),
                _ => continue,
            };
            diags.push((
                false,
                Diagnostic {
                    code,
                    file: decl.file,
                    range: decl.range,
                    message,
                    fix: None,
                },
            ));
        }
    }
    diags
}

fn duplicate_units(ctx: &Ctx, diags: &mut Vec<Diagnostic>) {
    for (library, units) in ctx.libraries {
        for list in units.values() {
            let user: Vec<UnitId> = list
                .iter()
                .copied()
                .filter(|&u| !ctx.files[ctx.units[u as usize].file as usize].builtin)
                .collect();
            if user.len() < 2 {
                continue;
            }
            for &u in &user {
                let unit = &ctx.units[u as usize];
                let others: Vec<String> = user
                    .iter()
                    .filter(|&&o| o != u)
                    .map(|&o| {
                        let other = &ctx.units[o as usize];
                        format!(
                            "{}:{}",
                            ctx.files[other.file as usize].path.display(),
                            ctx.line_of(other.file, other.name_range.start())
                        )
                    })
                    .collect();
                diags.push(Diagnostic {
                    code: codes::DUPLICATE_UNIT,
                    file: unit.file,
                    range: unit.name_range,
                    message: format!(
                        "{} `{}` is also declared in library `{library}` at {}",
                        unit.kind.describe(),
                        unit.name,
                        others.join(", ")
                    ),
                    fix: None,
                });
            }
        }
    }
}

/// Removes one item of a use clause, or the whole clause and its line.
fn remove_use_item(text: &str, item: &crate::resolver::UseItem) -> Fix {
    let range = if item.only_item {
        whole_lines(text, item.clause)
    } else {
        // The item and the comma after it, or the comma before it if last.
        let after = &text[usize::from(item.range.end())..usize::from(item.clause.end())];
        let comma_after = after.find(',').filter(|&i| after[..i].trim().is_empty());
        match comma_after {
            Some(i) => {
                let mut end = usize::from(item.range.end()) + i + 1;
                end += text[end..].len() - text[end..].trim_start_matches([' ', '\t']).len();
                TextRange::new(item.range.start(), TextSize::from(end as u32))
            }
            None => {
                let before =
                    &text[usize::from(item.clause.start())..usize::from(item.range.start())];
                let comma = before.rfind(',').unwrap_or(before.len());
                let start = usize::from(item.clause.start()) + comma;
                TextRange::new(TextSize::from(start as u32), item.range.end())
            }
        }
    };
    Fix {
        title: "Remove the unused use clause".into(),
        edits: vec![Edit {
            range,
            text: String::new(),
        }],
        safe: false,
    }
}

/// Extends `range` to whole lines when nothing else is on them.
fn whole_lines(text: &str, range: TextRange) -> TextRange {
    let start = usize::from(range.start());
    let end = usize::from(range.end());
    let line_start = text[..start].rfind('\n').map_or(0, |i| i + 1);
    let line_end = text[end..].find('\n').map_or(text.len(), |i| end + i + 1);
    let before_blank = text[line_start..start].trim().is_empty();
    let after_blank = text[end..line_end].trim().is_empty();
    if before_blank && after_blank {
        TextRange::new(
            TextSize::from(line_start as u32),
            TextSize::from(line_end as u32),
        )
    } else {
        range
    }
}

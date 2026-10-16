//! Completion.

use lsp_types::{CompletionItem, CompletionItemKind, CompletionParams, CompletionResponse};
use rustc_hash::FxHashSet;
use vfl_sema::{Decl, DeclId, DeclKind, FileId, Target};
use vfl_syntax::TextSize;

use super::file_of;
use super::references::file_references;
use crate::world::World;

pub(super) const KEYWORDS: &[&str] = &[
    "abs",
    "access",
    "after",
    "alias",
    "all",
    "and",
    "architecture",
    "array",
    "assert",
    "attribute",
    "begin",
    "block",
    "body",
    "buffer",
    "bus",
    "case",
    "component",
    "configuration",
    "constant",
    "context",
    "downto",
    "else",
    "elsif",
    "end",
    "entity",
    "exit",
    "file",
    "for",
    "function",
    "generate",
    "generic",
    "group",
    "guarded",
    "if",
    "impure",
    "in",
    "inertial",
    "inout",
    "is",
    "label",
    "library",
    "linkage",
    "literal",
    "loop",
    "map",
    "mod",
    "nand",
    "new",
    "next",
    "nor",
    "not",
    "null",
    "of",
    "on",
    "open",
    "or",
    "others",
    "out",
    "package",
    "port",
    "postponed",
    "procedure",
    "process",
    "protected",
    "pure",
    "range",
    "record",
    "register",
    "reject",
    "rem",
    "report",
    "return",
    "rol",
    "ror",
    "select",
    "severity",
    "shared",
    "signal",
    "sla",
    "sll",
    "sra",
    "srl",
    "subtype",
    "then",
    "to",
    "transport",
    "type",
    "unaffected",
    "units",
    "until",
    "use",
    "variable",
    "view",
    "wait",
    "when",
    "while",
    "with",
    "xnor",
    "xor",
];

fn completion_kind(kind: DeclKind) -> CompletionItemKind {
    match kind {
        DeclKind::Entity | DeclKind::Component => CompletionItemKind::INTERFACE,
        DeclKind::Architecture => CompletionItemKind::CLASS,
        DeclKind::Package
        | DeclKind::PackageInstance
        | DeclKind::PackageBody
        | DeclKind::Context
        | DeclKind::Configuration
        | DeclKind::GenericPackage => CompletionItemKind::MODULE,
        DeclKind::Signal | DeclKind::Variable | DeclKind::File | DeclKind::LoopParameter => {
            CompletionItemKind::VARIABLE
        }
        DeclKind::Constant | DeclKind::Generic { .. } | DeclKind::GenerateParameter => {
            CompletionItemKind::CONSTANT
        }
        DeclKind::Port { .. } | DeclKind::Parameter { .. } => CompletionItemKind::FIELD,
        DeclKind::Type
        | DeclKind::Subtype
        | DeclKind::IncompleteType
        | DeclKind::ProtectedBody
        | DeclKind::GenericType => CompletionItemKind::STRUCT,
        DeclKind::EnumLiteral => CompletionItemKind::ENUM_MEMBER,
        DeclKind::PhysicalUnit => CompletionItemKind::UNIT,
        DeclKind::Function | DeclKind::GenericSubprogram => CompletionItemKind::FUNCTION,
        DeclKind::Procedure => CompletionItemKind::METHOD,
        _ => CompletionItemKind::REFERENCE,
    }
}

#[derive(Default)]
struct Items {
    items: Vec<CompletionItem>,
    seen: FxHashSet<(String, String)>,
}

impl Items {
    pub(super) fn push(&mut self, label: &str, kind: CompletionItemKind, detail: &str) {
        if label.is_empty() || label.starts_with('"') {
            return;
        }
        if self
            .seen
            .insert((label.to_ascii_lowercase(), format!("{kind:?}")))
        {
            self.items.push(CompletionItem {
                label: label.to_owned(),
                kind: Some(kind),
                detail: (!detail.is_empty()).then(|| detail.to_owned()),
                ..Default::default()
            });
        }
    }

    pub(super) fn decl(&mut self, decl: &Decl) {
        self.push(
            &decl.spelling,
            completion_kind(decl.kind),
            decl.kind.describe(),
        );
    }
}

/// Calls `f` with the declarations visible at `offset` in `file`: those of
/// the file's units and their primary units, of the packages the file
/// refers to, and of `std.standard`.
pub(crate) fn visible_decls(
    world: &World,
    file: FileId,
    offset: TextSize,
    mut f: impl FnMut(DeclId, &Decl),
) {
    let analysis = &world.analysis;
    // Declarations in scope in this file's units and their primary units.
    let units: Vec<_> = analysis.file_units(file).collect();
    let mut scope_units = units.clone();
    for &unit in &units {
        let info = &analysis.units()[unit as usize];
        if let Some(primary) = &info.primary
            && let Some(p) = analysis.units().iter().position(|u| {
                u.library == info.library && u.name == *primary && u.primary.is_none()
            })
        {
            scope_units.push(p as vfl_sema::UnitId);
        }
    }
    for &unit in &scope_units {
        for (index, decl) in analysis.unit_decls(unit).iter().enumerate() {
            let visible = match decl.parent {
                None => true,
                Some(p) if p.index == 0 => true,
                Some(p) => analysis.decl(p).is_some_and(|parent| {
                    parent.file == file && parent.full_range.contains_inclusive(offset)
                }),
            };
            if visible {
                let id = DeclId {
                    unit,
                    index: index as u32,
                };
                f(id, decl);
            }
        }
    }
    // Packages this file refers to, and std.standard.
    let mut packages: Vec<DeclId> = Vec::new();
    for reference in file_references(world, file) {
        for target in &reference.targets {
            if let Target::Decl(id) = target
                && id.index == 0
                && matches!(
                    analysis.units()[id.unit as usize].kind,
                    DeclKind::Package | DeclKind::PackageInstance
                )
                && !packages.contains(id)
            {
                packages.push(*id);
            }
        }
    }
    if let Some(standard) = analysis.units().iter().position(|u| {
        u.library == "std" && u.name == "standard" && analysis.files()[u.file as usize].builtin
    }) {
        packages.push(DeclId {
            unit: standard as vfl_sema::UnitId,
            index: 0,
        });
    }
    for id in packages {
        if let Some(members) = analysis.decl(id).and_then(|d| d.members.clone()) {
            for member in members.iter() {
                if let Some(decl) = analysis.decl(member) {
                    f(member, decl);
                }
            }
        }
    }
}

pub(super) fn completion(
    world: &mut World,
    params: CompletionParams,
) -> Option<CompletionResponse> {
    let position = &params.text_document_position;
    let file = file_of(world, &position.text_document.uri)?;
    let offset = world.doc(file).offset(position.position);
    Some(CompletionResponse::Array(completion_at(
        world, file, offset,
    )))
}

/// The completions at `offset`.
pub(crate) fn completion_at(world: &World, file: FileId, offset: TextSize) -> Vec<CompletionItem> {
    let doc = world.doc(file);
    let offset = offset.min(TextSize::of(doc.text));
    let before = &doc.text[..usize::from(offset)];
    let word_start = before
        .trim_end_matches(|c: char| c.is_alphanumeric() || c == '_')
        .len();
    let analysis = &world.analysis;
    let mut items = Items::default();

    // After `prefix.`: what the prefix contains.
    if before[..word_start].ends_with('.') {
        let dot = TextSize::from((word_start - 1) as u32);
        let mut found = false;
        for reference in file_references(world, file) {
            if reference.range.end() != dot {
                continue;
            }
            for target in &reference.targets {
                match target {
                    Target::Library(library) => {
                        for unit in analysis.units() {
                            if *unit.library == **library
                                && unit.primary.is_none()
                                && !matches!(
                                    unit.kind,
                                    DeclKind::Architecture | DeclKind::PackageBody
                                )
                            {
                                items.push(
                                    &analysis.files()[unit.file as usize].text
                                        [std::ops::Range::<usize>::from(unit.name_range)],
                                    completion_kind(unit.kind),
                                    unit.kind.describe(),
                                );
                            }
                        }
                        found = true;
                    }
                    Target::Decl(id) => {
                        if let Some(members) = analysis.decl(*id).and_then(|d| d.members.clone()) {
                            for member in members.iter() {
                                if let Some(decl) = analysis.decl(member) {
                                    items.decl(decl);
                                }
                            }
                            items.push("all", CompletionItemKind::KEYWORD, "");
                            found = true;
                        }
                    }
                }
            }
        }
        if found {
            return items.items;
        }
        // Elements of a record, methods of a protected type.
        if let Some(members) = crate::types::members_after_dot(world, file, dot) {
            for (label, kind, detail) in members {
                items.push(&label, kind, &detail);
            }
            return items.items;
        }
    }

    visible_decls(world, file, offset, |_, decl| items.decl(decl));
    // Libraries and their units.
    let mut libraries: Vec<&str> = vec!["work"];
    for unit in analysis.units() {
        let unit_file = &analysis.files()[unit.file as usize];
        if !unit_file.builtin && unit.primary.is_none() {
            items.push(
                &unit_file.text[std::ops::Range::<usize>::from(unit.name_range)],
                completion_kind(unit.kind),
                if unit_file.foreign.is_some() {
                    "module"
                } else {
                    unit.kind.describe()
                },
            );
        }
        if !libraries.contains(&unit.library.as_str()) {
            libraries.push(&unit.library);
        }
    }
    for library in libraries {
        items.push(library, CompletionItemKind::MODULE, "library");
    }
    for keyword in KEYWORDS {
        items.push(keyword, CompletionItemKind::KEYWORD, "");
    }
    items.items
}

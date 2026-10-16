//! What is at a position, go to definition, find references and
//! document highlights.

use std::path::{Path, PathBuf};

use lsp_types::{
    DocumentHighlight, DocumentHighlightKind, DocumentHighlightParams, GotoDefinitionParams,
    GotoDefinitionResponse, Location, ReferenceParams,
};
use vfl_sema::{Access, Decl, DeclId, FileId, Target};
use vfl_syntax::{TextRange, TextSize};

use super::{file_of, uri};
use crate::refactor::{self, Symbol};
use crate::world::World;

/// What is at a position: a reference's targets, or a declaration's name.
pub(crate) struct Hit {
    pub range: TextRange,
    pub targets: Vec<DeclId>,
    pub library: Option<String>,
}

/// The references in a file, unit by unit.
pub(crate) fn file_references(
    world: &World,
    file: FileId,
) -> impl Iterator<Item = &vfl_sema::Reference> {
    world
        .analysis
        .file_units(file)
        .flat_map(|unit| world.analysis.unit_references(unit))
}

pub(crate) fn hit_at(world: &World, file: FileId, offset: TextSize) -> Option<Hit> {
    let contains = |r: TextRange| r.start() <= offset && offset <= r.end();
    for reference in file_references(world, file) {
        if contains(reference.range) {
            let mut targets = Vec::new();
            let mut library = None;
            for t in &reference.targets {
                match t {
                    Target::Decl(id) => targets.push(*id),
                    Target::Library(name) => library = Some(name.to_string()),
                }
            }
            return Some(Hit {
                range: reference.range,
                targets,
                library,
            });
        }
    }
    let decls = world.analysis.file_units(file).flat_map(|unit| {
        world
            .analysis
            .unit_decls(unit)
            .iter()
            .enumerate()
            .map(move |(index, decl)| {
                let index = index as u32;
                (DeclId { unit, index }, decl)
            })
    });
    for (id, decl) in decls {
        if decl.file == file && contains(decl.range) && !decl.spelling.is_empty() {
            return Some(Hit {
                range: decl.range,
                targets: vec![id],
                library: None,
            });
        }
    }
    None
}

/// Where a declaration is, writing bundled library sources to a cache
/// directory so editors can open them.
pub(super) fn location(world: &World, decl: &Decl) -> Option<Location> {
    let path = openable_path(world, decl.file)?;
    Some(Location {
        uri: uri(&path)?,
        range: world.doc(decl.file).range(decl.range),
    })
}

/// A path an editor can open for `file`: bundled library sources are
/// written to a cache directory first.
pub(crate) fn openable_path(world: &World, file: FileId) -> Option<PathBuf> {
    let file = &world.analysis.files()[file as usize];
    if file.builtin {
        bundled_path(&file.path, &file.text)
    } else {
        Some(file.path.clone())
    }
}

fn bundled_path(path: &Path, text: &str) -> Option<PathBuf> {
    let relative = path.strip_prefix("<bundled>").ok()?;
    let target = std::env::temp_dir()
        .join(format!("vfl-{}", env!("CARGO_PKG_VERSION")))
        .join(relative)
        .with_extension("vhd");
    if std::fs::read_to_string(&target).ok().as_deref() != Some(text) {
        std::fs::create_dir_all(target.parent()?).ok()?;
        std::fs::write(&target, text).ok()?;
    }
    Some(target)
}

pub(super) fn definition(
    world: &mut World,
    params: GotoDefinitionParams,
) -> Option<GotoDefinitionResponse> {
    let position = &params.text_document_position_params;
    let file = file_of(world, &position.text_document.uri)?;
    let offset = world.doc(file).offset(position.position);
    let locations: Vec<Location> = definitions(world, file, offset)
        .into_iter()
        .filter_map(|(file, range)| {
            Some(Location {
                uri: uri(&openable_path(world, file)?)?,
                range: world.doc(file).range(range),
            })
        })
        .collect();
    (!locations.is_empty()).then_some(GotoDefinitionResponse::Array(locations))
}

/// Where what is named at `offset` is declared.
pub(crate) fn definitions(
    world: &World,
    file: FileId,
    offset: TextSize,
) -> Vec<(FileId, TextRange)> {
    if let Some((_, Symbol::Element { record, name })) = refactor::element_at(world, file, offset) {
        return refactor::element_decls(world, record, &name);
    }
    let Some(hit) = hit_at(world, file, offset) else {
        return Vec::new();
    };
    hit.targets
        .iter()
        .filter_map(|id| world.analysis.decl(*id))
        .map(|decl| (decl.file, decl.range))
        .collect()
}

/// Every place that names one of `targets`, in file `within` or in all:
/// declarations and references, once each.
pub(crate) fn occurrences(
    world: &World,
    targets: &[DeclId],
    declarations: bool,
    within: Option<FileId>,
) -> Vec<(FileId, TextRange, Access)> {
    let mut out = Vec::new();
    if declarations {
        for id in targets {
            if let Some(decl) = world.analysis.decl(*id)
                && within.is_none_or(|f| f == decl.file)
            {
                out.push((decl.file, decl.range, Access::Other));
            }
        }
    }
    let references: Box<dyn Iterator<Item = (FileId, &vfl_sema::Reference)>> = match within {
        Some(file) => Box::new(world.analysis.file_units(file).flat_map(move |u| {
            world
                .analysis
                .unit_references(u)
                .iter()
                .map(move |r| (file, r))
        })),
        None => Box::new(world.analysis.references()),
    };
    for (file, reference) in references {
        if reference
            .targets
            .iter()
            .any(|t| matches!(t, Target::Decl(id) if targets.contains(id)))
        {
            out.push((file, reference.range, reference.access));
        }
    }
    out.sort_by_key(|(f, r, _)| (*f, r.start(), r.end()));
    out.dedup_by_key(|(f, r, _)| (*f, *r));
    out
}

pub(super) fn references(world: &mut World, params: ReferenceParams) -> Option<Vec<Location>> {
    let position = &params.text_document_position;
    let file = file_of(world, &position.text_document.uri)?;
    let offset = world.doc(file).offset(position.position);
    let (_, symbol) = refactor::symbol_at(world, file, offset, None)?;
    let found = refactor::occurrences(world, &symbol, params.context.include_declaration, None);
    Some(
        found
            .into_iter()
            .filter(|(file, _, _)| !world.analysis.files()[*file as usize].builtin)
            .filter_map(|(file, range, _)| {
                Some(Location {
                    uri: uri(&world.analysis.files()[file as usize].path)?,
                    range: world.doc(file).range(range),
                })
            })
            .collect(),
    )
}

pub(super) fn highlight(
    world: &mut World,
    params: DocumentHighlightParams,
) -> Option<Vec<DocumentHighlight>> {
    let position = &params.text_document_position_params;
    let file = file_of(world, &position.text_document.uri)?;
    let doc = world.doc(file);
    let (_, symbol) = refactor::symbol_at(world, file, doc.offset(position.position), Some(file))?;
    Some(
        refactor::occurrences(world, &symbol, true, Some(file))
            .into_iter()
            .filter(|(f, _, _)| *f == file)
            .map(|(_, range, access)| DocumentHighlight {
                range: doc.range(range),
                kind: Some(if access.writes() {
                    DocumentHighlightKind::WRITE
                } else if access.reads() {
                    DocumentHighlightKind::READ
                } else {
                    DocumentHighlightKind::TEXT
                }),
            })
            .collect(),
    )
}

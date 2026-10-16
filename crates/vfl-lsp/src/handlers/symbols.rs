//! Document and workspace symbols, outlines and folding ranges.

use lsp_types::{
    DocumentSymbol, DocumentSymbolParams, DocumentSymbolResponse, FoldingRange, FoldingRangeKind,
    FoldingRangeParams, SymbolInformation, SymbolKind, Url, WorkspaceSymbolParams,
    WorkspaceSymbolResponse,
};
use rustc_hash::{FxHashMap, FxHashSet};
use vfl_sema::{Decl, DeclId, DeclKind, FileId};
use vfl_syntax::{LineIndex, TextSize};

use super::file_of;
use super::references::location;
use crate::world::{Doc, World};

fn symbol_kind(kind: DeclKind) -> Option<SymbolKind> {
    Some(match kind {
        DeclKind::Entity => SymbolKind::INTERFACE,
        DeclKind::Architecture => SymbolKind::CLASS,
        DeclKind::Package | DeclKind::PackageInstance | DeclKind::PackageBody => {
            SymbolKind::PACKAGE
        }
        DeclKind::Context | DeclKind::Configuration => SymbolKind::MODULE,
        DeclKind::Component => SymbolKind::INTERFACE,
        DeclKind::Signal | DeclKind::Variable | DeclKind::File => SymbolKind::VARIABLE,
        DeclKind::Constant | DeclKind::Generic { .. } => SymbolKind::CONSTANT,
        DeclKind::Port { .. } => SymbolKind::FIELD,
        DeclKind::Type | DeclKind::Subtype | DeclKind::ProtectedBody | DeclKind::GenericType => {
            SymbolKind::STRUCT
        }
        DeclKind::EnumLiteral => SymbolKind::ENUM_MEMBER,
        DeclKind::Function | DeclKind::GenericSubprogram => SymbolKind::FUNCTION,
        DeclKind::Procedure => SymbolKind::METHOD,
        DeclKind::Alias | DeclKind::Attribute => SymbolKind::PROPERTY,
        DeclKind::Label => SymbolKind::EVENT,
        DeclKind::View | DeclKind::Group => SymbolKind::OBJECT,
        DeclKind::GenericPackage => SymbolKind::PACKAGE,
        DeclKind::IncompleteType
        | DeclKind::Parameter { .. }
        | DeclKind::PhysicalUnit
        | DeclKind::LoopParameter
        | DeclKind::GenerateParameter => return None,
    })
}

/// The declarations of a file worth listing, once each (a file in two
/// libraries is analyzed twice), with their ids.
fn file_decls(world: &World, file: FileId) -> Vec<(DeclId, &Decl)> {
    let mut seen = FxHashSet::default();
    let mut out = Vec::new();
    for unit in world.analysis.file_units(file) {
        for (index, decl) in world.analysis.unit_decls(unit).iter().enumerate() {
            if decl.file != file || decl.spelling.is_empty() || !seen.insert(decl.range) {
                continue;
            }
            out.push((
                DeclId {
                    unit,
                    index: index as u32,
                },
                decl,
            ));
        }
    }
    out
}

#[allow(deprecated)]
pub(super) fn document_symbols(
    world: &mut World,
    params: DocumentSymbolParams,
) -> Option<DocumentSymbolResponse> {
    let Some(file) = file_of(world, &params.text_document.uri) else {
        return verilog_symbols(world, &params.text_document.uri);
    };
    let doc = world.doc(file);
    let (decls, roots, children) = outline(world, file);
    fn build(
        i: usize,
        decls: &[(DeclId, &Decl)],
        children: &[Vec<usize>],
        doc: Doc,
    ) -> DocumentSymbol {
        let decl = decls[i].1;
        let mut range = decl.full_range;
        if !range.contains_range(decl.range) {
            range = decl.range;
        }
        DocumentSymbol {
            name: decl.spelling.clone(),
            detail: Some(decl.kind.describe().into()),
            kind: symbol_kind(decl.kind).unwrap_or(SymbolKind::NULL),
            tags: None,
            deprecated: None,
            range: doc.range(range),
            selection_range: doc.range(decl.range),
            children: Some(
                children[i]
                    .iter()
                    .map(|&c| build(c, decls, children, doc))
                    .collect(),
            ),
        }
    }
    Some(DocumentSymbolResponse::Nested(
        roots
            .into_iter()
            .map(|i| build(i, &decls, &children, doc))
            .collect(),
    ))
}

/// The outline of a Verilog file.
#[allow(deprecated)]
fn verilog_symbols(world: &World, uri: &Url) -> Option<DocumentSymbolResponse> {
    let file = crate::verilog::file(world, &uri.to_file_path().ok()?)?;
    let index = LineIndex::new(&file.text);
    let doc = Doc {
        text: &file.text,
        index: &index,
    };
    fn build(symbol: crate::verilog::Symbol, doc: Doc) -> DocumentSymbol {
        DocumentSymbol {
            name: symbol.name,
            detail: Some(symbol.detail),
            kind: symbol.kind,
            tags: None,
            deprecated: None,
            range: doc.range(symbol.range),
            selection_range: doc.range(symbol.selection),
            children: Some(symbol.children.into_iter().map(|c| build(c, doc)).collect()),
        }
    }
    Some(DocumentSymbolResponse::Nested(
        crate::verilog::symbols(&file.parse.syntax())
            .into_iter()
            .map(|s| build(s, doc))
            .collect(),
    ))
}

/// The regions of a Verilog file that can be folded. VHDL files keep the
/// editor's folding by indentation.
pub(super) fn folding_ranges(
    world: &mut World,
    params: FoldingRangeParams,
) -> Option<Vec<FoldingRange>> {
    let path = params.text_document.uri.to_file_path().ok()?;
    let file = crate::verilog::file(world, &path)?;
    let text = &*file.text;
    let index = LineIndex::new(text);
    let line = |offset: TextSize| index.line(offset);
    let mut out = Vec::new();
    for (range, fold) in crate::verilog::folds(&file.parse.syntax()) {
        let start = line(range.start());
        let mut end = line(range.end());
        // Keep a closing line (`end`, `);`) visible under the fold.
        let closing = fold == crate::verilog::Fold::Code
            && text[..usize::from(range.end())]
                .rsplit('\n')
                .next()
                .is_some_and(|before| before.trim().is_empty());
        if closing {
            end = end.saturating_sub(1);
        }
        if end > start {
            out.push(FoldingRange {
                start_line: start,
                end_line: end,
                kind: match fold {
                    crate::verilog::Fold::Code => None,
                    crate::verilog::Fold::Comment => Some(FoldingRangeKind::Comment),
                    crate::verilog::Fold::Region => Some(FoldingRangeKind::Region),
                },
                ..Default::default()
            });
        }
    }
    Some(out)
}

/// A file's outline: its listed declarations, the indexes of the top-level
/// ones, and the children of each.
#[allow(clippy::type_complexity)]
pub(crate) fn outline(
    world: &World,
    file: FileId,
) -> (Vec<(DeclId, &Decl)>, Vec<usize>, Vec<Vec<usize>>) {
    let decls = file_decls(world, file);
    let ids: FxHashMap<DeclId, usize> = decls
        .iter()
        .enumerate()
        .map(|(i, (id, _))| (*id, i))
        .collect();
    let mut children: Vec<Vec<usize>> = vec![Vec::new(); decls.len()];
    let mut roots = Vec::new();
    for (i, (_, decl)) in decls.iter().enumerate() {
        if symbol_kind(decl.kind).is_none() {
            continue;
        }
        // Attach to the nearest listed ancestor.
        let mut parent = decl.parent;
        let mut attached = false;
        while let Some(p) = parent {
            if let Some(&j) = ids.get(&p)
                && symbol_kind(decls[j].1.kind).is_some()
            {
                children[j].push(i);
                attached = true;
                break;
            }
            parent = world.analysis.decl(p).and_then(|d| d.parent);
        }
        if !attached {
            roots.push(i);
        }
    }
    (decls, roots, children)
}

#[allow(deprecated)]
pub(super) fn workspace_symbols(
    world: &mut World,
    params: WorkspaceSymbolParams,
) -> Option<WorkspaceSymbolResponse> {
    let mut out = Vec::new();
    for id in workspace_decls(world, &params.query) {
        let Some(decl) = world.analysis.decl(id) else {
            continue;
        };
        let Some(location) = location(world, decl) else {
            continue;
        };
        let container =
            (id.index != 0).then(|| world.analysis.unit_decls(id.unit)[0].spelling.clone());
        out.push(SymbolInformation {
            name: decl.spelling.clone(),
            kind: symbol_kind(decl.kind).unwrap_or(SymbolKind::NULL),
            tags: None,
            deprecated: None,
            location,
            container_name: container,
        });
    }
    Some(WorkspaceSymbolResponse::Flat(out))
}

/// The project's design units, types, subprograms and package objects
/// whose name contains `query`, ignoring case; 1000 at most.
pub(crate) fn workspace_decls(world: &World, query: &str) -> Vec<DeclId> {
    let query = query.to_ascii_lowercase();
    let mut out = Vec::new();
    let user_files = world.analysis.user_files().len() as FileId;
    for file in 0..user_files {
        for (id, decl) in file_decls(world, file) {
            let listed = matches!(
                decl.kind,
                DeclKind::Entity
                    | DeclKind::Architecture
                    | DeclKind::Package
                    | DeclKind::PackageInstance
                    | DeclKind::Context
                    | DeclKind::Configuration
                    | DeclKind::Component
                    | DeclKind::Type
                    | DeclKind::Subtype
                    | DeclKind::Function
                    | DeclKind::Procedure
            ) || (matches!(decl.kind, DeclKind::Constant | DeclKind::Signal)
                && decl.parent.is_some_and(|p| {
                    p.index == 0
                        && world.analysis.units()[p.unit as usize].kind == DeclKind::Package
                }));
            if !listed || !decl.name.contains(&query) {
                continue;
            }
            out.push(id);
            if out.len() >= 1000 {
                return out;
            }
        }
    }
    out
}

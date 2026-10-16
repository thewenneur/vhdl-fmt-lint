//! Navigation beyond go to definition: go to implementation (an entity's
//! architectures, a component's entity, a package's body, a subprogram's
//! body), to type definition and to declaration (from a body to what it
//! completes); linked editing of a name and its `end` name; and selection
//! ranges, from the syntax tree.
//!
//! Each request reads the analysis and the tree as they are; nothing here
//! runs on an edit.

use lsp_types::{
    GotoDefinitionResponse, LinkedEditingRangeParams, LinkedEditingRanges, Location,
    SelectionRange, SelectionRangeParams, TextDocumentPositionParams,
};
use vfl_sema::{DeclId, DeclKind, FileId, Target, TypeDef};
use vfl_syntax::{SyntaxKind, SyntaxNode, SyntaxToken, TextRange, TextSize};

use crate::handlers::{hit_at, openable_path};
use crate::refactor::{self, Symbol};
use crate::types::reference_at;
use crate::world::World;

/// The file and offset of a position, in an analyzed VHDL file.
pub(crate) fn file_offset(
    world: &World,
    position: &TextDocumentPositionParams,
) -> Option<(FileId, TextSize)> {
    let file = world.file_id(&position.text_document.uri.to_file_path().ok()?)?;
    Some((file, world.doc(file).offset(position.position)))
}

/// Locations an editor can open, once each.
pub(crate) fn locations(world: &World, mut found: Vec<(FileId, TextRange)>) -> Vec<Location> {
    found.sort_by_key(|(f, r)| (*f, r.start()));
    found.dedup();
    found
        .into_iter()
        .filter_map(|(file, range)| {
            Some(Location {
                uri: lsp_types::Url::from_file_path(openable_path(world, file)?).ok()?,
                range: world.doc(file).range(range),
            })
        })
        .collect()
}

fn response(world: &World, found: Vec<(FileId, TextRange)>) -> Option<GotoDefinitionResponse> {
    let locations = locations(world, found);
    (!locations.is_empty()).then_some(GotoDefinitionResponse::Array(locations))
}

/// The declarations named at `offset`.
fn targets(world: &World, file: FileId, offset: TextSize) -> Vec<DeclId> {
    hit_at(world, file, offset).map_or_else(Vec::new, |hit| hit.targets)
}

fn name_of(world: &World, id: DeclId) -> (FileId, TextRange) {
    let decl = &world.analysis.unit_decls(id.unit)[id.index as usize];
    (decl.file, decl.range)
}

/// The design unit declared as `id`, if it is one.
fn unit_info(world: &World, id: DeclId) -> Option<&vfl_sema::UnitInfo> {
    (id.index == 0).then(|| &world.analysis.units()[id.unit as usize])
}

/// The secondary units of kind `kind` of the primary unit `id`: an
/// entity's architectures, a package's body.
pub(crate) fn secondary_units(world: &World, id: DeclId, kind: DeclKind) -> Vec<DeclId> {
    let Some(primary) = unit_info(world, id) else {
        return Vec::new();
    };
    world
        .analysis
        .units()
        .iter()
        .enumerate()
        .filter(|(_, u)| {
            u.kind == kind
                && u.library == primary.library
                && u.primary.as_deref() == Some(primary.name.as_str())
        })
        .map(|(i, _)| DeclId {
            unit: i as vfl_sema::UnitId,
            index: 0,
        })
        .collect()
}

/// The entities (or modules of other languages) named like component
/// `id`, which it binds to by default: those of its own library first.
pub(crate) fn bound_entities(world: &World, id: DeclId) -> Vec<DeclId> {
    let analysis = &world.analysis;
    let Some(component) = analysis.decl(id) else {
        return Vec::new();
    };
    let library = &analysis.units()[id.unit as usize].library;
    let mut found: Vec<(bool, DeclId)> = analysis
        .units()
        .iter()
        .enumerate()
        .filter(|(_, u)| u.kind == DeclKind::Entity && u.name == component.name)
        .map(|(i, u)| {
            let id = DeclId {
                unit: i as vfl_sema::UnitId,
                index: 0,
            };
            (u.library != *library, id)
        })
        .collect();
    found.sort();
    found.into_iter().map(|(_, id)| id).collect()
}

/// The declarations that complete `id`: the bodies of a subprogram or a
/// protected type, the full declaration of a deferred constant. A body's
/// name refers to what it completes, so these are found where they can be:
/// in the unit of `id` and in the body of its package.
pub(crate) fn completions(world: &World, id: DeclId) -> Vec<DeclId> {
    let analysis = &world.analysis;
    let Some(decl) = analysis.decl(id) else {
        return Vec::new();
    };
    let mut units = vec![id.unit];
    units.extend(
        secondary_units(
            world,
            DeclId {
                unit: id.unit,
                index: 0,
            },
            DeclKind::PackageBody,
        )
        .into_iter()
        .map(|u| u.unit),
    );
    let mut out = Vec::new();
    for unit in units {
        for (index, candidate) in analysis.unit_decls(unit).iter().enumerate() {
            let candidate_id = DeclId {
                unit,
                index: index as u32,
            };
            if candidate_id == id || candidate.name != decl.name {
                continue;
            }
            if reference_at(analysis, candidate.file, candidate.range)
                .is_some_and(|r| r.targets.contains(&Target::Decl(id)))
            {
                out.push(candidate_id);
            }
        }
    }
    out
}

/// What `id` completes, when it is a body or a secondary unit: a
/// subprogram's declaration, a package or an architecture's entity.
pub(crate) fn completed(world: &World, id: DeclId) -> Option<DeclId> {
    let analysis = &world.analysis;
    let decl = analysis.decl(id)?;
    if decl.kind == DeclKind::Architecture {
        let unit = unit_info(world, id)?;
        let entity = analysis.units().iter().position(|u| {
            u.kind == DeclKind::Entity
                && u.library == unit.library
                && Some(u.name.as_str()) == unit.primary.as_deref()
        })?;
        return Some(DeclId {
            unit: entity as vfl_sema::UnitId,
            index: 0,
        });
    }
    let reference = reference_at(analysis, decl.file, decl.range)?;
    reference.targets.iter().find_map(|t| match t {
        Target::Decl(t) if *t != id => Some(*t),
        _ => None,
    })
}

/// `textDocument/implementation`.
pub fn implementation(
    world: &mut World,
    params: TextDocumentPositionParams,
) -> Option<GotoDefinitionResponse> {
    let (file, offset) = file_offset(world, &params)?;
    response(world, implementations(world, file, offset))
}

pub(crate) fn implementations(
    world: &World,
    file: FileId,
    offset: TextSize,
) -> Vec<(FileId, TextRange)> {
    let mut out = Vec::new();
    for id in targets(world, file, offset) {
        let Some(decl) = world.analysis.decl(id) else {
            continue;
        };
        let found = match decl.kind {
            DeclKind::Entity => secondary_units(world, id, DeclKind::Architecture),
            DeclKind::Package => secondary_units(world, id, DeclKind::PackageBody),
            DeclKind::Component => bound_entities(world, id),
            _ => completions(world, id),
        };
        out.extend(found.into_iter().map(|id| name_of(world, id)));
    }
    out
}

/// `textDocument/declaration`: from a body to what it completes, from an
/// architecture to its entity; elsewhere, where the name is declared.
pub fn declaration(
    world: &mut World,
    params: TextDocumentPositionParams,
) -> Option<GotoDefinitionResponse> {
    let (file, offset) = file_offset(world, &params)?;
    let found = targets(world, file, offset)
        .into_iter()
        .map(|id| name_of(world, completed(world, id).unwrap_or(id)))
        .collect();
    response(world, found)
}

/// `textDocument/typeDefinition`: the type mark of an object, an alias, a
/// record element or a function's result; the type of an enumeration
/// literal or a unit; the type mark of a subtype.
pub fn type_definition(
    world: &mut World,
    params: TextDocumentPositionParams,
) -> Option<GotoDefinitionResponse> {
    let (file, offset) = file_offset(world, &params)?;
    response(world, type_definitions(world, file, offset))
}

pub(crate) fn type_definitions(
    world: &World,
    file: FileId,
    offset: TextSize,
) -> Vec<(FileId, TextRange)> {
    let analysis = &world.analysis;
    if let Some((_, Symbol::Element { record, name })) = refactor::element_at(world, file, offset) {
        let Some(TypeDef::Record(elements)) =
            analysis.decl(record).and_then(|d| d.type_def.as_deref())
        else {
            return Vec::new();
        };
        return elements
            .iter()
            .filter(|e| e.name == name)
            .filter_map(|e| e.subtype)
            .map(|id| name_of(world, id))
            .collect();
    }
    targets(world, file, offset)
        .into_iter()
        .filter_map(|id| {
            let decl = analysis.decl(id)?;
            let typed = decl.kind.is_object()
                || matches!(
                    decl.kind,
                    DeclKind::Alias
                        | DeclKind::Function
                        | DeclKind::EnumLiteral
                        | DeclKind::PhysicalUnit
                        | DeclKind::Subtype
                );
            typed.then_some(decl.subtype?)
        })
        .map(|id| name_of(world, id))
        .collect()
}

fn is_ident(kind: SyntaxKind) -> bool {
    matches!(kind, SyntaxKind::IDENT | SyntaxKind::EXT_IDENT)
}

/// The token at `offset`, found by a binary search among the children at
/// each level: `token_at_offset` builds every child it passes over.
pub(crate) fn token_at(root: &SyntaxNode, offset: TextSize) -> Option<SyntaxToken> {
    let mut node = root.clone();
    loop {
        let range = TextRange::at(offset, 1.into());
        match node.child_or_token_at_range(range)? {
            rowan::NodeOrToken::Node(child) => node = child,
            rowan::NodeOrToken::Token(token) => return Some(token),
        }
    }
}

/// The identifier at `offset`, also when `offset` is just after it.
fn ident_at(root: &SyntaxNode, offset: TextSize) -> Option<SyntaxToken> {
    root.token_at_offset(offset).find(|t| is_ident(t.kind()))
}

/// The name at the start of a construct: its label, its designator.
fn start_name(node: &SyntaxNode) -> Option<SyntaxToken> {
    match node.kind() {
        SyntaxKind::RECORD_TYPE_DEF
        | SyntaxKind::PROTECTED_TYPE_DEF
        | SyntaxKind::PROTECTED_TYPE_BODY
        | SyntaxKind::PHYSICAL_TYPE_DEF => first_ident(&node.parent()?),
        SyntaxKind::SUBPROGRAM_BODY => node
            .children()
            .find(|n| {
                matches!(
                    n.kind(),
                    SyntaxKind::FUNCTION_SPEC | SyntaxKind::PROCEDURE_SPEC
                )
            })
            .and_then(|spec| first_ident(&spec)),
        SyntaxKind::ENTITY_DECL
        | SyntaxKind::ARCHITECTURE_BODY
        | SyntaxKind::PACKAGE_DECL
        | SyntaxKind::PACKAGE_BODY
        | SyntaxKind::CONFIGURATION_DECL
        | SyntaxKind::CONTEXT_DECL
        | SyntaxKind::COMPONENT_DECL => first_ident(node),
        _ => node
            .children()
            .find(|n| n.kind() == SyntaxKind::LABEL)
            .and_then(|label| first_ident(&label)),
    }
}

fn first_ident(node: &SyntaxNode) -> Option<SyntaxToken> {
    node.children_with_tokens()
        .filter_map(|e| e.into_token())
        .find(|t| is_ident(t.kind()))
}

/// The name after the `end` of a construct, if it repeats it.
fn end_name(node: &SyntaxNode) -> Option<SyntaxToken> {
    let mut after_end = false;
    let mut found = None;
    for token in node.children_with_tokens().filter_map(|e| e.into_token()) {
        match token.kind() {
            SyntaxKind::END_KW => {
                after_end = true;
                found = None;
            }
            kind if after_end && is_ident(kind) => found = Some(token),
            _ => {}
        }
    }
    found
}

/// The construct whose start or end name `token` is.
fn construct_of(token: &SyntaxToken) -> Option<SyntaxNode> {
    let parent = token.parent()?;
    match parent.kind() {
        SyntaxKind::LABEL => parent.parent(),
        SyntaxKind::TYPE_DECL => parent.children().find(|n| {
            matches!(
                n.kind(),
                SyntaxKind::RECORD_TYPE_DEF
                    | SyntaxKind::PROTECTED_TYPE_DEF
                    | SyntaxKind::PROTECTED_TYPE_BODY
                    | SyntaxKind::PHYSICAL_TYPE_DEF
            )
        }),
        SyntaxKind::FUNCTION_SPEC | SyntaxKind::PROCEDURE_SPEC => parent
            .parent()
            .filter(|p| p.kind() == SyntaxKind::SUBPROGRAM_BODY),
        _ => Some(parent),
    }
}

/// A name and the `end` name that repeats it, when the token at `offset`
/// is one of them: an entity, architecture, package, component,
/// subprogram, type or labelled statement.
pub(crate) fn linked_names(
    world: &World,
    file: FileId,
    offset: TextSize,
) -> Option<[TextRange; 2]> {
    let root = world.analysis.files().get(file as usize)?.syntax();
    let token = ident_at(&root, offset)?;
    let node = construct_of(&token)?;
    let start = start_name(&node)?;
    let end = end_name(&node)?;
    let same = vfl_syntax::identifier_key(start.text()) == vfl_syntax::identifier_key(end.text());
    (same && (token == start || token == end)).then(|| [start.text_range(), end.text_range()])
}

/// `textDocument/linkedEditingRange`.
pub fn linked_editing(
    world: &mut World,
    params: LinkedEditingRangeParams,
) -> Option<LinkedEditingRanges> {
    let (file, offset) = file_offset(world, &params.text_document_position_params)?;
    let doc = world.doc(file);
    let [start, end] = linked_names(world, file, offset)?;
    // Two ranges with the same text: an edit that changes the case of one
    // would no longer be mirrored.
    (doc.text[std::ops::Range::<usize>::from(start)]
        == doc.text[std::ops::Range::<usize>::from(end)])
    .then(|| LinkedEditingRanges {
        ranges: vec![doc.range(start), doc.range(end)],
        word_pattern: Some(r"[a-zA-Z][a-zA-Z0-9_]*|\\([^\\]|\\\\)+\\".into()),
    })
}

/// A node's range without its leading and trailing comments and spaces.
fn trimmed(node: &SyntaxNode) -> Option<TextRange> {
    let range = node.text_range();
    let mut first = node.first_token()?;
    while first.kind().is_trivia() {
        first = first
            .next_token()
            .filter(|t| t.text_range().end() <= range.end())?;
    }
    let mut last = node.last_token()?;
    while last.kind().is_trivia() {
        last = last
            .prev_token()
            .filter(|t| t.text_range().start() >= range.start())?;
    }
    Some(TextRange::new(
        first.text_range().start(),
        last.text_range().end(),
    ))
}

/// The ranges around `offset`, innermost first: the token, then each
/// syntax node around it.
pub(crate) fn selection_ranges(root: &SyntaxNode, offset: TextSize) -> Vec<TextRange> {
    let token = root
        .token_at_offset(offset)
        .max_by_key(|t| (!t.kind().is_trivia(), is_ident(t.kind())));
    let mut out: Vec<TextRange> = Vec::new();
    let Some(token) = token else {
        return out;
    };
    if !token.kind().is_trivia() {
        out.push(token.text_range());
    }
    for node in token.parent_ancestors() {
        let Some(range) = trimmed(&node) else {
            continue;
        };
        if out
            .last()
            .is_none_or(|last| range != *last && range.contains_range(*last))
        {
            out.push(range);
        }
    }
    let whole = root.text_range();
    if out.last() != Some(&whole) {
        out.push(whole);
    }
    out
}

/// `textDocument/selectionRange`.
pub fn selection_range(
    world: &mut World,
    params: SelectionRangeParams,
) -> Option<Vec<SelectionRange>> {
    let file = world.file_id(&params.text_document.uri.to_file_path().ok()?)?;
    let doc = world.doc(file);
    let root = world.analysis.files()[file as usize].syntax();
    Some(
        params
            .positions
            .iter()
            .map(|&position| {
                let offset = doc.offset(position);
                let mut ranges = selection_ranges(&root, offset);
                if ranges.is_empty() {
                    ranges.push(TextRange::empty(offset));
                }
                let mut selection: Option<SelectionRange> = None;
                for range in ranges.into_iter().rev() {
                    selection = Some(SelectionRange {
                        range: doc.range(range),
                        parent: selection.map(Box::new),
                    });
                }
                selection.expect("at least one range")
            })
            .collect(),
    )
}

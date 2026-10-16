//! Code lenses counting the instances of entities and components, and the
//! call hierarchy: who calls a subprogram and what it calls, and the
//! instance tree (which design entities instantiate an entity, and what
//! its architecture instantiates).
//!
//! A code lens is only a position until the editor shows it: its count is
//! worked out by `codeLens/resolve`. Both read the analysis as it is.

use lsp_types::{
    CallHierarchyIncomingCall, CallHierarchyIncomingCallsParams, CallHierarchyItem,
    CallHierarchyOutgoingCall, CallHierarchyOutgoingCallsParams, CallHierarchyPrepareParams,
    CodeLens, CodeLensParams, Command, SymbolKind, Url,
};
use rustc_hash::{FxHashMap, FxHashSet};
use serde::{Deserialize, Serialize};
use vfl_sema::{Access, DeclId, DeclKind, FileId, Target, UnitId};
use vfl_syntax::{SyntaxKind, SyntaxNode, TextRange, TextSize};

use crate::handlers::{hit_at, openable_path};
use crate::navigate::{
    bound_entities, completed, completions, file_offset, locations, secondary_units, token_at,
};
use crate::world::World;

/// The client command a resolved code lens runs: show these references.
/// Its arguments are the document's URI, the position of the name and the
/// locations, as in VS Code's `editor.action.showReferences`.
pub const SHOW_REFERENCES: &str = "vfl.showReferences";

/// Where a declaration is named: what code lenses and hierarchy items keep
/// between requests, since declaration ids do not outlive an analysis.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Anchor {
    uri: Url,
    offset: u32,
    /// For hierarchy items: whether the item is a design entity in the
    /// instance tree, rather than a subprogram, process or unit in the
    /// call hierarchy.
    #[serde(default)]
    instances: bool,
}

/// The declaration whose name starts at `anchor`.
fn anchored(world: &World, anchor: &Anchor) -> Option<DeclId> {
    let file = world.file_id(&anchor.uri.to_file_path().ok()?)?;
    let offset = TextSize::from(anchor.offset);
    world.analysis.file_units(file).find_map(|unit| {
        world
            .analysis
            .unit_decls(unit)
            .iter()
            .position(|d| d.file == file && d.range.start() == offset)
            .map(|index| DeclId {
                unit,
                index: index as u32,
            })
    })
}

fn uri_of(world: &World, file: FileId) -> Option<Url> {
    Url::from_file_path(openable_path(world, file)?).ok()
}

/// `textDocument/codeLens`: one lens per entity and component declared in
/// the file, to be resolved.
pub fn code_lens(world: &mut World, params: CodeLensParams) -> Option<Vec<CodeLens>> {
    let uri = params.text_document.uri;
    let file = world.file_id(&uri.to_file_path().ok()?)?;
    let doc = world.doc(file);
    let mut seen = FxHashSet::default();
    let mut out = Vec::new();
    for unit in world.analysis.file_units(file) {
        for decl in world.analysis.unit_decls(unit) {
            if decl.file == file
                && matches!(decl.kind, DeclKind::Entity | DeclKind::Component)
                && seen.insert(decl.range)
            {
                out.push(CodeLens {
                    range: doc.range(decl.range),
                    command: None,
                    data: serde_json::to_value(Anchor {
                        uri: uri.clone(),
                        offset: decl.range.start().into(),
                        instances: true,
                    })
                    .ok(),
                });
            }
        }
    }
    Some(out)
}

/// `codeLens/resolve`: how many times the entity or component is
/// instantiated, showing the instances when clicked.
pub fn resolve_code_lens(world: &mut World, mut lens: CodeLens) -> CodeLens {
    let anchor: Option<Anchor> = lens
        .data
        .clone()
        .and_then(|d| serde_json::from_value(d).ok());
    let found = anchor
        .as_ref()
        .and_then(|a| anchored(world, a))
        .map(|id| locations(world, instances_of(world, id)))
        .unwrap_or_default();
    let title = match found.len() {
        1 => "1 instance".to_owned(),
        n => format!("{n} instances"),
    };
    let arguments = anchor.map(|a| {
        vec![
            serde_json::json!(a.uri),
            serde_json::json!(lens.range.start),
            serde_json::json!(found),
        ]
    });
    lens.command = Some(Command {
        title,
        command: SHOW_REFERENCES.into(),
        arguments,
    });
    lens
}

/// The units of the project's own files.
fn user_units(world: &World) -> impl Iterator<Item = UnitId> + '_ {
    let analysis = &world.analysis;
    (0..analysis.units().len() as UnitId)
        .filter(|&u| !analysis.files()[analysis.units()[u as usize].file as usize].builtin)
}

/// Whether the name at `range` is the unit an instantiation statement
/// instantiates.
fn is_instantiation(root: &SyntaxNode, range: TextRange) -> bool {
    let Some(token) = token_at(root, range.start()) else {
        return false;
    };
    for node in token.parent_ancestors() {
        match node.kind() {
            SyntaxKind::NAME | SyntaxKind::SELECTED_NAME | SyntaxKind::CALL_EXPR => {}
            SyntaxKind::COMPONENT_INSTANTIATION_STMT => return true,
            _ => return false,
        }
    }
    false
}

/// The trees of files, built once each.
#[derive(Default)]
struct Roots(FxHashMap<FileId, SyntaxNode>);

impl Roots {
    fn get(&mut self, world: &World, file: FileId) -> &SyntaxNode {
        self.0
            .entry(file)
            .or_insert_with(|| world.analysis.files()[file as usize].syntax())
    }
}

/// Where entity or component `id` is instantiated: the instantiated names.
/// An entity's instances include those of the components named after it,
/// which bind to it by default.
pub(crate) fn instances_of(world: &World, id: DeclId) -> Vec<(FileId, TextRange)> {
    let analysis = &world.analysis;
    let Some(decl) = analysis.decl(id) else {
        return Vec::new();
    };
    let mut targets: FxHashSet<DeclId> = FxHashSet::default();
    targets.insert(id);
    if decl.kind == DeclKind::Entity {
        for unit in user_units(world) {
            for (index, d) in analysis.unit_decls(unit).iter().enumerate() {
                if d.kind == DeclKind::Component && d.name == decl.name {
                    targets.insert(DeclId {
                        unit,
                        index: index as u32,
                    });
                }
            }
        }
    }
    let mut roots = Roots::default();
    let mut out = Vec::new();
    for unit in user_units(world) {
        let file = analysis.units()[unit as usize].file;
        for reference in analysis.unit_references(unit) {
            let named = reference
                .targets
                .iter()
                .any(|t| matches!(t, Target::Decl(t) if targets.contains(t)));
            if named && is_instantiation(roots.get(world, file), reference.range) {
                out.push((file, reference.range));
            }
        }
    }
    out.sort_by_key(|(f, r)| (*f, r.start()));
    out.dedup();
    out
}

/// A node of a hierarchy: a declaration, and whether it is a design entity
/// of the instance tree (an architecture, or an entity or component
/// without one) rather than a subprogram, process or unit making calls.
pub(crate) type Node = (DeclId, bool);

/// How a node is shown: its name, what it is, and its symbol kind.
pub(crate) fn describe(
    world: &World,
    (id, instances): Node,
) -> Option<(String, String, SymbolKind)> {
    let analysis = &world.analysis;
    let decl = analysis.decl(id)?;
    if instances && decl.kind == DeclKind::Architecture {
        let entity = completed(world, id).and_then(|e| analysis.decl(e));
        let name = entity.map_or_else(|| decl.spelling.clone(), |e| e.spelling.clone());
        return Some((
            name,
            format!("architecture {}", decl.spelling),
            SymbolKind::CLASS,
        ));
    }
    let name = if decl.spelling.is_empty() {
        "process".to_owned()
    } else {
        decl.spelling.clone()
    };
    let kind = match decl.kind {
        DeclKind::Function => SymbolKind::FUNCTION,
        DeclKind::Procedure => SymbolKind::METHOD,
        DeclKind::Label => SymbolKind::EVENT,
        DeclKind::Package | DeclKind::PackageBody => SymbolKind::PACKAGE,
        DeclKind::Architecture => SymbolKind::CLASS,
        _ => SymbolKind::INTERFACE,
    };
    let detail = match decl.kind {
        DeclKind::Label => "process".to_owned(),
        kind => kind.describe().to_owned(),
    };
    Some((name, detail, kind))
}

/// A hierarchy item for a node.
fn item(world: &World, node: Node) -> Option<CallHierarchyItem> {
    let (id, instances) = node;
    let decl = world.analysis.decl(id)?;
    let doc = world.doc(decl.file);
    let uri = uri_of(world, decl.file)?;
    let (name, detail, kind) = describe(world, node)?;
    let mut range = decl.full_range;
    if !range.contains_range(decl.range) {
        range = decl.range;
    }
    Some(CallHierarchyItem {
        name,
        kind,
        tags: None,
        detail: Some(detail),
        uri: uri.clone(),
        range: doc.range(range),
        selection_range: doc.range(decl.range),
        data: serde_json::to_value(Anchor {
            uri,
            offset: decl.range.start().into(),
            instances,
        })
        .ok(),
    })
}

fn is_subprogram(kind: DeclKind) -> bool {
    matches!(kind, DeclKind::Function | DeclKind::Procedure)
}

/// The design entity an entity or component stands for in the instance
/// tree: the last architecture of the entity (the default binding), the
/// entity itself without one, or the component without an entity.
fn design_entity(world: &World, id: DeclId) -> DeclId {
    let entity = match world.analysis.decl(id).map(|d| d.kind) {
        Some(DeclKind::Component) => match bound_entities(world, id).first() {
            Some(&entity) => entity,
            None => return id,
        },
        _ => id,
    };
    secondary_units(world, entity, DeclKind::Architecture)
        .last()
        .copied()
        .unwrap_or(entity)
}

/// The nodes named at `offset`: a subprogram (its body when it has one),
/// or the design entity of an entity, component or architecture.
pub(crate) fn nodes_at(world: &World, file: FileId, offset: TextSize) -> Vec<Node> {
    let Some(hit) = hit_at(world, file, offset) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for id in hit.targets {
        let Some(decl) = world.analysis.decl(id) else {
            continue;
        };
        let node = match decl.kind {
            kind if is_subprogram(kind) => {
                (completions(world, id).first().copied().unwrap_or(id), false)
            }
            DeclKind::Entity | DeclKind::Component => (design_entity(world, id), true),
            DeclKind::Architecture => (id, true),
            _ => continue,
        };
        if !out.contains(&node) {
            out.push(node);
        }
    }
    out
}

/// `textDocument/prepareCallHierarchy`.
pub fn prepare(
    world: &mut World,
    params: CallHierarchyPrepareParams,
) -> Option<Vec<CallHierarchyItem>> {
    let (file, offset) = file_offset(world, &params.text_document_position_params)?;
    let items: Vec<CallHierarchyItem> = nodes_at(world, file, offset)
        .into_iter()
        .filter_map(|node| item(world, node))
        .collect();
    (!items.is_empty()).then_some(items)
}

/// What calls are made from in each file: the subprogram bodies, processes
/// and design units, worked out once per file.
#[derive(Default)]
struct Callers(FxHashMap<FileId, Vec<(TextRange, DeclId)>>);

impl Callers {
    /// The innermost subprogram body, process or design unit around a
    /// call in `file` at `range`.
    fn find(&mut self, world: &World, file: FileId, range: TextRange) -> Option<DeclId> {
        let callers = self.0.entry(file).or_insert_with(|| {
            let analysis = &world.analysis;
            let root = analysis.files()[file as usize].syntax();
            let mut out = Vec::new();
            for unit in analysis.file_units(file) {
                for (index, decl) in analysis.unit_decls(unit).iter().enumerate() {
                    let callable = match decl.kind {
                        DeclKind::Label => {
                            root.covering_element(decl.full_range).kind()
                                == SyntaxKind::PROCESS_STMT
                        }
                        kind => {
                            is_subprogram(kind)
                                || matches!(
                                    kind,
                                    DeclKind::Architecture
                                        | DeclKind::Entity
                                        | DeclKind::Package
                                        | DeclKind::PackageBody
                                )
                        }
                    };
                    if decl.file == file && callable {
                        let index = index as u32;
                        out.push((decl.full_range, DeclId { unit, index }));
                    }
                }
            }
            out
        });
        callers
            .iter()
            .filter(|(r, _)| r.contains_range(range))
            .min_by_key(|(r, _)| r.len())
            .map(|(_, id)| *id)
    }
}

/// Calls (or instantiations) linking a node to others: each other node,
/// the file of the calls, and where they are in it, in order of first
/// appearance.
pub(crate) type Calls = Vec<(Node, FileId, Vec<TextRange>)>;

fn group(sites: Vec<(Node, FileId, TextRange)>) -> Calls {
    let mut out: Calls = Vec::new();
    let mut index: FxHashMap<Node, usize> = FxHashMap::default();
    for (node, file, range) in sites {
        let i = *index.entry(node).or_insert_with(|| {
            out.push((node, file, Vec::new()));
            out.len() - 1
        });
        out[i].2.push(range);
    }
    out
}

/// The callers of a subprogram, or the design entities (architectures)
/// that instantiate a design entity, with where they call it.
pub(crate) fn incoming_of(world: &World, (id, instances): Node) -> Calls {
    let analysis = &world.analysis;
    let Some(decl) = analysis.decl(id) else {
        return Vec::new();
    };
    let mut sites: Vec<(Node, FileId, TextRange)> = Vec::new();
    if instances {
        let entity = match decl.kind {
            DeclKind::Architecture => match completed(world, id) {
                Some(entity) => entity,
                None => return Vec::new(),
            },
            _ => id,
        };
        for (file, range) in instances_of(world, entity) {
            // The architecture the instance is in.
            let from = analysis.file_units(file).find_map(|unit| {
                let unit_decl = &analysis.unit_decls(unit)[0];
                (unit_decl.kind == DeclKind::Architecture
                    && unit_decl.full_range.contains_range(range))
                .then_some(DeclId { unit, index: 0 })
            });
            if let Some(from) = from {
                sites.push(((from, true), file, range));
            }
        }
    } else if is_subprogram(decl.kind) {
        let declaration = completed(world, id).unwrap_or(id);
        let mut ids: FxHashSet<DeclId> = completions(world, declaration).into_iter().collect();
        ids.insert(declaration);
        ids.insert(id);
        let mut callers = Callers::default();
        for unit in user_units(world) {
            let file = analysis.units()[unit as usize].file;
            for reference in analysis.unit_references(unit) {
                // A body's name and an `end` name name it without calling.
                if reference.access == Access::Other {
                    continue;
                }
                let calls = reference
                    .targets
                    .iter()
                    .any(|t| matches!(t, Target::Decl(t) if ids.contains(t)));
                if calls && let Some(from) = callers.find(world, file, reference.range) {
                    sites.push(((from, false), file, reference.range));
                }
            }
        }
        // A file in two libraries has its units twice.
        sites.sort_by_key(|(_, f, r)| (*f, r.start()));
        sites.dedup_by_key(|(_, f, r)| (*f, *r));
    }
    group(sites)
}

/// What a subprogram, process or unit calls, or the design entities an
/// architecture instantiates, with where.
pub(crate) fn outgoing_of(world: &World, (id, instances): Node) -> Calls {
    let analysis = &world.analysis;
    let Some(decl) = analysis.decl(id) else {
        return Vec::new();
    };
    let file = decl.file;
    let mut sites: Vec<(Node, FileId, TextRange)> = Vec::new();
    if instances {
        if decl.kind != DeclKind::Architecture {
            return Vec::new();
        }
        let root = analysis.files()[file as usize].syntax();
        for reference in analysis.unit_references(id.unit) {
            let instantiated = reference.targets.iter().find_map(|t| match t {
                Target::Decl(t)
                    if analysis.decl(*t).is_some_and(|d| {
                        matches!(d.kind, DeclKind::Entity | DeclKind::Component)
                    }) =>
                {
                    Some(*t)
                }
                _ => None,
            });
            if let Some(target) = instantiated
                && is_instantiation(&root, reference.range)
            {
                sites.push(((design_entity(world, target), true), file, reference.range));
            }
        }
    } else {
        for reference in analysis.unit_references(id.unit) {
            if reference.access == Access::Other || !decl.full_range.contains_range(reference.range)
            {
                continue;
            }
            let callee = reference.targets.iter().find_map(|t| match t {
                Target::Decl(t) if analysis.decl(*t).is_some_and(|d| is_subprogram(d.kind)) => {
                    Some(*t)
                }
                _ => None,
            });
            if let Some(callee) = callee {
                let body = completions(world, callee)
                    .first()
                    .copied()
                    .unwrap_or(callee);
                sites.push(((body, false), file, reference.range));
            }
        }
    }
    group(sites)
}

/// The node a hierarchy item stands for.
fn node_of(world: &World, item: CallHierarchyItem) -> Option<Node> {
    let anchor: Anchor = serde_json::from_value(item.data?).ok()?;
    Some((anchored(world, &anchor)?, anchor.instances))
}

/// `callHierarchy/incomingCalls`.
pub fn incoming(
    world: &mut World,
    params: CallHierarchyIncomingCallsParams,
) -> Option<Vec<CallHierarchyIncomingCall>> {
    let node = node_of(world, params.item)?;
    Some(
        incoming_of(world, node)
            .into_iter()
            .filter_map(|(from, file, ranges)| {
                let doc = world.doc(file);
                Some(CallHierarchyIncomingCall {
                    from: item(world, from)?,
                    from_ranges: ranges.into_iter().map(|r| doc.range(r)).collect(),
                })
            })
            .collect(),
    )
}

/// `callHierarchy/outgoingCalls`.
pub fn outgoing(
    world: &mut World,
    params: CallHierarchyOutgoingCallsParams,
) -> Option<Vec<CallHierarchyOutgoingCall>> {
    let node = node_of(world, params.item)?;
    Some(
        outgoing_of(world, node)
            .into_iter()
            .filter_map(|(to, file, ranges)| {
                let doc = world.doc(file);
                Some(CallHierarchyOutgoingCall {
                    to: item(world, to)?,
                    from_ranges: ranges.into_iter().map(|r| doc.range(r)).collect(),
                })
            })
            .collect(),
    )
}

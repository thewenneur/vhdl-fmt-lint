//! The design hierarchy: the `vfl/hierarchy` request, for the hierarchy
//! views of the editors.
//!
//! The instantiation tree is read off the analysis on demand, never as
//! part of an edit: each architecture's instantiation statements are found
//! in its syntax tree, and what each instantiates is the declaration the
//! analysis resolved its name to (an entity, a component, a configuration
//! or a Verilog module). A component binds to the entity of the same name,
//! in the instantiating unit's library first. An entity without a named
//! architecture uses its last one, as VHDL binds the most recently
//! analyzed. Generate statements and blocks become grouping nodes.
//!
//! The tree is bounded: an entity that instantiates itself further up is
//! not expanded again, and the depth and number of nodes are capped.

use std::path::PathBuf;
use std::rc::Rc;

use rustc_hash::{FxHashMap, FxHashSet};
use serde::{Deserialize, Serialize};
use vfl_sema::{DeclId, DeclKind, FileId, Target, UnitId};
use vfl_syntax::{SyntaxKind, SyntaxNode, TextRange, identifier_key};

use crate::world::World;

/// Levels of instances below a top, at most.
const MAX_DEPTH: usize = 64;
/// Nodes in a whole answer, at most: a block instantiated many times in
/// many places multiplies quickly.
const MAX_NODES: usize = 20_000;

/// `vfl/hierarchy` parameters.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HierarchyParams {
    /// The top entity, as `name` or `library.name`. Without one, the tree
    /// starts from every entity no other entity instantiates.
    #[serde(default)]
    pub top: Option<String>,
    /// Without a top, whether testbenches (entities without ports) are
    /// tops. When they are not, what only testbenches instantiate is.
    #[serde(default = "yes")]
    pub testbenches: bool,
}

impl Default for HierarchyParams {
    fn default() -> Self {
        HierarchyParams {
            top: None,
            testbenches: true,
        }
    }
}

fn yes() -> bool {
    true
}

/// What a node of the tree is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum NodeKind {
    /// A top.
    Entity,
    /// An instantiation statement.
    Instance,
    /// A `for`, `if` or `case` generate statement, holding the instances
    /// inside it.
    Generate,
    /// A block statement, holding the instances inside it.
    Block,
}

/// What an instance names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum UnitKind {
    Entity,
    Component,
    Configuration,
    /// A Verilog or SystemVerilog module.
    Module,
}

/// A node of the instantiation tree. `P` is a place in a file: an LSP
/// location for the language server, a byte range for editors that link
/// vfl directly.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Node<P> {
    pub kind: NodeKind,
    /// The instance or statement label; a top's entity name.
    pub label: String,
    /// The entity, component, configuration or module, as written in its
    /// declaration when it is found.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unit_kind: Option<UnitKind>,
    /// The library of the entity, or as written when it is not found.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub library: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub architecture: Option<String>,
    /// The scheme of a generate statement: `for i in 0 to 3`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// The instance's (or statement's) label; a top's entity name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location: Option<P>,
    /// The name of the entity (or module) declaration.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entity_location: Option<P>,
    /// The name of the architecture body.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub architecture_location: Option<P>,
    /// The instance names nothing that was found, or a component no
    /// entity binds to.
    #[serde(skip_serializing_if = "is_false")]
    pub unresolved: bool,
    /// The entity instantiates itself further up: not expanded again.
    #[serde(skip_serializing_if = "is_false")]
    pub recursive: bool,
    /// Left unexpanded: the tree is too deep or too large.
    #[serde(skip_serializing_if = "is_false")]
    pub truncated: bool,
    /// A top without ports.
    #[serde(skip_serializing_if = "is_false")]
    pub testbench: bool,
    pub children: Vec<Node<P>>,
}

fn is_false(b: &bool) -> bool {
    !*b
}

impl<P> Node<P> {
    /// The same tree with its places converted; a place `f` cannot convert
    /// is left out.
    pub fn map<Q>(self, f: &mut impl FnMut(P) -> Option<Q>) -> Node<Q> {
        Node {
            kind: self.kind,
            label: self.label,
            name: self.name,
            unit_kind: self.unit_kind,
            library: self.library,
            architecture: self.architecture,
            detail: self.detail,
            location: self.location.and_then(&mut *f),
            entity_location: self.entity_location.and_then(&mut *f),
            architecture_location: self.architecture_location.and_then(&mut *f),
            unresolved: self.unresolved,
            recursive: self.recursive,
            truncated: self.truncated,
            testbench: self.testbench,
            children: self.children.into_iter().map(|c| c.map(f)).collect(),
        }
    }
}

/// An entity of the project, to choose a top from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Candidate {
    pub library: String,
    pub name: String,
    pub path: PathBuf,
    /// No other entity instantiates it.
    pub top: bool,
    /// It has no ports.
    pub testbench: bool,
}

/// The answer to `vfl/hierarchy`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Hierarchy<P> {
    pub roots: Vec<Node<P>>,
    /// The project's VHDL entities, sorted by library and name.
    pub entities: Vec<Candidate>,
    /// Why there is no tree, such as a top that is not in the project.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// A place in an analyzed file.
pub type Place = (FileId, TextRange);

/// The hierarchy of the project, with places as file and range.
pub fn hierarchy(world: &mut World, params: &HierarchyParams) -> Hierarchy<Place> {
    world.refresh();
    let mut builder = Builder::new(world);
    let entities = builder.user_entities();
    let testbench: FxHashSet<UnitId> = entities
        .iter()
        .copied()
        .filter(|&e| builder.is_testbench(e))
        .collect();
    // What each entity's architectures instantiate, testbenches' left out
    // unless they are tops.
    let mut instantiated: FxHashSet<UnitId> = FxHashSet::default();
    for &entity in &entities {
        if !params.testbenches && testbench.contains(&entity) {
            continue;
        }
        for arch in builder.architectures(entity) {
            for target in builder.instances(arch).targets() {
                if target != entity {
                    instantiated.insert(target);
                }
            }
        }
    }
    let analysis = &world.analysis;
    let mut candidates: Vec<(Candidate, UnitId)> = entities
        .iter()
        .map(|&e| {
            let unit = &analysis.units()[e as usize];
            (
                Candidate {
                    library: unit.library.clone(),
                    name: builder.spelling(e),
                    path: analysis.files()[unit.file as usize].path.clone(),
                    top: !instantiated.contains(&e),
                    testbench: testbench.contains(&e),
                },
                e,
            )
        })
        .collect();
    candidates.sort_by_key(|(c, _)| (c.library.to_lowercase(), c.name.to_lowercase()));

    let mut message = None;
    let tops: Vec<UnitId> = match params.top.as_deref().map(str::trim) {
        Some(top) if !top.is_empty() => {
            let (library, name) = match top.rsplit_once('.') {
                Some((l, n)) => (Some(identifier_key(l)), identifier_key(n)),
                None => (None, identifier_key(top)),
            };
            let found: Vec<UnitId> = candidates
                .iter()
                .map(|(_, e)| *e)
                .filter(|&e| {
                    let unit = &analysis.units()[e as usize];
                    unit.name == name && library.as_ref().is_none_or(|l| *l == unit.library)
                })
                .collect();
            if found.is_empty() {
                message = Some(format!("no entity `{top}` in the project"));
            }
            found
        }
        _ => candidates
            .iter()
            .filter(|(c, _)| c.top && (params.testbenches || !c.testbench))
            .map(|(_, e)| *e)
            .collect(),
    };
    let roots = tops
        .into_iter()
        .map(|e| {
            let mut root = builder.entity_node(e, None);
            root.testbench = testbench.contains(&e);
            root
        })
        .collect();
    Hierarchy {
        roots,
        entities: candidates.into_iter().map(|(c, _)| c).collect(),
        message,
    }
}

/// `vfl/hierarchy`: the hierarchy with LSP locations.
pub fn request(world: &mut World, params: HierarchyParams) -> Hierarchy<lsp_types::Location> {
    let tree = hierarchy(world, &params);
    let mut paths: FxHashMap<FileId, Option<lsp_types::Url>> = FxHashMap::default();
    let world = &*world;
    let mut place = |(file, range): Place| {
        let uri = paths
            .entry(file)
            .or_insert_with(|| {
                crate::handlers::openable_path(world, file)
                    .and_then(|p| lsp_types::Url::from_file_path(p).ok())
            })
            .clone()?;
        Some(lsp_types::Location {
            uri,
            range: world.doc(file).range(range),
        })
    };
    Hierarchy {
        roots: tree.roots.into_iter().map(|n| n.map(&mut place)).collect(),
        entities: tree.entities,
        message: tree.message,
    }
}

/// A concurrent statement of an architecture that matters to the tree.
#[derive(Debug)]
enum Item {
    Instance(Instance),
    Group {
        kind: NodeKind,
        label: String,
        detail: Option<String>,
        range: Option<TextRange>,
        items: Vec<Item>,
    },
}

#[derive(Debug)]
struct Instance {
    label: String,
    label_range: Option<TextRange>,
    /// The name as written, without its library.
    written: String,
    written_library: Option<String>,
    unit_kind: UnitKind,
    /// The entity it binds to, if found.
    entity: Option<UnitId>,
    /// The architecture it names, or its configuration does.
    architecture: Option<String>,
    /// The module it instantiates, as a declaration.
    module: Option<DeclId>,
}

/// The instances of one architecture, in source order.
#[derive(Debug, Default)]
struct Instances {
    file: FileId,
    items: Vec<Item>,
}

impl Instances {
    /// The entities instantiated, at any depth of generate statements.
    fn targets(&self) -> Vec<UnitId> {
        fn walk(items: &[Item], out: &mut Vec<UnitId>) {
            for item in items {
                match item {
                    Item::Instance(i) => out.extend(i.entity),
                    Item::Group { items, .. } => walk(items, out),
                }
            }
        }
        let mut out = Vec::new();
        walk(&self.items, &mut out);
        out
    }
}

struct Builder<'a> {
    world: &'a World,
    /// (library, name) -> entity units, in unit order.
    entities: FxHashMap<(&'a str, &'a str), Vec<UnitId>>,
    /// (library, entity) -> architecture units, in unit order.
    architectures: FxHashMap<(&'a str, &'a str), Vec<UnitId>>,
    /// The instances of each architecture, found once per request.
    instances: FxHashMap<UnitId, Rc<Instances>>,
    nodes: usize,
}

impl<'a> Builder<'a> {
    fn new(world: &'a World) -> Self {
        let mut entities: FxHashMap<(&str, &str), Vec<UnitId>> = FxHashMap::default();
        let mut architectures: FxHashMap<(&str, &str), Vec<UnitId>> = FxHashMap::default();
        for (id, unit) in world.analysis.units().iter().enumerate() {
            let key = (unit.library.as_str(), unit.name.as_str());
            match unit.kind {
                DeclKind::Entity => entities.entry(key).or_default().push(id as UnitId),
                DeclKind::Architecture => {
                    if let Some(entity) = &unit.primary {
                        architectures
                            .entry((unit.library.as_str(), entity.as_str()))
                            .or_default()
                            .push(id as UnitId);
                    }
                }
                _ => {}
            }
        }
        Builder {
            world,
            entities,
            architectures,
            instances: FxHashMap::default(),
            nodes: 0,
        }
    }

    /// The entities of the user's VHDL files.
    fn user_entities(&self) -> Vec<UnitId> {
        let analysis = &self.world.analysis;
        let user = analysis.user_files().len();
        analysis
            .units()
            .iter()
            .enumerate()
            .filter(|(_, u)| {
                u.kind == DeclKind::Entity && (u.file as usize) < user && self.world.is_vhdl(u.file)
            })
            .map(|(i, _)| i as UnitId)
            .collect()
    }

    fn entity_decl(&self, entity: UnitId) -> Option<&'a vfl_sema::Decl> {
        self.world.analysis.decl(DeclId {
            unit: entity,
            index: 0,
        })
    }

    fn spelling(&self, unit: UnitId) -> String {
        match self.entity_decl(unit) {
            Some(decl) => decl.spelling.clone(),
            None => self.world.analysis.units()[unit as usize].name.clone(),
        }
    }

    fn is_testbench(&self, entity: UnitId) -> bool {
        let analysis = &self.world.analysis;
        let Some(members) = self.entity_decl(entity).and_then(|d| d.members.as_ref()) else {
            return true;
        };
        !members.iter().any(|id| {
            analysis
                .decl(id)
                .is_some_and(|d| matches!(d.kind, DeclKind::Port { .. }))
        })
    }

    /// The architectures of an entity, in unit order.
    fn architectures(&self, entity: UnitId) -> Vec<UnitId> {
        let unit = &self.world.analysis.units()[entity as usize];
        self.architectures
            .get(&(unit.library.as_str(), unit.name.as_str()))
            .cloned()
            .unwrap_or_default()
    }

    /// The architecture `name` of an entity, else its last one.
    fn architecture(&self, entity: UnitId, name: Option<&str>) -> Option<UnitId> {
        let architectures = self.architectures(entity);
        match name {
            Some(name) => {
                let key = identifier_key(name);
                architectures
                    .into_iter()
                    .find(|&a| self.world.analysis.units()[a as usize].name == key)
            }
            None => architectures.last().copied(),
        }
    }

    /// The entity `name` of `library`, else of any library.
    fn find_entity(&self, library: &str, name: &str) -> Option<UnitId> {
        if let Some(units) = self.entities.get(&(library, name)) {
            return units.first().copied();
        }
        let user = self.world.analysis.user_files().len();
        let units = self.world.analysis.units();
        let mut found: Vec<UnitId> = self
            .entities
            .iter()
            .filter(|((_, n), _)| *n == name)
            .flat_map(|(_, units)| units.iter().copied())
            .collect();
        // The user's own files first, then in unit order.
        found.sort_by_key(|&u| ((units[u as usize].file as usize) >= user, u));
        found.first().copied()
    }

    /// The syntax node of a unit: an `ARCHITECTURE_BODY`, say.
    fn unit_node(&self, unit: UnitId) -> Option<SyntaxNode> {
        let info = &self.world.analysis.units()[unit as usize];
        let file = &self.world.analysis.files()[info.file as usize];
        if file.foreign.is_some() {
            return None;
        }
        file.syntax()
            .children()
            .filter(|n| n.kind() == SyntaxKind::DESIGN_UNIT)
            .nth(info.index)?
            .children()
            .find(|n| n.kind() != SyntaxKind::CONTEXT_CLAUSE)
    }

    fn instances(&mut self, architecture: UnitId) -> Rc<Instances> {
        if let Some(found) = self.instances.get(&architecture) {
            return found.clone();
        }
        let file = self.world.analysis.units()[architecture as usize].file;
        let items = match self.unit_node(architecture) {
            Some(node) => self.items(architecture, &node),
            None => Vec::new(),
        };
        let found = Rc::new(Instances { file, items });
        self.instances.insert(architecture, found.clone());
        found
    }

    /// The instances and groups among the statements under `node`.
    fn items(&self, architecture: UnitId, node: &SyntaxNode) -> Vec<Item> {
        let mut out = Vec::new();
        for child in node.children() {
            match child.kind() {
                SyntaxKind::COMPONENT_INSTANTIATION_STMT => {
                    out.push(Item::Instance(self.instance(architecture, &child)));
                }
                SyntaxKind::FOR_GENERATE_STMT
                | SyntaxKind::IF_GENERATE_STMT
                | SyntaxKind::CASE_GENERATE_STMT
                | SyntaxKind::BLOCK_STMT => {
                    let items = self.items(architecture, &child);
                    if items.is_empty() {
                        continue;
                    }
                    let label = label(&child);
                    let (kind, detail) = if child.kind() == SyntaxKind::BLOCK_STMT {
                        (NodeKind::Block, None)
                    } else {
                        (NodeKind::Generate, scheme(&child))
                    };
                    out.push(Item::Group {
                        kind,
                        label: label.as_ref().map_or_else(String::new, |(l, _)| l.clone()),
                        detail,
                        range: label.map(|(_, r)| r),
                        items,
                    });
                }
                // The alternatives of `if` and `case` generate statements.
                SyntaxKind::GENERATE_BODY | SyntaxKind::CASE_ALTERNATIVE => {
                    out.extend(self.items(architecture, &child));
                }
                _ => {}
            }
        }
        out
    }

    fn instance(&self, unit: UnitId, node: &SyntaxNode) -> Instance {
        let (label, label_range) = match self::label(node) {
            Some((l, r)) => (l, Some(r)),
            None => (String::new(), None),
        };
        let has = |kind| {
            node.children_with_tokens()
                .any(|e| e.as_token().is_some_and(|t| t.kind() == kind))
        };
        let mut unit_kind = if has(SyntaxKind::ENTITY_KW) {
            UnitKind::Entity
        } else if has(SyntaxKind::CONFIGURATION_KW) {
            UnitKind::Configuration
        } else {
            UnitKind::Component
        };
        // `work.e`, or `work.e(arch)`.
        let target = node.children().find(|n| {
            matches!(
                n.kind(),
                SyntaxKind::NAME | SyntaxKind::SELECTED_NAME | SyntaxKind::CALL_EXPR
            )
        });
        let (name, mut architecture) = match &target {
            Some(t) if t.kind() == SyntaxKind::CALL_EXPR => {
                let mut parts = t.children();
                let name = parts.next();
                let arch = parts.next().and_then(|a| first_ident(&a));
                (name, arch)
            }
            Some(t) => (Some(t.clone()), None),
            None => (None, None),
        };
        let idents: Vec<String> = name
            .iter()
            .flat_map(|n| n.descendants_with_tokens())
            .filter_map(|e| e.into_token())
            .filter(|t| matches!(t.kind(), SyntaxKind::IDENT | SyntaxKind::EXT_IDENT))
            .map(|t| t.text().to_owned())
            .collect();
        let written = idents.last().cloned().unwrap_or_default();
        let written_library = (idents.len() > 1).then(|| idents[0].clone());

        let analysis = &self.world.analysis;
        let library = analysis.units()[unit as usize].library.as_str();
        let decl = name.and_then(|n| self.resolved(unit, n.text_range()));
        let mut entity = None;
        let mut module = None;
        if let Some((id, decl)) = decl {
            match decl.kind {
                DeclKind::Entity if analysis.foreign_modules(decl.file).is_some() => {
                    unit_kind = UnitKind::Module;
                    module = Some(id);
                }
                DeclKind::Entity => entity = Some(id.unit),
                DeclKind::Component => entity = self.find_entity(library, &decl.name),
                DeclKind::Configuration => {
                    let config_library = analysis.units()[id.unit as usize].library.as_str();
                    if let Some((of, arch)) = self.configured(id.unit) {
                        entity = self.find_entity(config_library, &of);
                        architecture = architecture.or(arch);
                    }
                }
                _ => {}
            }
        }
        Instance {
            label,
            label_range,
            written,
            written_library,
            unit_kind,
            entity,
            architecture,
            module,
        }
    }

    /// The entity, component, configuration or module the name at `range`
    /// of a unit was resolved to.
    fn resolved(&self, unit: UnitId, range: TextRange) -> Option<(DeclId, &'a vfl_sema::Decl)> {
        let analysis = &self.world.analysis;
        let refs = analysis.unit_references(unit);
        let start = refs.partition_point(|r| r.range.start() < range.start());
        refs[start..]
            .iter()
            .take_while(|r| r.range.end() <= range.end())
            .flat_map(|r| r.targets.iter())
            .filter_map(|t| match t {
                Target::Decl(id) => Some((*id, analysis.decl(*id)?)),
                Target::Library(_) => None,
            })
            .filter(|(_, d)| {
                matches!(
                    d.kind,
                    DeclKind::Entity | DeclKind::Component | DeclKind::Configuration
                )
            })
            .last()
    }

    /// The entity of a configuration, and the architecture its block
    /// configuration names.
    fn configured(&self, configuration: UnitId) -> Option<(String, Option<String>)> {
        let node = self.unit_node(configuration)?;
        let entity = node
            .children()
            .find(|n| matches!(n.kind(), SyntaxKind::NAME | SyntaxKind::SELECTED_NAME))
            .and_then(|n| last_ident(&n))?;
        let architecture = node
            .children()
            .find(|n| n.kind() == SyntaxKind::BLOCK_CONFIGURATION)
            .and_then(|b| first_ident(&b));
        Some((identifier_key(&entity), architecture))
    }

    /// The node of a top, or of an instance of `entity` under `stack`.
    fn entity_node(&mut self, entity: UnitId, architecture: Option<&str>) -> Node<Place> {
        let mut node = self.blank(NodeKind::Entity, self.spelling(entity));
        node.location = self.entity_place(entity);
        self.fill(&mut node, entity, architecture, &mut Vec::new());
        node
    }

    fn blank(&mut self, kind: NodeKind, label: String) -> Node<Place> {
        self.nodes += 1;
        Node {
            kind,
            label,
            name: None,
            unit_kind: None,
            library: None,
            architecture: None,
            detail: None,
            location: None,
            entity_location: None,
            architecture_location: None,
            unresolved: false,
            recursive: false,
            truncated: false,
            testbench: false,
            children: Vec::new(),
        }
    }

    fn entity_place(&self, entity: UnitId) -> Option<Place> {
        let unit = &self.world.analysis.units()[entity as usize];
        Some((unit.file, unit.name_range))
    }

    /// Fills in what `entity` is and, unless it recurses or the tree is too
    /// large, the instances of its architecture.
    fn fill(
        &mut self,
        node: &mut Node<Place>,
        entity: UnitId,
        architecture: Option<&str>,
        stack: &mut Vec<UnitId>,
    ) {
        let units = self.world.analysis.units();
        node.name = Some(self.spelling(entity));
        node.unit_kind.get_or_insert(UnitKind::Entity);
        node.library = Some(units[entity as usize].library.clone());
        node.entity_location = self.entity_place(entity);
        let arch = self.architecture(entity, architecture);
        match arch {
            Some(a) => {
                let unit = &units[a as usize];
                node.architecture =
                    Some(self.world.doc(unit.file).text[unit.name_range].to_owned());
                node.architecture_location = Some((unit.file, unit.name_range));
            }
            None => node.architecture = architecture.map(str::to_owned),
        }
        let Some(arch) = arch else { return };
        if stack.contains(&entity) {
            node.recursive = true;
            return;
        }
        let instances = self.instances(arch);
        if instances.items.is_empty() {
            return;
        }
        if stack.len() >= MAX_DEPTH || self.nodes >= MAX_NODES {
            node.truncated = true;
            return;
        }
        stack.push(entity);
        node.children = self.children(instances.file, &instances.items, stack);
        stack.pop();
    }

    fn children(
        &mut self,
        file: FileId,
        items: &[Item],
        stack: &mut Vec<UnitId>,
    ) -> Vec<Node<Place>> {
        let mut out = Vec::with_capacity(items.len());
        for item in items {
            match item {
                Item::Group {
                    kind,
                    label,
                    detail,
                    range,
                    items,
                } => {
                    let mut node = self.blank(*kind, label.clone());
                    node.detail = detail.clone();
                    node.location = range.map(|r| (file, r));
                    node.children = self.children(file, items, stack);
                    out.push(node);
                }
                Item::Instance(i) => {
                    let mut node = self.blank(NodeKind::Instance, i.label.clone());
                    node.unit_kind = Some(i.unit_kind);
                    node.location = i.label_range.map(|r| (file, r));
                    match (i.entity, i.module) {
                        (Some(entity), _) => {
                            self.fill(&mut node, entity, i.architecture.as_deref(), stack);
                        }
                        (None, Some(module)) => {
                            let decl = self.world.analysis.decl(module);
                            node.name = decl.map(|d| d.spelling.clone());
                            node.library = Some(
                                self.world.analysis.units()[module.unit as usize]
                                    .library
                                    .clone(),
                            );
                            node.entity_location = decl.map(|d| (d.file, d.range));
                        }
                        (None, None) => {
                            node.name = Some(i.written.clone());
                            node.library = i.written_library.clone();
                            node.architecture = i.architecture.clone();
                            node.unresolved = true;
                        }
                    }
                    out.push(node);
                }
            }
        }
        out
    }
}

/// A statement's label and its range.
fn label(node: &SyntaxNode) -> Option<(String, TextRange)> {
    let label = node.children().find(|n| n.kind() == SyntaxKind::LABEL)?;
    let token = label
        .children_with_tokens()
        .filter_map(|e| e.into_token())
        .find(|t| matches!(t.kind(), SyntaxKind::IDENT | SyntaxKind::EXT_IDENT))?;
    Some((token.text().to_owned(), token.text_range()))
}

fn first_ident(node: &SyntaxNode) -> Option<String> {
    node.descendants_with_tokens()
        .filter_map(|e| e.into_token())
        .find(|t| matches!(t.kind(), SyntaxKind::IDENT | SyntaxKind::EXT_IDENT))
        .map(|t| t.text().to_owned())
}

fn last_ident(node: &SyntaxNode) -> Option<String> {
    node.descendants_with_tokens()
        .filter_map(|e| e.into_token())
        .filter(|t| matches!(t.kind(), SyntaxKind::IDENT | SyntaxKind::EXT_IDENT))
        .last()
        .map(|t| t.text().to_owned())
}

/// A generate statement's scheme, from after its label to `generate`, on
/// one line: `for i in 0 to 3`.
fn scheme(node: &SyntaxNode) -> Option<String> {
    let mut text = String::new();
    let mut started = false;
    for element in node.children_with_tokens() {
        match element.kind() {
            SyntaxKind::LABEL if !started => continue,
            SyntaxKind::GENERATE_KW => break,
            _ => {}
        }
        started = true;
        text.push_str(&element.to_string());
        text.push(' ');
    }
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    const MAX: usize = 60;
    match text.char_indices().nth(MAX) {
        Some((i, _)) => Some(format!("{}…", &text[..i])),
        None => (!text.is_empty()).then_some(text),
    }
}

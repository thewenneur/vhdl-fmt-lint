//! The documentation model: what the generated pages show, read off the
//! syntax trees of a project.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use vfl_config::DocConfig;
use vfl_syntax::{LineIndex, SyntaxKind, SyntaxNode, SyntaxToken, identifier_key};

use crate::comments::CommentStyle;
use crate::fsm::{self, Fsm};

/// A documented design.
#[derive(Debug, Clone, Default)]
pub struct Design {
    pub title: String,
    pub entities: Vec<Entity>,
    pub packages: Vec<Package>,
    /// Configurations, for instances of them: (library, name key, entity key).
    pub configurations: Vec<(String, String, String)>,
    /// Entity key -> indexes into `entities`, for resolving instances.
    pub by_name: HashMap<String, Vec<usize>>,
    /// (library key, name key) -> the first package of that name, for
    /// resolving `use` clauses.
    pub packages_by_name: HashMap<(String, String), usize>,
}

/// An entity and its architectures.
#[derive(Debug, Clone, Default)]
pub struct Entity {
    /// The id of its page (the file stem): `library.name`, made unique.
    pub page: String,
    /// The name as written.
    pub name: String,
    pub library: String,
    /// The file, relative to the project root, with `/` separators.
    pub file: String,
    /// The 1-based line of the declaration.
    pub line: u32,
    pub description: String,
    pub generics: Vec<Interface>,
    pub ports: Vec<Interface>,
    pub architectures: Vec<Architecture>,
    /// The packages its context clauses (and its architectures') use.
    pub uses: Vec<Use>,
}

/// A `use library.unit` of a context clause, keyed.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Use {
    /// The library key, `work` resolved to the unit's own library.
    pub library: String,
    pub unit: String,
}

impl Entity {
    /// An entity without ports, usually a testbench.
    pub fn is_testbench(&self) -> bool {
        self.ports.is_empty()
    }

    pub fn key(&self) -> String {
        identifier_key(&self.name)
    }
}

/// A generic or a port.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Interface {
    pub name: String,
    /// `in`, `out`, `inout`, `buffer`, `linkage` or `view` for ports; empty
    /// for generics and ports without a mode (which are `in`).
    pub mode: String,
    /// The subtype as written, or `type`, `package`, `function`, ... for
    /// generics that are not constants.
    pub ty: String,
    pub default: Option<String>,
    pub description: String,
}

impl Interface {
    /// The direction of a port, for drawing.
    pub fn direction(&self) -> Direction {
        match self.mode.as_str() {
            "out" | "buffer" => Direction::Out,
            "inout" | "linkage" | "view" => Direction::InOut,
            _ => Direction::In,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    In,
    Out,
    InOut,
}

/// An architecture body.
#[derive(Debug, Clone, Default)]
pub struct Architecture {
    pub name: String,
    pub file: String,
    pub line: u32,
    pub description: String,
    pub signals: Vec<Object>,
    pub constants: Vec<Object>,
    pub types: Vec<TypeInfo>,
    pub subprograms: Vec<Subprogram>,
    pub processes: Vec<Process>,
    pub instances: Vec<Instance>,
    pub fsms: Vec<Fsm>,
    /// Component declarations, for the ports of their instances.
    pub components: Vec<Component>,
    /// Concurrent signal assignments.
    pub assignments: Vec<Assignment>,
    pub aliases: Vec<Alias>,
    /// Packages its own context clause uses.
    pub uses: Vec<Use>,
}

impl Architecture {
    /// Names with their aliases replaced by the aliased objects.
    pub fn resolve_aliases(&self, names: &[String]) -> Vec<String> {
        let mut out: Vec<String> = Vec::with_capacity(names.len());
        for name in names {
            let key = identifier_key(name);
            let resolved = self
                .aliases
                .iter()
                .find(|a| identifier_key(&a.name) == key)
                .map_or(name, |a| &a.target);
            if !out
                .iter()
                .any(|o| identifier_key(o) == identifier_key(resolved))
            {
                out.push(resolved.clone());
            }
        }
        out
    }
}

/// A concurrent signal assignment: simple, conditional or selected.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Assignment {
    pub label: Option<String>,
    pub line: u32,
    /// The assigned signal.
    pub target: String,
    /// The names it reads, as written (signals or not: callers filter).
    pub reads: Vec<String>,
}

/// An object alias: `alias en : std_logic is ctrl(0);`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Alias {
    pub name: String,
    pub ty: String,
    /// The aliased object.
    pub target: String,
    /// The index or range of the target, as written (`0`, `2 downto 1`).
    pub slice: Option<String>,
    pub description: String,
}

/// A signal, constant or other object declaration.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Object {
    pub name: String,
    pub ty: String,
    pub value: Option<String>,
    pub description: String,
}

/// A type or subtype declaration.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TypeInfo {
    pub name: String,
    /// `type` or `subtype`.
    pub kind: &'static str,
    /// The definition as written, on one line.
    pub definition: String,
    pub description: String,
    /// The elements of a record type: name, subtype and description.
    pub fields: Vec<(String, String, String)>,
}

/// A function or procedure.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Subprogram {
    pub name: String,
    /// The specification, on one line.
    pub signature: String,
    pub description: String,
}

/// A process statement.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Process {
    pub label: Option<String>,
    pub line: u32,
    /// The sensitivity list as written (`all` for `process (all)`), empty
    /// for processes with wait statements.
    pub sensitivity: Vec<String>,
    /// The clock of a clocked process (`clk` for `rising_edge(clk)`).
    pub clock: Option<String>,
    /// Whether the clock edge is a falling one.
    pub falling: bool,
    /// The reset of a clocked process.
    pub reset: Option<Reset>,
    /// The signals the process assigns.
    pub drives: Vec<String>,
    /// The names the process reads, as written (signals or not: callers
    /// filter).
    pub reads: Vec<String>,
    pub description: String,
}

/// The reset of a clocked process.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Reset {
    pub signal: String,
    /// Tested outside the clock edge.
    pub asynchronous: bool,
    /// `Some(true)` for active high, `Some(false)` for active low.
    pub active_high: Option<bool>,
}

/// A component, entity or configuration instantiation.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Instance {
    pub label: String,
    /// Labels of the generate and block statements around it, outermost
    /// first.
    pub scope: Vec<String>,
    pub kind: InstanceKind,
    /// The library as written; `None` for a component or a plain name.
    pub library: Option<String>,
    /// The entity, component or configuration name as written.
    pub unit: String,
    pub architecture: Option<String>,
    pub generic_map: Vec<(String, String)>,
    pub port_map: Vec<(String, String)>,
    pub description: String,
}

impl Instance {
    /// The label with the labels of its scope: `gen.u_fifo`.
    pub fn path(&self) -> String {
        let mut parts = self.scope.clone();
        parts.push(self.label.clone());
        parts.join(".")
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum InstanceKind {
    #[default]
    Component,
    Entity,
    Configuration,
}

/// A package with its body's description merged in.
#[derive(Debug, Clone, Default)]
pub struct Package {
    /// The id of its page (the file stem): `library.name`, made unique.
    pub page: String,
    pub name: String,
    pub library: String,
    pub file: String,
    pub line: u32,
    pub description: String,
    pub constants: Vec<Object>,
    pub signals: Vec<Object>,
    pub types: Vec<TypeInfo>,
    pub subprograms: Vec<Subprogram>,
    pub components: Vec<Component>,
    pub aliases: Vec<Alias>,
    pub uses: Vec<Use>,
}

/// A component declaration.
#[derive(Debug, Clone, Default)]
pub struct Component {
    pub name: String,
    pub description: String,
    pub generics: Vec<Interface>,
    pub ports: Vec<Interface>,
}

/// One file of the project, ready to be documented.
pub struct FileInput<'a> {
    pub root: &'a SyntaxNode,
    pub text: &'a str,
    /// Relative to the project root, with `/` separators.
    pub path: String,
    pub library: String,
}

/// Builds the documentation model of `files`.
pub fn build(files: &[FileInput], config: &DocConfig, title: String) -> Design {
    let mut design = Design {
        title,
        ..Design::default()
    };
    // Enumeration types of packages, for state machines whose state type is
    // declared in a package.
    let mut enums = fsm::StateTypes::default();
    for file in files {
        for node in file.root.descendants() {
            if node.kind() == SyntaxKind::PACKAGE_DECL {
                fsm::collect_types(&node, &mut enums);
            }
        }
    }
    let mut architectures: Vec<(String, String, Architecture)> = Vec::new();
    let mut package_bodies: Vec<(String, String, String, Vec<Use>)> = Vec::new();
    for file in files {
        let style = CommentStyle::for_file(file.root, config.comments, &config.comment_marker);
        let ctx = Ctx {
            style: &style,
            text: file.text,
            lines: LineIndex::new(file.text),
            file: &file.path,
            config,
        };
        for unit in file.root.children() {
            if unit.kind() != SyntaxKind::DESIGN_UNIT {
                continue;
            }
            let uses = unit
                .children()
                .find(|n| n.kind() == SyntaxKind::CONTEXT_CLAUSE)
                .map(|c| uses_of(&c, &file.library))
                .unwrap_or_default();
            for node in unit.children() {
                match node.kind() {
                    SyntaxKind::ENTITY_DECL => {
                        let mut entity = ctx.entity(&node);
                        entity.library = file.library.clone();
                        entity.uses = uses.clone();
                        design.entities.push(entity);
                    }
                    SyntaxKind::ARCHITECTURE_BODY => {
                        let entity = node
                            .children()
                            .find(|n| n.kind() == SyntaxKind::NAME)
                            .and_then(|n| ident(&n))
                            .map(|t| identifier_key(t.text()))
                            .unwrap_or_default();
                        let mut arch = ctx.architecture(&node, &enums);
                        arch.uses = uses.clone();
                        architectures.push((file.library.clone(), entity, arch));
                    }
                    SyntaxKind::CONFIGURATION_DECL => {
                        let entity = node
                            .children()
                            .find(|n| n.kind() == SyntaxKind::NAME)
                            .and_then(|n| ident(&n))
                            .map(|t| identifier_key(t.text()));
                        if let (Some(name), Some(entity)) = (own_name(&node), entity) {
                            design.configurations.push((
                                file.library.clone(),
                                identifier_key(&name),
                                entity,
                            ));
                        }
                    }
                    SyntaxKind::PACKAGE_DECL => {
                        let mut package = ctx.package(&node);
                        package.library = file.library.clone();
                        package.uses = uses.clone();
                        design.packages.push(package);
                    }
                    SyntaxKind::PACKAGE_BODY => {
                        let name = node
                            .children_with_tokens()
                            .filter_map(|e| e.into_token())
                            .find(|t| matches!(t.kind(), SyntaxKind::IDENT | SyntaxKind::EXT_IDENT))
                            .map(|t| identifier_key(t.text()))
                            .unwrap_or_default();
                        let description = style.doc(&node);
                        package_bodies.push((
                            file.library.clone(),
                            name,
                            description,
                            uses.clone(),
                        ));
                    }
                    _ => {}
                }
            }
        }
    }
    let mut by_name: HashMap<String, Vec<usize>> = HashMap::new();
    for (i, e) in design.entities.iter().enumerate() {
        by_name.entry(e.key()).or_default().push(i);
    }
    for (library, entity, arch) in architectures {
        let target = by_name
            .get(&entity)
            .into_iter()
            .flatten()
            .copied()
            .min_by_key(|&i| design.entities[i].library != library);
        if let Some(target) = target {
            let entity = &mut design.entities[target];
            for u in &arch.uses {
                if !entity.uses.contains(u) {
                    entity.uses.push(u.clone());
                }
            }
            entity.architectures.push(arch);
        }
    }
    for (library, name, description, uses) in package_bodies {
        if let Some(package) = design
            .packages
            .iter_mut()
            .find(|p| p.library == library && identifier_key(&p.name) == name)
        {
            if package.description.is_empty() {
                package.description = description;
            }
            for u in uses {
                if !package.uses.contains(&u) {
                    package.uses.push(u);
                }
            }
        }
    }
    if !config.testbenches {
        design.entities.retain(|e| !e.is_testbench());
    }
    let sort_key = |name: &str, library: &str| (identifier_key(name), library.to_owned());
    design
        .entities
        .sort_by_key(|e| sort_key(&e.name, &e.library));
    design
        .packages
        .sort_by_key(|p| sort_key(&p.name, &p.library));
    // Units of the same name in several files (variants for different
    // tools, say) each get their own page.
    let mut taken = HashSet::from(["index".to_owned()]);
    let mut unique = |library: &str, name: &str| {
        let base = crate::pages::page_id(library, name);
        let mut id = base.clone();
        let mut n = 2;
        while !taken.insert(id.clone()) {
            id = format!("{base}-{n}");
            n += 1;
        }
        id
    };
    for e in &mut design.entities {
        e.page = unique(&e.library, &e.name);
    }
    for p in &mut design.packages {
        p.page = unique(&p.library, &p.name);
    }
    for (i, e) in design.entities.iter().enumerate() {
        design.by_name.entry(e.key()).or_default().push(i);
    }
    for (i, p) in design.packages.iter().enumerate() {
        design
            .packages_by_name
            .entry((identifier_key(&p.library), identifier_key(&p.name)))
            .or_insert(i);
    }
    design
}

struct Ctx<'a> {
    style: &'a CommentStyle,
    text: &'a str,
    lines: LineIndex,
    file: &'a str,
    config: &'a DocConfig,
}

impl Ctx<'_> {
    fn line(&self, node: &SyntaxNode) -> u32 {
        self.lines
            .line_col(self.text, node.text_range().start())
            .line
            + 1
    }

    fn entity(&self, node: &SyntaxNode) -> Entity {
        let (generics, ports) = self.interfaces(node);
        Entity {
            page: String::new(),
            name: own_name(node).unwrap_or_default(),
            library: String::new(),
            file: self.file.to_owned(),
            line: self.line(node),
            description: self.style.doc(node),
            generics,
            ports,
            architectures: Vec::new(),
            uses: Vec::new(),
        }
    }

    /// The generics and ports of an entity or component.
    fn interfaces(&self, node: &SyntaxNode) -> (Vec<Interface>, Vec<Interface>) {
        let mut generics = Vec::new();
        let mut ports = Vec::new();
        for clause in node.children() {
            let target = match clause.kind() {
                SyntaxKind::GENERIC_CLAUSE => &mut generics,
                SyntaxKind::PORT_CLAUSE => &mut ports,
                _ => continue,
            };
            let Some(list) = clause
                .children()
                .find(|n| n.kind() == SyntaxKind::INTERFACE_LIST)
            else {
                continue;
            };
            for decl in list.children() {
                target.extend(self.interface(&decl));
            }
        }
        (generics, ports)
    }

    fn interface(&self, decl: &SyntaxNode) -> Vec<Interface> {
        let description = self.style.doc(decl);
        match decl.kind() {
            SyntaxKind::INTERFACE_OBJECT_DECL => {
                let names = decl
                    .children()
                    .find(|n| n.kind() == SyntaxKind::IDENT_LIST)
                    .map(|n| idents(&n))
                    .unwrap_or_default();
                let mut mode = String::new();
                let mut ty = String::new();
                for element in decl.children_with_tokens() {
                    match element.kind() {
                        SyntaxKind::IN_KW
                        | SyntaxKind::OUT_KW
                        | SyntaxKind::INOUT_KW
                        | SyntaxKind::BUFFER_KW
                        | SyntaxKind::LINKAGE_KW => {
                            mode = element.to_string().to_ascii_lowercase();
                        }
                        SyntaxKind::MODE_VIEW_INDICATION => {
                            mode = "view".into();
                            if let Some(node) = element.as_node() {
                                ty = compact(node);
                                if let Some(rest) = ty
                                    .get(..5)
                                    .filter(|v| v.eq_ignore_ascii_case("view "))
                                    .map(|_| ty[5..].to_owned())
                                {
                                    ty = rest;
                                }
                            }
                        }
                        SyntaxKind::SUBTYPE_INDICATION => {
                            if let Some(node) = element.as_node() {
                                ty = compact(node);
                            }
                        }
                        _ => {}
                    }
                }
                let default = after_token(decl, SyntaxKind::COLON_EQ);
                names
                    .into_iter()
                    .map(|name| Interface {
                        name,
                        mode: mode.clone(),
                        ty: ty.clone(),
                        default: default.clone(),
                        description: description.clone(),
                    })
                    .collect()
            }
            SyntaxKind::INTERFACE_TYPE_DECL => vec![Interface {
                name: own_name(decl).unwrap_or_default(),
                ty: "type".into(),
                description,
                ..Interface::default()
            }],
            SyntaxKind::INTERFACE_PACKAGE_DECL => vec![Interface {
                name: own_name(decl).unwrap_or_default(),
                ty: "package".into(),
                default: Some(compact(decl))
                    .and_then(|t| t.split_once(" is ").map(|(_, rest)| rest.to_owned())),
                description,
                ..Interface::default()
            }],
            SyntaxKind::INTERFACE_SUBPROGRAM_DECL => {
                let spec = decl.children().find(|n| {
                    matches!(
                        n.kind(),
                        SyntaxKind::FUNCTION_SPEC | SyntaxKind::PROCEDURE_SPEC
                    )
                });
                vec![Interface {
                    name: spec.as_ref().and_then(own_name).unwrap_or_default(),
                    ty: spec.as_ref().map(compact).unwrap_or_default(),
                    description,
                    ..Interface::default()
                }]
            }
            _ => Vec::new(),
        }
    }

    fn architecture(&self, node: &SyntaxNode, enums: &fsm::StateTypes) -> Architecture {
        let mut arch = Architecture {
            name: own_name(node).unwrap_or_default(),
            file: self.file.to_owned(),
            line: self.line(node),
            description: self.style.doc(node),
            ..Architecture::default()
        };
        let mut declarations = Declarations::default();
        self.declarations(node, &mut declarations);
        arch.signals = declarations.signals;
        arch.constants = declarations.constants;
        arch.types = declarations.types;
        arch.subprograms = declarations.subprograms;
        arch.components = declarations.components;
        arch.aliases = declarations.aliases;
        self.assignments(node, &mut arch.assignments);
        if self.config.processes {
            for process in node
                .descendants()
                .filter(|n| n.kind() == SyntaxKind::PROCESS_STMT)
            {
                arch.processes.push(self.process(&process));
            }
        }
        self.instances(node, &mut Vec::new(), &mut arch.instances);
        if self.config.fsm {
            arch.fsms = fsm::detect(node, enums, self.style);
        }
        if !self.config.signals {
            arch.signals.clear();
        }
        if !self.config.constants {
            arch.constants.clear();
        }
        if !self.config.types {
            arch.types.clear();
        }
        if !self.config.subprograms {
            arch.subprograms.clear();
        }
        if !self.config.instances {
            arch.instances.clear();
        }
        arch
    }

    /// The declarations directly in `node`'s declarative part.
    fn declarations(&self, node: &SyntaxNode, out: &mut Declarations) {
        for decl in node.children() {
            let description = self.style.doc(&decl);
            match decl.kind() {
                SyntaxKind::SIGNAL_DECL | SyntaxKind::CONSTANT_DECL => {
                    let names = decl
                        .children()
                        .find(|n| n.kind() == SyntaxKind::IDENT_LIST)
                        .map(|n| idents(&n))
                        .unwrap_or_default();
                    let ty = decl
                        .children()
                        .find(|n| n.kind() == SyntaxKind::SUBTYPE_INDICATION)
                        .map(|n| compact(&n))
                        .unwrap_or_default();
                    let value = after_token(&decl, SyntaxKind::COLON_EQ);
                    let target = if decl.kind() == SyntaxKind::SIGNAL_DECL {
                        &mut out.signals
                    } else {
                        &mut out.constants
                    };
                    for name in names {
                        target.push(Object {
                            name,
                            ty: ty.clone(),
                            value: value.clone(),
                            description: description.clone(),
                        });
                    }
                }
                SyntaxKind::ALIAS_DECL => {
                    let ty = decl
                        .children()
                        .find(|n| n.kind() == SyntaxKind::SUBTYPE_INDICATION)
                        .map(|n| compact(&n))
                        .unwrap_or_default();
                    // The aliased name follows `is`.
                    let target = decl
                        .children_with_tokens()
                        .skip_while(|e| e.kind() != SyntaxKind::IS_KW)
                        .find_map(|e| e.into_node());
                    let Some(target) = target else { continue };
                    let (object, slice) = match target.kind() {
                        SyntaxKind::CALL_EXPR => (
                            target.children().next().map(|n| compact(&n)),
                            target
                                .children()
                                .find(|n| n.kind() == SyntaxKind::ASSOCIATION_LIST)
                                .map(|n| {
                                    compact(&n)
                                        .trim_start_matches('(')
                                        .trim_end_matches(')')
                                        .trim()
                                        .to_owned()
                                }),
                        ),
                        _ => (Some(compact(&target)), None),
                    };
                    out.aliases.push(Alias {
                        name: own_name(&decl).unwrap_or_default(),
                        ty,
                        target: object.unwrap_or_default(),
                        slice,
                        description,
                    });
                }
                SyntaxKind::TYPE_DECL | SyntaxKind::SUBTYPE_DECL => {
                    let definition = after_token(&decl, SyntaxKind::IS_KW).unwrap_or_default();
                    let fields = decl
                        .children()
                        .filter(|n| n.kind() == SyntaxKind::RECORD_TYPE_DEF)
                        .flat_map(|r| r.children())
                        .filter(|n| n.kind() == SyntaxKind::ELEMENT_DECL)
                        .flat_map(|element| {
                            let ty = element
                                .children()
                                .find(|n| n.kind() == SyntaxKind::SUBTYPE_INDICATION)
                                .map(|n| compact(&n))
                                .unwrap_or_default();
                            let doc = self.style.doc(&element);
                            element
                                .children()
                                .find(|n| n.kind() == SyntaxKind::IDENT_LIST)
                                .map(|n| idents(&n))
                                .unwrap_or_default()
                                .into_iter()
                                .map(move |name| (name, ty.clone(), doc.clone()))
                        })
                        .collect();
                    out.types.push(TypeInfo {
                        fields,
                        name: own_name(&decl).unwrap_or_default(),
                        kind: if decl.kind() == SyntaxKind::TYPE_DECL {
                            "type"
                        } else {
                            "subtype"
                        },
                        definition,
                        description,
                    });
                }
                SyntaxKind::SUBPROGRAM_DECL | SyntaxKind::SUBPROGRAM_BODY => {
                    let Some(spec) = decl.children().find(|n| {
                        matches!(
                            n.kind(),
                            SyntaxKind::FUNCTION_SPEC | SyntaxKind::PROCEDURE_SPEC
                        )
                    }) else {
                        continue;
                    };
                    let name = own_name(&spec).unwrap_or_default();
                    let signature = compact(&spec);
                    // A body in a package body repeats its declaration.
                    if out.subprograms.iter().any(|s| s.signature == signature) {
                        continue;
                    }
                    out.subprograms.push(Subprogram {
                        name,
                        signature,
                        description,
                    });
                }
                SyntaxKind::COMPONENT_DECL => {
                    let (generics, ports) = self.interfaces(&decl);
                    out.components.push(Component {
                        name: own_name(&decl).unwrap_or_default(),
                        description,
                        generics,
                        ports,
                    });
                }
                _ => {}
            }
        }
    }

    fn package(&self, node: &SyntaxNode) -> Package {
        let mut declarations = Declarations::default();
        self.declarations(node, &mut declarations);
        Package {
            page: String::new(),
            name: own_name(node).unwrap_or_default(),
            library: String::new(),
            file: self.file.to_owned(),
            line: self.line(node),
            description: self.style.doc(node),
            constants: declarations.constants,
            signals: declarations.signals,
            types: declarations.types,
            subprograms: declarations.subprograms,
            components: declarations.components,
            aliases: declarations.aliases,
            uses: Vec::new(),
        }
    }

    /// The concurrent signal assignments of an architecture, in generate
    /// and block statements too.
    fn assignments(&self, node: &SyntaxNode, out: &mut Vec<Assignment>) {
        for child in node.children() {
            match child.kind() {
                SyntaxKind::SIGNAL_ASSIGN_STMT | SyntaxKind::SELECTED_ASSIGN_STMT => {
                    let Some(target) = assignment_target(&child) else {
                        continue;
                    };
                    let Some(name) = base_name(&target) else {
                        continue;
                    };
                    out.push(Assignment {
                        label: label(&child),
                        line: self.line(&child),
                        target: name,
                        reads: reads(&child),
                    });
                }
                SyntaxKind::FOR_GENERATE_STMT
                | SyntaxKind::IF_GENERATE_STMT
                | SyntaxKind::CASE_GENERATE_STMT
                | SyntaxKind::BLOCK_STMT
                | SyntaxKind::GENERATE_BODY
                | SyntaxKind::CASE_ALTERNATIVE => self.assignments(&child, out),
                _ => {}
            }
        }
    }

    fn process(&self, node: &SyntaxNode) -> Process {
        let label = label(node);
        let sensitivity = node
            .children()
            .find(|n| n.kind() == SyntaxKind::SENSITIVITY_LIST)
            .map(|list| {
                if list
                    .children_with_tokens()
                    .any(|e| e.kind() == SyntaxKind::ALL_KW)
                {
                    vec!["all".to_owned()]
                } else {
                    list.children().map(|n| compact(&n)).collect()
                }
            })
            .unwrap_or_default();
        let mut drives: Vec<String> = Vec::new();
        for assign in node
            .descendants()
            .filter(|n| n.kind() == SyntaxKind::SIGNAL_ASSIGN_STMT)
        {
            if let Some(target) = assign.children().next().and_then(|t| base_name(&t))
                && !drives
                    .iter()
                    .any(|d| identifier_key(d) == identifier_key(&target))
            {
                drives.push(target);
            }
        }
        let clock = clock(node);
        Process {
            label,
            line: self.line(node),
            sensitivity,
            falling: clock.as_ref().is_some_and(|(_, falling)| *falling),
            reset: clock.as_ref().and_then(|_| reset(node)),
            clock: clock.map(|(c, _)| c),
            drives,
            reads: reads(node),
            description: self.style.doc(node),
        }
    }

    fn instances(&self, node: &SyntaxNode, scope: &mut Vec<String>, out: &mut Vec<Instance>) {
        for child in node.children() {
            match child.kind() {
                SyntaxKind::COMPONENT_INSTANTIATION_STMT => {
                    if let Some(instance) = self.instance(&child, scope) {
                        out.push(instance);
                    }
                }
                SyntaxKind::FOR_GENERATE_STMT
                | SyntaxKind::IF_GENERATE_STMT
                | SyntaxKind::CASE_GENERATE_STMT
                | SyntaxKind::BLOCK_STMT => {
                    let pushed = label(&child).map(|l| scope.push(l)).is_some();
                    self.instances(&child, scope, out);
                    if pushed {
                        scope.pop();
                    }
                }
                SyntaxKind::GENERATE_BODY | SyntaxKind::CASE_ALTERNATIVE => {
                    self.instances(&child, scope, out);
                }
                _ => {}
            }
        }
    }

    fn instance(&self, node: &SyntaxNode, scope: &[String]) -> Option<Instance> {
        let mut kind = InstanceKind::Component;
        let mut target = None;
        let mut generic_map = Vec::new();
        let mut port_map = Vec::new();
        for element in node.children_with_tokens() {
            match element.kind() {
                SyntaxKind::ENTITY_KW => kind = InstanceKind::Entity,
                SyntaxKind::CONFIGURATION_KW => kind = InstanceKind::Configuration,
                SyntaxKind::COMPONENT_KW => kind = InstanceKind::Component,
                SyntaxKind::NAME | SyntaxKind::SELECTED_NAME | SyntaxKind::CALL_EXPR
                    if target.is_none() =>
                {
                    target = element.into_node();
                }
                SyntaxKind::GENERIC_MAP_ASPECT => {
                    generic_map = associations(element.as_node()?);
                }
                SyntaxKind::PORT_MAP_ASPECT => {
                    port_map = associations(element.as_node()?);
                }
                _ => {}
            }
        }
        let target = target?;
        // `work.counter(rtl)` parses as a call of `work.counter`.
        let (name_node, architecture) = if target.kind() == SyntaxKind::CALL_EXPR {
            let arch = target
                .children()
                .find(|n| n.kind() == SyntaxKind::ASSOCIATION_LIST)
                .map(|n| {
                    compact(&n)
                        .trim_matches(|c| c == '(' || c == ')')
                        .trim()
                        .to_owned()
                });
            (target.children().next()?, arch)
        } else {
            (target, None)
        };
        let (library, unit) = match name_node.kind() {
            SyntaxKind::SELECTED_NAME => {
                let prefix = name_node.children().next().map(|n| compact(&n));
                let last = name_node
                    .children_with_tokens()
                    .filter_map(|e| e.into_token())
                    .filter(|t| matches!(t.kind(), SyntaxKind::IDENT | SyntaxKind::EXT_IDENT))
                    .last()?;
                (prefix, last.text().to_owned())
            }
            _ => (None, compact(&name_node)),
        };
        Some(Instance {
            label: label(node).unwrap_or_default(),
            scope: scope.to_vec(),
            kind,
            library,
            unit,
            architecture,
            generic_map,
            port_map,
            description: self.style.doc(node),
        })
    }
}

#[derive(Default)]
struct Declarations {
    signals: Vec<Object>,
    constants: Vec<Object>,
    types: Vec<TypeInfo>,
    subprograms: Vec<Subprogram>,
    components: Vec<Component>,
    aliases: Vec<Alias>,
}

/// The packages and contexts a context clause uses, with `work` resolved
/// to `library`.
fn uses_of(context: &SyntaxNode, library: &str) -> Vec<Use> {
    let mut uses = Vec::new();
    for item in context.children() {
        let names: Vec<SyntaxNode> = match item.kind() {
            SyntaxKind::USE_CLAUSE | SyntaxKind::CONTEXT_REFERENCE => item
                .children()
                .filter(|n| matches!(n.kind(), SyntaxKind::SELECTED_NAME))
                .collect(),
            _ => continue,
        };
        for name in names {
            // `lib.pkg.all` or `lib.pkg.item`: the first two identifiers;
            // `lib.ctx` for a context reference.
            let parts: Vec<String> = name
                .descendants_with_tokens()
                .filter_map(|e| e.into_token())
                .filter(|t| matches!(t.kind(), SyntaxKind::IDENT | SyntaxKind::EXT_IDENT))
                .map(|t| identifier_key(t.text()))
                .collect();
            if parts.len() < 2 {
                continue;
            }
            let lib = if parts[0] == "work" {
                identifier_key(library)
            } else {
                parts[0].clone()
            };
            let u = Use {
                library: lib,
                unit: parts[1].clone(),
            };
            if !uses.contains(&u) {
                uses.push(u);
            }
        }
    }
    uses
}

/// The formal and actual of each association, `("", actual)` for
/// positional ones.
fn associations(aspect: &SyntaxNode) -> Vec<(String, String)> {
    let Some(list) = aspect
        .children()
        .find(|n| n.kind() == SyntaxKind::ASSOCIATION_LIST)
    else {
        return Vec::new();
    };
    list.children()
        .filter(|n| n.kind() == SyntaxKind::ASSOCIATION_ELEMENT)
        .map(|element| {
            let has_arrow = element
                .children_with_tokens()
                .any(|e| e.kind() == SyntaxKind::ARROW);
            if has_arrow {
                let formal = element
                    .children()
                    .next()
                    .map(|n| compact(&n))
                    .unwrap_or_default();
                let actual = after_token(&element, SyntaxKind::ARROW).unwrap_or_default();
                (formal, actual)
            } else {
                (String::new(), compact(&element))
            }
        })
        .collect()
}

/// The clock of a clocked process: the signal of its first
/// `rising_edge(x)`, `falling_edge(x)` or `x'event`.
/// With whether the edge is a falling one.
fn clock(process: &SyntaxNode) -> Option<(String, bool)> {
    process.descendants().find_map(|node| edge(&node))
}

/// The clock and edge an expression tests: `rising_edge(clk)`,
/// `falling_edge(clk)` or `clk'event` (taken as rising, unless compared
/// with `'0'` in the same conjunction).
fn edge(node: &SyntaxNode) -> Option<(String, bool)> {
    match node.kind() {
        SyntaxKind::CALL_EXPR => {
            let callee = node.children().next()?;
            let name = compact(&callee).to_ascii_lowercase();
            if name == "rising_edge" || name == "falling_edge" {
                let argument = node
                    .children()
                    .find(|n| n.kind() == SyntaxKind::ASSOCIATION_LIST)?;
                return Some((
                    compact(&argument)
                        .trim_matches(|c| c == '(' || c == ')')
                        .trim()
                        .to_owned(),
                    name == "falling_edge",
                ));
            }
            None
        }
        SyntaxKind::ATTRIBUTE_NAME => {
            let text = compact(node);
            let (prefix, attribute) = text.rsplit_once('\'')?;
            if !attribute.eq_ignore_ascii_case("event") {
                return None;
            }
            let falling = node
                .parent()
                .filter(|p| p.kind() == SyntaxKind::BINARY_EXPR)
                .is_some_and(|p| compact(&p).contains("= '0'"));
            Some((prefix.to_owned(), falling))
        }
        _ => None,
    }
}

/// Whether an expression tests a clock edge.
fn tests_edge(expr: &SyntaxNode) -> bool {
    expr.descendants().any(|n| edge(&n).is_some())
}

/// The reset of a clocked process: the test of the branch before the
/// clock edge (asynchronous), or of the first `if` under the edge whose
/// signal is named like a reset (synchronous).
fn reset(process: &SyntaxNode) -> Option<Reset> {
    for stmt in process
        .descendants()
        .filter(|n| n.kind() == SyntaxKind::IF_STMT)
    {
        let branches = if_branches(&stmt);
        let Some(k) = branches
            .iter()
            .position(|(c, _)| c.as_ref().is_some_and(tests_edge))
        else {
            continue;
        };
        if k > 0 {
            let (signal, active_high) = reset_test(branches[0].0.as_ref()?)?;
            return Some(Reset {
                signal,
                asynchronous: true,
                active_high,
            });
        }
        // `if rising_edge(clk) then if rst = '1' then ...`
        let inner = branches[0]
            .1
            .iter()
            .find(|n| n.kind() == SyntaxKind::IF_STMT)?;
        let first = if_branches(inner).into_iter().next()?;
        let (signal, active_high) = reset_test(first.0.as_ref()?)?;
        let key = identifier_key(&signal);
        if key.contains("rst") || key.contains("reset") {
            return Some(Reset {
                signal,
                asynchronous: false,
                active_high,
            });
        }
        return None;
    }
    // `wait until rising_edge(clk); if rst = '1' then ...`
    let body: Vec<SyntaxNode> = process.children().collect();
    let wait = body
        .iter()
        .position(|n| n.kind() == SyntaxKind::WAIT_STMT && tests_edge(n))?;
    let inner = body[wait..]
        .iter()
        .find(|n| n.kind() == SyntaxKind::IF_STMT)?;
    let first = if_branches(inner).into_iter().next()?;
    let (signal, active_high) = reset_test(first.0.as_ref()?)?;
    let key = identifier_key(&signal);
    (key.contains("rst") || key.contains("reset")).then_some(Reset {
        signal,
        asynchronous: false,
        active_high,
    })
}

/// The branches of an `if` statement: each condition (`None` for `else`)
/// with its statements.
fn if_branches(stmt: &SyntaxNode) -> Vec<(Option<SyntaxNode>, Vec<SyntaxNode>)> {
    let mut branches: Vec<(Option<SyntaxNode>, Vec<SyntaxNode>)> = Vec::new();
    let mut want_condition = false;
    let mut in_body = false;
    for element in stmt.children_with_tokens() {
        match element.kind() {
            SyntaxKind::IF_KW | SyntaxKind::ELSIF_KW => {
                want_condition = true;
                in_body = false;
            }
            SyntaxKind::ELSE_KW => {
                branches.push((None, Vec::new()));
                in_body = true;
            }
            SyntaxKind::THEN_KW => in_body = true,
            SyntaxKind::END_KW => break,
            _ => {
                if let Some(node) = element.into_node() {
                    if want_condition {
                        branches.push((Some(node), Vec::new()));
                        want_condition = false;
                    } else if in_body && let Some(last) = branches.last_mut() {
                        last.1.push(node);
                    }
                }
            }
        }
    }
    branches
}

/// `rst = '1'`, `rst_n = '0'`, `rst` or `not rst_n`: the signal and
/// whether it is active high.
fn reset_test(cond: &SyntaxNode) -> Option<(String, Option<bool>)> {
    let cond = strip_parens(cond);
    match cond.kind() {
        SyntaxKind::NAME => Some((ident(&cond)?.text().to_owned(), Some(true))),
        SyntaxKind::UNARY_EXPR => {
            let operand = cond.children().next()?;
            let operand = strip_parens(&operand);
            let not = cond
                .children_with_tokens()
                .any(|e| e.kind() == SyntaxKind::NOT_KW);
            (not && operand.kind() == SyntaxKind::NAME)
                .then(|| Some((ident(&operand)?.text().to_owned(), Some(false))))
                .flatten()
        }
        SyntaxKind::BINARY_EXPR => {
            let eq = cond
                .children_with_tokens()
                .any(|e| e.kind() == SyntaxKind::EQ);
            if !eq {
                return None;
            }
            let mut sides = cond.children();
            let (a, b) = (sides.next()?, sides.next()?);
            let (name, value) = if a.kind() == SyntaxKind::NAME {
                (a, b)
            } else {
                (b, a)
            };
            if name.kind() != SyntaxKind::NAME {
                return None;
            }
            let value = compact(&value).to_ascii_lowercase();
            let high = match value.as_str() {
                "'1'" | "true" | "1" => Some(true),
                "'0'" | "false" | "0" => Some(false),
                _ => return None,
            };
            Some((ident(&name)?.text().to_owned(), high))
        }
        _ => None,
    }
}

fn strip_parens(node: &SyntaxNode) -> SyntaxNode {
    let mut node = node.clone();
    while node.kind() == SyntaxKind::PAREN_EXPR {
        match node.children().next() {
            Some(inner) => node = inner,
            None => break,
        }
    }
    node
}

/// The target of a signal assignment: the node before `<=`.
fn assignment_target(stmt: &SyntaxNode) -> Option<SyntaxNode> {
    let mut target = None;
    for element in stmt.children_with_tokens() {
        match element.kind() {
            SyntaxKind::LTE => return target,
            SyntaxKind::LABEL => {}
            _ => {
                if let Some(node) = element.into_node() {
                    target = Some(node);
                }
            }
        }
    }
    None
}

/// The simple names read in `node`, by spelling, each once: every name but
/// the targets of signal and variable assignments.
fn reads(node: &SyntaxNode) -> Vec<String> {
    let mut targets = Vec::new();
    for stmt in node.descendants().filter(|n| {
        matches!(
            n.kind(),
            SyntaxKind::SIGNAL_ASSIGN_STMT
                | SyntaxKind::SELECTED_ASSIGN_STMT
                | SyntaxKind::VARIABLE_ASSIGN_STMT
        )
    }) {
        if let Some(target) = assignment_target(&stmt) {
            let mut base = target;
            while base.kind() != SyntaxKind::NAME {
                match base.children().next() {
                    Some(first) => base = first,
                    None => break,
                }
            }
            targets.push(base.text_range());
        }
    }
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for name in node.descendants().filter(|n| n.kind() == SyntaxKind::NAME) {
        if targets.contains(&name.text_range()) {
            continue;
        }
        // Labels of the statement itself are not names.
        if name.parent().is_some_and(|p| p.kind() == SyntaxKind::LABEL) {
            continue;
        }
        if let Some(t) = ident(&name)
            && seen.insert(identifier_key(t.text()))
        {
            out.push(t.text().to_owned());
        }
    }
    out
}

/// The label of a statement.
pub(crate) fn label(node: &SyntaxNode) -> Option<String> {
    let label = node.children().find(|n| n.kind() == SyntaxKind::LABEL)?;
    ident(&label).map(|t| t.text().to_owned())
}

/// The name a declaration introduces: its first identifier token.
pub(crate) fn own_name(node: &SyntaxNode) -> Option<String> {
    node.children_with_tokens()
        .filter_map(|e| e.into_token())
        .find(|t| {
            matches!(
                t.kind(),
                SyntaxKind::IDENT | SyntaxKind::EXT_IDENT | SyntaxKind::STRING_LITERAL
            )
        })
        .map(|t| t.text().to_owned())
}

pub(crate) fn ident(node: &SyntaxNode) -> Option<SyntaxToken> {
    node.children_with_tokens()
        .filter_map(|e| e.into_token())
        .find(|t| matches!(t.kind(), SyntaxKind::IDENT | SyntaxKind::EXT_IDENT))
}

fn idents(node: &SyntaxNode) -> Vec<String> {
    node.children_with_tokens()
        .filter_map(|e| e.into_token())
        .filter(|t| matches!(t.kind(), SyntaxKind::IDENT | SyntaxKind::EXT_IDENT))
        .map(|t| t.text().to_owned())
        .collect()
}

/// The object a target names: `a` for `a(3)`, `a.b` or `a(7 downto 0)`.
pub(crate) fn base_name(node: &SyntaxNode) -> Option<String> {
    let mut node = node.clone();
    loop {
        match node.kind() {
            SyntaxKind::NAME => return ident(&node).map(|t| t.text().to_owned()),
            SyntaxKind::AGGREGATE => return None,
            _ => node = node.children().next()?,
        }
    }
}

/// The text after the first `token` child of `node`, up to its end or
/// final `;`, on one line.
pub(crate) fn after_token(node: &SyntaxNode, token: SyntaxKind) -> Option<String> {
    let mut seen = false;
    let mut out = String::new();
    for element in node.children_with_tokens() {
        if !seen {
            seen = element.kind() == token;
            continue;
        }
        if element.kind() == SyntaxKind::SEMI {
            break;
        }
        match element {
            vfl_syntax::SyntaxElement::Node(n) => push_compact(&n, &mut out),
            vfl_syntax::SyntaxElement::Token(t) => push_token(&t, &mut out),
        }
    }
    seen.then(|| tidy(&out))
}

/// The text of a node on one line, without comments.
pub(crate) fn compact(node: &SyntaxNode) -> String {
    let mut out = String::new();
    push_compact(node, &mut out);
    tidy(&out)
}

fn push_compact(node: &SyntaxNode, out: &mut String) {
    for token in node
        .descendants_with_tokens()
        .filter_map(|e| e.into_token())
    {
        push_token(&token, out);
    }
}

fn push_token(token: &SyntaxToken, out: &mut String) {
    match token.kind() {
        SyntaxKind::WHITESPACE | SyntaxKind::LINE_COMMENT | SyntaxKind::BLOCK_COMMENT => {
            if !out.ends_with(' ') {
                out.push(' ');
            }
        }
        _ => out.push_str(token.text()),
    }
}

fn tidy(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let chars: Vec<char> = text.trim().chars().collect();
    for (i, &c) in chars.iter().enumerate() {
        if c == ' ' {
            let prev = out.chars().last();
            let next = chars.get(i + 1).copied();
            if prev == Some('(') || next == Some(')') || next == Some(',') || next == Some(';') {
                continue;
            }
        }
        out.push(c);
    }
    out
}

/// The project-relative path of a file, with `/` separators.
pub fn relative_path(root: &std::path::Path, path: &std::path::Path) -> String {
    let relative: PathBuf = path.strip_prefix(root).unwrap_or(path).to_owned();
    relative.to_string_lossy().replace('\\', "/")
}

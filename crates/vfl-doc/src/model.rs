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
    /// The signals the process assigns.
    pub drives: Vec<String>,
    pub description: String,
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
    let mut package_bodies: Vec<(String, String, String)> = Vec::new();
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
            for node in unit.children() {
                match node.kind() {
                    SyntaxKind::ENTITY_DECL => {
                        let mut entity = ctx.entity(&node);
                        entity.library = file.library.clone();
                        design.entities.push(entity);
                    }
                    SyntaxKind::ARCHITECTURE_BODY => {
                        let entity = node
                            .children()
                            .find(|n| n.kind() == SyntaxKind::NAME)
                            .and_then(|n| ident(&n))
                            .map(|t| identifier_key(t.text()))
                            .unwrap_or_default();
                        let arch = ctx.architecture(&node, &enums);
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
                        package_bodies.push((file.library.clone(), name, description));
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
            design.entities[target].architectures.push(arch);
        }
    }
    for (library, name, description) in package_bodies {
        if description.is_empty() {
            continue;
        }
        if let Some(package) = design
            .packages
            .iter_mut()
            .find(|p| p.library == library && identifier_key(&p.name) == name)
            && package.description.is_empty()
        {
            package.description = description;
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
                SyntaxKind::TYPE_DECL | SyntaxKind::SUBTYPE_DECL => {
                    let definition = after_token(&decl, SyntaxKind::IS_KW).unwrap_or_default();
                    out.types.push(TypeInfo {
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
        Process {
            label,
            line: self.line(node),
            sensitivity,
            clock: clock(node),
            drives,
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
fn clock(process: &SyntaxNode) -> Option<String> {
    for node in process.descendants() {
        match node.kind() {
            SyntaxKind::CALL_EXPR => {
                let callee = node.children().next()?;
                let name = compact(&callee).to_ascii_lowercase();
                if name == "rising_edge" || name == "falling_edge" {
                    let argument = node
                        .children()
                        .find(|n| n.kind() == SyntaxKind::ASSOCIATION_LIST)?;
                    return Some(
                        compact(&argument)
                            .trim_matches(|c| c == '(' || c == ')')
                            .trim()
                            .to_owned(),
                    );
                }
            }
            SyntaxKind::ATTRIBUTE_NAME => {
                let text = compact(&node);
                if let Some((prefix, attribute)) = text.rsplit_once('\'')
                    && attribute.eq_ignore_ascii_case("event")
                {
                    return Some(prefix.to_owned());
                }
            }
            _ => {}
        }
    }
    None
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

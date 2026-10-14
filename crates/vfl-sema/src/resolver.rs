//! Name resolution for one design unit: walks the unit's syntax tree in
//! source order, declaring names into nested regions and resolving every
//! name it meets against them, the unit's use clauses, and the library
//! units of the project.
//!
//! Resolution is by name only, without overload resolution or types. A name
//! that cannot be checked (selected through a record, from a library that
//! is not part of the project, ...) resolves to "unknown" and is never
//! reported: false errors are worse than missed ones.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use smallvec::smallvec;
use vfl_syntax::{SyntaxKind, SyntaxNode, SyntaxToken, TextRange, identifier_key};

use crate::analysis::{Ctx, UnitResult};
use crate::decl::{
    Access, Decl, DeclId, DeclKind, Element, FileId, Mode, Reference, Region, Target, Targets,
    TypeDef, UnitId,
};
use crate::diagnostic::{Diagnostic, Edit, Fix, codes};
use crate::syntax::*;

/// A use clause item that names were imported through.
#[derive(Debug, Clone)]
pub(crate) struct Import {
    pub id: ImportId,
    pub kind: ImportKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct ImportId {
    pub unit: UnitId,
    pub index: u32,
}

impl Import {
    /// Feeds what the import makes visible to `h`.
    pub fn hash_into(&self, h: &mut impl std::hash::Hasher) {
        use std::hash::Hash;
        self.id.hash(h);
        match &self.kind {
            ImportKind::All(region) => {
                0u8.hash(h);
                region.hash_into(h);
            }
            ImportKind::Names(name, targets) => {
                1u8.hash(h);
                name.hash(h);
                targets.hash(h);
            }
            ImportKind::Library(library) => {
                2u8.hash(h);
                library.hash(h);
            }
            ImportKind::Opaque => 3u8.hash(h),
        }
    }

    /// Calls `f` with every declaration the import makes visible.
    pub fn decl_ids(&self, mut f: impl FnMut(DeclId)) {
        match &self.kind {
            ImportKind::All(region) => region.iter().for_each(f),
            ImportKind::Names(_, targets) => {
                for t in targets {
                    if let Target::Decl(id) = t {
                        f(*id);
                    }
                }
            }
            ImportKind::Library(_) | ImportKind::Opaque => {}
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) enum ImportKind {
    /// `use lib.pkg.all`.
    All(Arc<Region>),
    /// `use lib.pkg.x` or `use lib.pkg`.
    Names(String, Targets),
    /// `use lib.all`: every unit of a library.
    Library(String),
    /// Something that could not be resolved: every failed lookup in its
    /// scope is unknown rather than an error.
    Opaque,
}

/// A use clause item, for the unused-use check.
#[derive(Debug, Clone)]
pub(crate) struct UseItem {
    /// The selected name.
    pub range: TextRange,
    /// The whole clause, and whether this is its only item.
    pub clause: TextRange,
    pub only_item: bool,
    /// Whether it names something we know; unknown ones are never unused.
    pub resolved: bool,
}

/// The result of looking a name up.
#[derive(Debug, Clone)]
enum Lookup {
    Found(Targets),
    /// Not checkable; never an error.
    Unknown,
    NotFound,
}

#[derive(Default)]
struct Frame {
    locals: Region,
    /// Regions visible as if declared here: an entity's for its
    /// architecture, a package's for its body.
    inherited: Vec<Arc<Region>>,
    imports: Vec<Import>,
    libraries: Vec<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Interface {
    Generic,
    Port,
    Parameter,
}

pub(crate) struct Resolver<'a> {
    ctx: &'a Ctx<'a>,
    unit: UnitId,
    file: FileId,
    library: String,
    decls: Vec<Decl>,
    frames: Vec<Frame>,
    parents: Vec<DeclId>,
    refs: Vec<Reference>,
    diags: Vec<Diagnostic>,
    use_items: Vec<UseItem>,
    used_imports: HashSet<ImportId>,
    /// Inside an `ERROR` node, or a builtin file: no name errors.
    quiet: u32,
    /// Inside aggregate choices: a name may be a record element.
    soft: u32,
    /// Implicit declarations (GUARD), never reported as unused.
    implicit: Vec<DeclId>,
    /// Inside a subtype indication, where constraints may name record elements.
    in_subtype: u32,
    access: Access,
    interface: Interface,
    /// Operators whose visible declarations are unknown.
    opaque_ops: Vec<TextRange>,
}

impl<'a> Resolver<'a> {
    pub fn new(ctx: &'a Ctx<'a>, unit: UnitId) -> Self {
        let data = &ctx.units[unit as usize];
        let file = data.file;
        let mut root = Frame {
            libraries: vec!["std".into(), "work".into()],
            ..Frame::default()
        };
        if let Some(standard) = ctx.standard_region() {
            root.imports.push(Import {
                id: ImportId {
                    unit: u32::MAX,
                    index: 0,
                },
                kind: ImportKind::All(standard),
            });
        }
        Resolver {
            ctx,
            unit,
            file,
            library: data.library.clone(),
            decls: Vec::new(),
            frames: vec![root],
            parents: Vec::new(),
            refs: Vec::new(),
            diags: Vec::new(),
            use_items: Vec::new(),
            used_imports: HashSet::new(),
            quiet: u32::from(ctx.files[file as usize].builtin),
            soft: 0,
            implicit: Vec::new(),
            in_subtype: 0,
            access: Access::Read,
            interface: Interface::Parameter,
            opaque_ops: Vec::new(),
        }
    }

    // ---- Units ------------------------------------------------------------

    /// Analyzes the unit at `node` (the child of a `DESIGN_UNIT`).
    pub fn run(mut self, design_unit: &SyntaxNode) -> UnitResult {
        let Some(node) = design_unit
            .children()
            .find(|n| n.kind() != SyntaxKind::CONTEXT_CLAUSE)
        else {
            return self.finish(Arc::new(Region::default()));
        };
        // The unit's own declaration is always index 0.
        let kind = match node.kind() {
            SyntaxKind::ENTITY_DECL => DeclKind::Entity,
            SyntaxKind::ARCHITECTURE_BODY => DeclKind::Architecture,
            SyntaxKind::PACKAGE_DECL => DeclKind::Package,
            SyntaxKind::PACKAGE_BODY => DeclKind::PackageBody,
            SyntaxKind::PACKAGE_INSTANTIATION_DECL => DeclKind::PackageInstance,
            SyntaxKind::CONTEXT_DECL => DeclKind::Context,
            _ => DeclKind::Configuration,
        };
        let name = designator(&node);
        let id = self.push_decl(name.as_ref(), kind, &node);
        self.parents.push(id);

        // A unit's own name is visible inside it (`e'path_name`).
        let mut context = Frame::default();
        if let Some(name) = &name {
            context.locals.insert(&designator_key(name), id);
        }
        self.frames.push(context);
        if let Some(clause) = design_unit
            .children()
            .find(|n| n.kind() == SyntaxKind::CONTEXT_CLAUSE)
        {
            for item in clause.children() {
                self.visit(&item);
            }
        }

        let region = match node.kind() {
            SyntaxKind::CONTEXT_DECL => {
                // Its clauses are what context references import.
                self.visit_children(&node);
                Arc::new(Region::default())
            }
            SyntaxKind::ENTITY_DECL | SyntaxKind::PACKAGE_DECL => {
                self.frames.push(Frame::default());
                self.visit_children(&node);
                self.pop_unit_frame()
            }
            SyntaxKind::ARCHITECTURE_BODY => {
                self.secondary(&node, DeclKind::Entity);
                self.frames.push(Frame::default());
                self.visit_children_except(&node, |n| n.kind() == SyntaxKind::NAME);
                self.pop_unit_frame()
            }
            SyntaxKind::PACKAGE_BODY => {
                self.secondary(&node, DeclKind::Package);
                self.frames.push(Frame::default());
                self.visit_children(&node);
                self.pop_unit_frame()
            }
            SyntaxKind::PACKAGE_INSTANTIATION_DECL => self.package_instance(&node),
            _ => {
                self.configuration(&node);
                Arc::new(Region::default())
            }
        };
        if let Some(name) = name {
            self.end_label_refs(&node, &name, id);
        }
        self.finish(region)
    }

    fn finish(mut self, region: Arc<Region>) -> UnitResult {
        if let Some(first) = self.decls.first_mut() {
            first.members = Some(region.clone());
        }
        for id in std::mem::take(&mut self.implicit) {
            let range = self.decls[id.index as usize].range;
            self.record(range, smallvec![Target::Decl(id)], Access::ReadWrite);
        }
        let mut imports = Vec::new();
        let mut libraries = Vec::new();
        for frame in self.frames.iter().skip(1) {
            imports.extend(frame.imports.iter().cloned());
            libraries.extend(frame.libraries.iter().cloned());
        }
        UnitResult {
            decls: self.decls,
            region,
            imports,
            libraries,
            refs: self.refs,
            elements: Vec::new(),
            diagnostics: self.diags,
            use_items: self.use_items,
            used_imports: self.used_imports.into_iter().collect(),
            opaque_ops: {
                // Sorted for the type checker's binary search.
                let mut ops = self.opaque_ops;
                ops.sort_by_key(|r| r.start());
                ops
            },
        }
    }

    /// Pops the unit's own region. Its use clauses stay visible to the
    /// unit's secondary units, so they move to the enclosing frame.
    fn pop_unit_frame(&mut self) -> Arc<Region> {
        let frame = self.frames.pop().unwrap();
        let outer = self.frames.last_mut().unwrap();
        outer.imports.extend(frame.imports);
        outer.libraries.extend(frame.libraries);
        Arc::new(frame.locals)
    }

    /// Makes an architecture's entity, or a package body's package, visible.
    fn secondary(&mut self, node: &SyntaxNode, kind: DeclKind) {
        let (name, range) = if kind == DeclKind::Entity {
            match node
                .children()
                .find(|n| n.kind() == SyntaxKind::NAME)
                .and_then(|n| simple_name_token(&n))
            {
                Some(t) => (identifier_key(t.text()), t.text_range()),
                None => return,
            }
        } else {
            match designator(node) {
                Some(t) => (identifier_key(t.text()), t.text_range()),
                None => return,
            }
        };
        let unit = self
            .ctx
            .unit_in_library(&self.library, &name, self.file)
            .filter(|&u| self.ctx.unit_decl_kind(u) == Some(kind));
        let Some(unit) = unit else {
            let what = if kind == DeclKind::Entity {
                "entity"
            } else {
                "package"
            };
            self.error(
                codes::UNRESOLVED_UNIT,
                range,
                format!("no {what} `{name}` in library `{}`", self.library),
            );
            self.push_opaque();
            return;
        };
        let Some(result) = self.ctx.result(unit) else {
            self.push_opaque();
            return;
        };
        self.refs.push(Reference {
            range,
            targets: smallvec![Target::Decl(DeclId { unit, index: 0 })],
            access: Access::Other,
        });
        let mut locals = Region::default();
        locals.insert(&name, DeclId { unit, index: 0 });
        self.frames.push(Frame {
            locals,
            inherited: vec![result.region.clone()],
            imports: result.imports.clone(),
            libraries: result.libraries.clone(),
        });
    }

    fn package_instance(&mut self, node: &SyntaxNode) -> Arc<Region> {
        let mut region = None;
        for child in node.children() {
            match child.kind() {
                SyntaxKind::SELECTED_NAME | SyntaxKind::NAME if region.is_none() => {
                    region = Some(match self.name_expr(&child, Access::Other) {
                        Lookup::Found(targets) => self.members_of(&targets),
                        _ => None,
                    });
                }
                SyntaxKind::GENERIC_MAP_ASPECT => {
                    self.associations(&child, None, Interface::Generic)
                }
                _ => self.visit(&child),
            }
        }
        region
            .flatten()
            .unwrap_or_else(|| Arc::new(Region::opaque()))
    }

    fn configuration(&mut self, node: &SyntaxNode) {
        // Only the entity is checked; block configurations are not analyzed.
        if let Some(name) = node.children().find(|n| n.kind() == SyntaxKind::NAME)
            && let Some(token) = simple_name_token(&name)
        {
            let key = identifier_key(token.text());
            match self.ctx.unit_in_library(&self.library, &key, self.file) {
                Some(unit) => self.refs.push(Reference {
                    range: token.text_range(),
                    targets: smallvec![Target::Decl(DeclId { unit, index: 0 })],
                    access: Access::Other,
                }),
                None => self.error(
                    codes::UNRESOLVED_UNIT,
                    token.text_range(),
                    format!("no entity `{key}` in library `{}`", self.library),
                ),
            }
        }
        for child in node.children() {
            if child.kind() == SyntaxKind::USE_CLAUSE || child.kind() == SyntaxKind::ATTRIBUTE_SPEC
            {
                self.visit(&child);
            }
        }
    }

    // ---- Declarations -----------------------------------------------------

    /// Makes every failed lookup in the current scope unknown.
    fn push_opaque(&mut self) {
        let id = self.new_import_id();
        self.frames.last_mut().unwrap().imports.push(Import {
            id,
            kind: ImportKind::Opaque,
        });
    }

    fn new_import_id(&self) -> ImportId {
        ImportId {
            unit: self.unit,
            index: self.use_items.len() as u32 + 1_000_000,
        }
    }

    fn push_decl(
        &mut self,
        name: Option<&SyntaxToken>,
        kind: DeclKind,
        node: &SyntaxNode,
    ) -> DeclId {
        let (key, spelling, range) = match name {
            Some(t) => (designator_key(t), t.text().to_owned(), t.text_range()),
            None => (String::new(), String::new(), node.text_range()),
        };
        let id = DeclId {
            unit: self.unit,
            index: self.decls.len() as u32,
        };
        self.decls.push(Decl {
            name: key,
            spelling,
            kind,
            file: self.file,
            range,
            full_range: node.text_range(),
            members: None,
            parent: self.parents.last().copied(),
            subtype: None,
            type_def: None,
            params: None,
            width: None,
            value: None,
        });
        id
    }

    /// Declares `name` in the innermost region.
    fn declare(&mut self, name: &SyntaxToken, kind: DeclKind, node: &SyntaxNode) -> DeclId {
        let id = self.push_decl(Some(name), kind, node);
        let key = self.decls[id.index as usize].name.clone();
        let frame = self.frames.last().unwrap();
        let clash = frame.locals.get(&key).iter().copied().find(|&other| {
            let other_kind = self.decl(other).map(|d| d.kind);
            !kind.is_overloadable()
                && !other_kind.is_some_and(DeclKind::is_overloadable)
                && !matches!(
                    (other_kind, kind),
                    (Some(DeclKind::IncompleteType), DeclKind::Type)
                        | (Some(DeclKind::IncompleteType), DeclKind::IncompleteType)
                )
        });
        if let Some(other) = clash
            && let Some(prev) = self.decl(other).map(|d| d.range)
        {
            let line = self.ctx.line_of(self.file, prev.start());
            self.error(
                codes::DUPLICATE_DECLARATION,
                name.text_range(),
                format!(
                    "`{}` is already declared in this region (line {line})",
                    name.text()
                ),
            );
        }
        self.frames.last_mut().unwrap().locals.insert(&key, id);
        id
    }

    fn decl(&self, id: DeclId) -> Option<&Decl> {
        if id.unit == self.unit {
            self.decls.get(id.index as usize)
        } else {
            self.ctx.decl(id)
        }
    }

    /// The declaration that subprogram body `body` completes: one of the
    /// same kind, name and profile, declared earlier in the region or in
    /// the package or protected type the region is the body of.
    fn subprogram_declaration(&self, body: DeclId) -> Option<DeclId> {
        let decl = self.decl(body)?;
        let profile = |d: &Decl| -> Option<Vec<Option<DeclId>>> {
            let mut marks: Vec<Option<DeclId>> = d
                .params
                .as_deref()?
                .iter()
                .map(|&p| self.decl(p).and_then(|p| p.subtype))
                .collect();
            marks.push(d.subtype);
            Some(marks)
        };
        let wanted = profile(decl)?;
        // A unit's own frame sits on the frame of what it is the body of.
        let frame = self.frames.last()?;
        let below = self
            .frames
            .len()
            .checked_sub(2)
            .map(|i| &self.frames[i])
            .filter(|_| frame.inherited.is_empty());
        std::iter::once(&frame.locals)
            .chain(frame.inherited.iter().map(|r| &**r))
            .chain(
                below
                    .into_iter()
                    .flat_map(|f| f.inherited.iter().map(|r| &**r)),
            )
            .flat_map(|region| region.get(&decl.name).iter().copied())
            .find(|&c| {
                c != body
                    && self.decl(c).is_some_and(|d| {
                        d.kind == decl.kind
                            && (c.unit != body.unit || c.index < body.index)
                            && profile(d).as_ref() == Some(&wanted)
                    })
            })
    }

    /// Records references for the label repeated after `end`.
    fn end_label_refs(&mut self, node: &SyntaxNode, name: &SyntaxToken, id: DeclId) {
        let key = designator_key(name);
        let mut after_end = false;
        for token in node.children_with_tokens().filter_map(|e| e.into_token()) {
            match token.kind() {
                SyntaxKind::END_KW => after_end = true,
                SyntaxKind::IDENT | SyntaxKind::EXT_IDENT | SyntaxKind::STRING_LITERAL
                    if after_end && designator_key(&token) == key =>
                {
                    self.refs.push(Reference {
                        range: token.text_range(),
                        targets: smallvec![Target::Decl(id)],
                        access: Access::Other,
                    });
                }
                _ => {}
            }
        }
    }

    fn ident_list(&mut self, node: &SyntaxNode, kind: DeclKind) -> Vec<DeclId> {
        let mut ids = Vec::new();
        if let Some(list) = node.children().find(|n| n.kind() == SyntaxKind::IDENT_LIST) {
            for token in idents(&list) {
                ids.push(self.declare(&token, kind, node));
            }
        }
        ids
    }

    fn object_decl(&mut self, node: &SyntaxNode, kind: DeclKind) {
        // The subtype and default come first: `signal s : t := s;` is wrong.
        for child in node.children() {
            match child.kind() {
                SyntaxKind::IDENT_LIST => {}
                SyntaxKind::SUBTYPE_INDICATION => {
                    self.with_access(Access::Other, |s| s.visit(&child))
                }
                _ => self.with_access(Access::Read, |s| s.visit(&child)),
            }
        }
        let ids = self.ident_list(node, kind);
        let subtype = node
            .children()
            .find(|n| n.kind() == SyntaxKind::SUBTYPE_INDICATION)
            .and_then(|n| self.subtype_mark(&n));
        let width = node
            .children()
            .find(|n| n.kind() == SyntaxKind::SUBTYPE_INDICATION)
            .and_then(|n| self.indication_width(&n));
        let value = if kind == DeclKind::Constant {
            node.children_with_tokens()
                .skip_while(|e| e.kind() != SyntaxKind::COLON_EQ)
                .find_map(|e| e.into_node())
                .and_then(|e| self.static_value(&e))
        } else {
            None
        };
        for &id in &ids {
            let decl = &mut self.decls[id.index as usize];
            decl.subtype = subtype;
            decl.width = width;
            decl.value = value;
        }
        if kind == DeclKind::Constant {
            // The full declaration of a deferred constant completes the one
            // in the package: it is used wherever that one is.
            for id in ids {
                let name = self.decls[id.index as usize].name.clone();
                let range = self.decls[id.index as usize].range;
                let deferred = self.frames.iter().rev().find_map(|f| {
                    f.inherited.iter().find_map(|r| {
                        r.get(&name)
                            .iter()
                            .copied()
                            .find(|&d| self.decl(d).is_some_and(|d| d.kind == DeclKind::Constant))
                    })
                });
                if let Some(deferred) = deferred {
                    self.record(
                        range,
                        smallvec![Target::Decl(deferred), Target::Decl(id)],
                        Access::Other,
                    );
                }
            }
        }
    }

    fn interface_object(&mut self, node: &SyntaxNode) {
        let has_default = has_token(node, SyntaxKind::COLON_EQ);
        let mut mode = Mode::In;
        for token in node.children_with_tokens().filter_map(|e| e.into_token()) {
            mode = match token.kind() {
                SyntaxKind::OUT_KW => Mode::Out,
                SyntaxKind::INOUT_KW => Mode::InOut,
                SyntaxKind::BUFFER_KW => Mode::Buffer,
                SyntaxKind::LINKAGE_KW => Mode::Linkage,
                _ => continue,
            };
        }
        if node
            .children()
            .any(|n| n.kind() == SyntaxKind::MODE_VIEW_INDICATION)
        {
            mode = Mode::View;
        }
        let kind = match self.interface {
            Interface::Generic => DeclKind::Generic { has_default },
            Interface::Port => DeclKind::Port { mode, has_default },
            Interface::Parameter => DeclKind::Parameter { mode, has_default },
        };
        self.object_decl(node, kind);
    }

    fn with_interface(&mut self, interface: Interface, f: impl FnOnce(&mut Self)) {
        let saved = std::mem::replace(&mut self.interface, interface);
        f(self);
        self.interface = saved;
    }

    fn with_access(&mut self, access: Access, f: impl FnOnce(&mut Self)) {
        let saved = std::mem::replace(&mut self.access, access);
        f(self);
        self.access = saved;
    }

    fn type_decl(&mut self, node: &SyntaxNode) {
        let Some(name) = designator(node) else {
            return self.visit_children(node);
        };
        let def = node
            .children()
            .find(|n| !matches!(n.kind(), SyntaxKind::LABEL));
        match def.as_ref().map(SyntaxNode::kind) {
            None => {
                self.declare(&name, DeclKind::IncompleteType, node);
            }
            Some(SyntaxKind::PROTECTED_TYPE_BODY) => {
                let def = def.unwrap();
                let id = self.push_decl(Some(&name), DeclKind::ProtectedBody, node);
                let mut frame = Frame::default();
                match self.lookup(&designator_key(&name)) {
                    Lookup::Found(targets) => {
                        self.refs.push(Reference {
                            range: name.text_range(),
                            targets: targets.clone(),
                            access: Access::Other,
                        });
                        if let Some(members) = self.members_of(&targets) {
                            frame.inherited.push(members);
                        }
                    }
                    _ => frame.imports.push(Import {
                        id: self.new_import_id(),
                        kind: ImportKind::Opaque,
                    }),
                }
                self.parents.push(id);
                self.frames.push(frame);
                self.visit_children(&def);
                self.frames.pop();
                self.parents.pop();
                self.end_label_refs(&def, &name, id);
            }
            Some(SyntaxKind::PROTECTED_TYPE_DEF) => {
                let def = def.unwrap();
                let id = self.declare(&name, DeclKind::Type, node);
                self.decls[id.index as usize].type_def = Some(Arc::new(TypeDef::Protected));
                self.parents.push(id);
                self.frames.push(Frame::default());
                self.visit_children(&def);
                let members = Arc::new(self.frames.pop().unwrap().locals);
                self.parents.pop();
                self.decls[id.index as usize].members = Some(members);
                self.end_label_refs(&def, &name, id);
            }
            Some(_) => {
                let def = def.unwrap();
                let id = self.declare(&name, DeclKind::Type, node);
                self.complete_incomplete_type(&name, id);
                let type_def = match def.kind() {
                    SyntaxKind::ENUM_TYPE_DEF => {
                        let mut literals = Region::default();
                        for token in idents(&def) {
                            let literal = self.declare(&token, DeclKind::EnumLiteral, &def);
                            self.decls[literal.index as usize].subtype = Some(id);
                            literals.insert(&designator_key(&token), literal);
                        }
                        self.decls[id.index as usize].members = Some(Arc::new(literals));
                        let keys = def
                            .children_with_tokens()
                            .filter_map(|e| e.into_token())
                            .filter(|t| {
                                matches!(
                                    t.kind(),
                                    SyntaxKind::IDENT
                                        | SyntaxKind::EXT_IDENT
                                        | SyntaxKind::CHAR_LITERAL
                                )
                            })
                            .map(|t| designator_key(&t))
                            .collect();
                        Some(TypeDef::Enum(keys))
                    }
                    SyntaxKind::PHYSICAL_TYPE_DEF => {
                        let mut units = Region::default();
                        for child in def.children() {
                            if child.kind() == SyntaxKind::UNIT_DECL {
                                self.visit_children(&child);
                                if let Some(unit) = designator(&child) {
                                    let unit_id =
                                        self.declare(&unit, DeclKind::PhysicalUnit, &child);
                                    self.decls[unit_id.index as usize].subtype = Some(id);
                                    units.insert(&designator_key(&unit), unit_id);
                                }
                            } else {
                                self.visit(&child);
                            }
                        }
                        self.decls[id.index as usize].members = Some(Arc::new(units));
                        self.end_label_refs(&def, &name, id);
                        Some(TypeDef::Physical)
                    }
                    SyntaxKind::RECORD_TYPE_DEF => {
                        // Element names are not declared in the region.
                        let mut elements = Vec::new();
                        for element in def.children() {
                            let mut subtype = None;
                            for child in element.children() {
                                if child.kind() != SyntaxKind::IDENT_LIST {
                                    self.with_access(Access::Other, |s| s.visit(&child));
                                    if child.kind() == SyntaxKind::SUBTYPE_INDICATION {
                                        subtype = self.subtype_mark(&child);
                                    }
                                }
                            }
                            if let Some(list) = element
                                .children()
                                .find(|n| n.kind() == SyntaxKind::IDENT_LIST)
                            {
                                for token in idents(&list) {
                                    elements.push(Element {
                                        name: designator_key(&token),
                                        spelling: token.text().to_owned(),
                                        subtype,
                                    });
                                }
                            }
                        }
                        self.end_label_refs(&def, &name, id);
                        Some(TypeDef::Record(elements))
                    }
                    _ => {
                        self.with_access(Access::Other, |s| s.visit(&def));
                        if def.kind() == SyntaxKind::ARRAY_TYPE_DEF {
                            // `array (0 to 7) of bit`: a constrained array.
                            let mut indexes = def
                                .children()
                                .filter(|n| !matches!(n.kind(), SyntaxKind::SUBTYPE_INDICATION));
                            if let (Some(index), None) = (indexes.next(), indexes.next()) {
                                let value = |n: &SyntaxNode| self.named_value(n);
                                self.decls[id.index as usize].width =
                                    crate::rtl::range_width(&index, &value);
                            }
                        }
                        self.type_definition(&def)
                    }
                };
                self.decls[id.index as usize].type_def = type_def.map(Arc::new);
            }
        }
    }

    /// A subprogram specification: declares the name and pushes a frame
    /// holding its parameters. The caller pops it.
    fn subprogram_spec(
        &mut self,
        spec: &SyntaxNode,
        kind: DeclKind,
        node: &SyntaxNode,
    ) -> Option<DeclId> {
        let name = designator(spec);
        let id = match &name {
            Some(name) => self.declare(name, kind, node),
            None => self.push_decl(None, kind, node),
        };
        self.parents.push(id);
        self.frames.push(Frame::default());
        let mut params = Vec::new();
        let mut after_return = false;
        for child in spec.children_with_tokens() {
            let child = match child {
                rowan::NodeOrToken::Token(t) => {
                    after_return |= t.kind() == SyntaxKind::RETURN_KW;
                    continue;
                }
                rowan::NodeOrToken::Node(n) => n,
            };
            match child.kind() {
                SyntaxKind::INTERFACE_LIST => {
                    let start = self.decls.len();
                    self.with_interface(Interface::Parameter, |s| s.visit(&child));
                    params.extend(
                        (start..self.decls.len())
                            .filter(|&i| {
                                let d = &self.decls[i];
                                d.parent == Some(id) && matches!(d.kind, DeclKind::Parameter { .. })
                            })
                            .map(|i| DeclId {
                                unit: self.unit,
                                index: i as u32,
                            }),
                    );
                }
                SyntaxKind::GENERIC_CLAUSE => {
                    self.with_interface(Interface::Generic, |s| s.visit(&child))
                }
                _ => {
                    self.with_access(Access::Other, |s| s.visit(&child));
                    if after_return {
                        self.decls[id.index as usize].subtype = self.type_mark(&child);
                    }
                }
            }
        }
        self.decls[id.index as usize].params = Some(params.into());
        name.is_some().then_some(id)
    }

    fn subprogram_kind(spec: &SyntaxNode) -> DeclKind {
        if spec.kind() == SyntaxKind::PROCEDURE_SPEC {
            DeclKind::Procedure
        } else {
            DeclKind::Function
        }
    }

    fn component(&mut self, node: &SyntaxNode) {
        let Some(name) = designator(node) else {
            return self.visit_children(node);
        };
        let id = self.declare(&name, DeclKind::Component, node);
        self.parents.push(id);
        self.frames.push(Frame::default());
        self.visit_children(node);
        let members = Arc::new(self.frames.pop().unwrap().locals);
        self.parents.pop();
        self.decls[id.index as usize].members = Some(members);
        self.end_label_refs(node, &name, id);
    }

    /// A nested package, package body or package instantiation.
    fn nested_package(&mut self, node: &SyntaxNode) {
        let Some(name) = designator(node) else {
            return self.visit_children(node);
        };
        match node.kind() {
            SyntaxKind::PACKAGE_DECL => {
                let id = self.declare(&name, DeclKind::Package, node);
                self.parents.push(id);
                self.frames.push(Frame::default());
                self.visit_children(node);
                let members = Arc::new(self.frames.pop().unwrap().locals);
                self.parents.pop();
                self.decls[id.index as usize].members = Some(members);
                self.end_label_refs(node, &name, id);
            }
            SyntaxKind::PACKAGE_BODY => {
                let id = self.push_decl(Some(&name), DeclKind::PackageBody, node);
                let mut frame = Frame::default();
                if let Lookup::Found(targets) = self.lookup(&designator_key(&name)) {
                    self.refs.push(Reference {
                        range: name.text_range(),
                        targets: targets.clone(),
                        access: Access::Other,
                    });
                    if let Some(members) = self.members_of(&targets) {
                        frame.inherited.push(members);
                    }
                }
                self.parents.push(id);
                self.frames.push(frame);
                self.visit_children(node);
                self.frames.pop();
                self.parents.pop();
                self.end_label_refs(node, &name, id);
            }
            _ => {
                let id = self.declare(&name, DeclKind::PackageInstance, node);
                let members = self.package_instance(node);
                self.decls[id.index as usize].members = Some(members);
            }
        }
    }

    // ---- Clauses ----------------------------------------------------------

    fn library_clause(&mut self, node: &SyntaxNode) {
        for list in node.children() {
            for token in idents(&list) {
                let key = identifier_key(token.text());
                if !self.ctx.library_known(&key) && key != "work" && self.quiet == 0 {
                    self.diags.push(Diagnostic {
                        code: codes::UNKNOWN_LIBRARY,
                        file: self.file,
                        range: token.text_range(),
                        message: format!(
                            "library `{key}` is not part of the project; names from it are not checked"
                        ),
                        fix: None,
                    });
                }
                self.refs.push(Reference {
                    range: token.text_range(),
                    targets: smallvec![Target::Library(self.library_name(&key).into())],
                    access: Access::Other,
                });
                self.frames.last_mut().unwrap().libraries.push(key);
            }
        }
    }

    fn library_name(&self, key: &str) -> String {
        if key == "work" {
            self.library.clone()
        } else {
            key.to_owned()
        }
    }

    fn use_clause(&mut self, node: &SyntaxNode) {
        let items: Vec<SyntaxNode> = node
            .children()
            .filter(|n| matches!(n.kind(), SyntaxKind::SELECTED_NAME | SyntaxKind::NAME))
            .collect();
        let only_item = items.len() == 1;
        for item in items {
            let id = ImportId {
                unit: self.unit,
                index: self.use_items.len() as u32,
            };
            self.use_items.push(UseItem {
                range: item.text_range(),
                clause: node.text_range(),
                only_item,
                resolved: true,
            });
            let kinds = self.use_item(&item);
            if kinds.iter().any(|k| matches!(k, ImportKind::Opaque)) {
                self.use_items.last_mut().unwrap().resolved = false;
            }
            for kind in kinds {
                self.frames
                    .last_mut()
                    .unwrap()
                    .imports
                    .push(Import { id, kind });
            }
        }
    }

    fn use_item(&mut self, item: &SyntaxNode) -> Vec<ImportKind> {
        if item.kind() != SyntaxKind::SELECTED_NAME {
            return vec![ImportKind::Opaque];
        }
        let Some(prefix) = item.children().next() else {
            return vec![ImportKind::Opaque];
        };
        if has_token(item, SyntaxKind::ALL_KW) {
            return vec![match self.name_expr(&prefix, Access::Other) {
                Lookup::Found(targets) => {
                    if let [Target::Library(lib)] = targets.as_slice() {
                        ImportKind::Library(lib.to_string())
                    } else {
                        match self.members_of(&targets) {
                            Some(region) => ImportKind::All(region),
                            None => ImportKind::Opaque,
                        }
                    }
                }
                _ => ImportKind::Opaque,
            }];
        }
        match self.name_expr(item, Access::Other) {
            Lookup::Found(targets) => {
                let name = item
                    .children_with_tokens()
                    .filter_map(|e| e.into_token())
                    .filter(|t| is_designator(t.kind()))
                    .last()
                    .map(|t| designator_key(&t))
                    .unwrap_or_default();
                // Using a type also makes its literals or units visible.
                let literals = targets.iter().find_map(|t| match t {
                    Target::Decl(id) => self
                        .decl(*id)
                        .filter(|d| d.kind == DeclKind::Type)
                        .and_then(|d| d.members.clone())
                        .filter(|m| {
                            m.iter()
                                .next()
                                .and_then(|first| self.decl(first))
                                .is_some_and(|d| {
                                    matches!(d.kind, DeclKind::EnumLiteral | DeclKind::PhysicalUnit)
                                })
                        }),
                    _ => None,
                });
                let mut kinds = vec![ImportKind::Names(name, targets)];
                kinds.extend(literals.map(ImportKind::All));
                kinds
            }
            _ => vec![ImportKind::Opaque],
        }
    }

    fn context_reference(&mut self, node: &SyntaxNode) {
        for item in node.children() {
            let Lookup::Found(targets) = self.name_expr(&item, Access::Other) else {
                self.push_opaque();
                continue;
            };
            let unit = targets.iter().find_map(|t| match t {
                Target::Decl(id) if id.index == 0 => Some(id.unit),
                _ => None,
            });
            match unit.and_then(|u| self.ctx.result(u)) {
                Some(result) => {
                    let frame = self.frames.last_mut().unwrap();
                    frame.imports.extend(result.imports.iter().cloned());
                    frame.libraries.extend(result.libraries.iter().cloned());
                }
                None => self.push_opaque(),
            }
        }
    }

    // ---- Lookup -----------------------------------------------------------

    fn lookup(&mut self, name: &str) -> Lookup {
        // An inner declaration hides outer ones, except that overloadable
        // declarations (subprograms, enumeration literals) add up: every
        // visible overload is a candidate for overload resolution.
        let mut overloads = Targets::new();
        for frame in self.frames.iter().rev() {
            let mut ids = frame.locals.get(name);
            if ids.is_empty() {
                ids = frame
                    .inherited
                    .iter()
                    .map(|r| r.get(name))
                    .find(|ids| !ids.is_empty())
                    .unwrap_or(&[]);
            }
            if ids.is_empty() {
                continue;
            }
            let overloadable = ids.iter().all(|&id| self.is_overloadable(id));
            if !overloadable {
                if overloads.is_empty() {
                    return Lookup::Found(ids.iter().map(|&id| Target::Decl(id)).collect());
                }
                return Lookup::Found(overloads);
            }
            overloads.extend(ids.iter().map(|&id| Target::Decl(id)));
        }
        let mut found = Targets::new();
        let mut opaque = false;
        let mut used = Vec::new();
        for frame in self.frames.iter().rev() {
            opaque |= frame.inherited.iter().any(|r| r.opaque);
            for import in &frame.imports {
                match &import.kind {
                    ImportKind::All(region) => {
                        let ids = region.get(name);
                        if !ids.is_empty() {
                            found.extend(ids.iter().map(|&id| Target::Decl(id)));
                            used.push(import.id);
                        }
                        opaque |= region.opaque;
                    }
                    ImportKind::Names(n, targets) => {
                        if n == name {
                            found.extend(targets.iter().cloned());
                            used.push(import.id);
                        }
                    }
                    ImportKind::Library(lib) => {
                        if let Some(unit) = self.ctx.unit_in_library(lib, name, self.file) {
                            found.push(Target::Decl(DeclId { unit, index: 0 }));
                            used.push(import.id);
                        } else if !self.ctx.library_known(lib) {
                            opaque = true;
                        }
                    }
                    ImportKind::Opaque => opaque = true,
                }
            }
        }
        self.used_imports.extend(used);
        if !overloads.is_empty() {
            // Imported overloads join the local ones; other imported names
            // are hidden by them.
            for target in found {
                if let Target::Decl(id) = target
                    && self.is_overloadable(id)
                    && !overloads.contains(&target)
                {
                    overloads.push(target);
                }
            }
            return Lookup::Found(overloads);
        }
        if !found.is_empty() {
            return Lookup::Found(found);
        }
        for frame in self.frames.iter().rev() {
            if frame.libraries.iter().any(|l| l == name) {
                return Lookup::Found(smallvec![Target::Library(self.library_name(name).into())]);
            }
        }
        if opaque {
            Lookup::Unknown
        } else {
            Lookup::NotFound
        }
    }

    fn is_overloadable(&self, id: DeclId) -> bool {
        self.decl(id).is_some_and(|d| d.kind.is_overloadable())
    }

    // ---- Types ------------------------------------------------------------

    /// The type or subtype a subtype indication's type mark names.
    pub(crate) fn subtype_mark(&self, indication: &SyntaxNode) -> Option<DeclId> {
        let mark = indication.children().find(|n| {
            matches!(
                n.kind(),
                SyntaxKind::NAME | SyntaxKind::SELECTED_NAME | SyntaxKind::CALL_EXPR
            )
        })?;
        self.type_mark(&mark)
    }

    /// The type or subtype a resolved type mark (`t`, `pkg.t`, or `t(...)`
    /// with a constraint) names.
    fn type_mark(&self, mark: &SyntaxNode) -> Option<DeclId> {
        let token = mark_token(mark)?;
        let range = token.text_range();
        // The mark was just resolved: its reference is among the last ones.
        let r = self
            .refs
            .iter()
            .rev()
            .take(256)
            .find(|r| r.range == range)?;
        r.targets.iter().find_map(|t| match t {
            Target::Decl(id)
                if self.decl(*id).is_some_and(|d| {
                    matches!(
                        d.kind,
                        DeclKind::Type
                            | DeclKind::Subtype
                            | DeclKind::IncompleteType
                            | DeclKind::GenericType
                    )
                }) =>
            {
                Some(*id)
            }
            _ => None,
        })
    }

    /// The subtype of the object an alias names, when it names a whole
    /// object (`alias a is s`) or gives a subtype.
    fn alias_subtype(&self, node: &SyntaxNode) -> Option<DeclId> {
        if let Some(indication) = node
            .children()
            .find(|n| n.kind() == SyntaxKind::SUBTYPE_INDICATION)
        {
            return self.subtype_mark(&indication);
        }
        let name = node
            .children()
            .find(|n| matches!(n.kind(), SyntaxKind::NAME | SyntaxKind::SELECTED_NAME))?;
        let token = mark_token(&name)?;
        let r = self
            .refs
            .iter()
            .rev()
            .take(64)
            .find(|r| r.range == token.text_range())?;
        match r.targets.as_slice() {
            [Target::Decl(id)] => self
                .decl(*id)
                .filter(|d| d.kind.is_object())
                .and_then(|d| d.subtype),
            _ => None,
        }
    }

    /// Links an incomplete type declaration to its full declaration.
    fn complete_incomplete_type(&mut self, name: &SyntaxToken, full: DeclId) {
        let key = designator_key(name);
        let incomplete = self
            .frames
            .last()
            .unwrap()
            .locals
            .get(&key)
            .iter()
            .copied()
            .find(|&d| {
                d != full
                    && d.unit == self.unit
                    && self.decls[d.index as usize].kind == DeclKind::IncompleteType
            });
        if let Some(incomplete) = incomplete {
            self.decls[incomplete.index as usize].subtype = Some(full);
        }
    }

    /// A type declared in `std.standard`.
    fn standard_type(&self, name: &str) -> Option<DeclId> {
        match self.ctx.standard_region() {
            Some(region) => region.get(name).first().copied(),
            // Analyzing `std.standard` itself.
            None => self
                .frames
                .iter()
                .find_map(|f| f.locals.get(name).first().copied()),
        }
    }

    /// The definition of the base type of `id`, following subtypes.
    fn base_def(&self, mut id: DeclId) -> Option<Arc<TypeDef>> {
        for _ in 0..32 {
            let decl = self.decl(id)?;
            match decl.kind {
                DeclKind::Type => return decl.type_def.clone(),
                DeclKind::Subtype | DeclKind::IncompleteType => id = decl.subtype?,
                _ if decl.kind.is_object() => id = decl.subtype?,
                _ => return None,
            }
        }
        None
    }

    /// The definition of a range, array, access or file type.
    fn type_definition(&self, def: &SyntaxNode) -> Option<TypeDef> {
        match def.kind() {
            SyntaxKind::RANGE_TYPE_DEF => Some(self.range_class(def)),
            SyntaxKind::ARRAY_TYPE_DEF => {
                let mut indexes = Vec::new();
                let mut element = None;
                let mut after_of = false;
                for child in def.children_with_tokens() {
                    match child {
                        rowan::NodeOrToken::Token(t) => after_of |= t.kind() == SyntaxKind::OF_KW,
                        rowan::NodeOrToken::Node(n) if after_of => {
                            if n.kind() == SyntaxKind::SUBTYPE_INDICATION {
                                element = self.subtype_mark(&n);
                            }
                        }
                        rowan::NodeOrToken::Node(n) => indexes.push(self.index_subtype(&n)),
                    }
                }
                Some(TypeDef::Array { indexes, element })
            }
            SyntaxKind::ACCESS_TYPE_DEF => Some(TypeDef::Access(
                def.children()
                    .find(|n| n.kind() == SyntaxKind::SUBTYPE_INDICATION)
                    .and_then(|n| self.subtype_mark(&n)),
            )),
            SyntaxKind::FILE_TYPE_DEF => Some(TypeDef::File(
                def.children().next().and_then(|n| self.type_mark(&n)),
            )),
            _ => None,
        }
    }

    /// Integer or real, from the literals and names of a range.
    fn range_class(&self, range: &SyntaxNode) -> TypeDef {
        let mut integer = false;
        for token in range
            .descendants_with_tokens()
            .filter_map(|e| e.into_token())
        {
            match token.kind() {
                SyntaxKind::DECIMAL_LITERAL | SyntaxKind::BASED_LITERAL => {
                    if token.text().contains('.') {
                        return TypeDef::Real;
                    }
                    integer = true;
                }
                _ => {}
            }
        }
        if integer {
            return TypeDef::Integer;
        }
        // Only names: the class of the first one's type.
        let first = range
            .descendants()
            .find(|n| n.kind() == SyntaxKind::NAME)
            .and_then(|n| self.resolved_decl(&n));
        match first.and_then(|id| self.base_def(id)).as_deref() {
            Some(TypeDef::Integer) => TypeDef::Integer,
            Some(TypeDef::Real) => TypeDef::Real,
            _ => TypeDef::Scalar,
        }
    }

    /// The value of a constant a resolved name refers to.
    fn named_value(&self, name: &SyntaxNode) -> Option<i64> {
        self.decl(self.resolved_decl(name)?)?.value
    }

    /// The value of a static integer expression, just resolved.
    fn static_value(&self, expr: &SyntaxNode) -> Option<i64> {
        crate::rtl::static_int(expr, &|n| self.named_value(n))
    }

    /// The width of a subtype indication: `std_logic_vector(7 downto 0)`,
    /// or a subtype mark of known width.
    fn indication_width(&self, indication: &SyntaxNode) -> Option<u32> {
        let mark = indication.children().find(|n| {
            matches!(
                n.kind(),
                SyntaxKind::NAME | SyntaxKind::SELECTED_NAME | SyntaxKind::CALL_EXPR
            )
        })?;
        if mark.kind() == SyntaxKind::CALL_EXPR {
            let list = mark
                .children()
                .find(|n| n.kind() == SyntaxKind::ASSOCIATION_LIST)?;
            crate::rtl::constraint_width(&list, &|n| self.named_value(n))
        } else {
            self.decl(self.type_mark(&mark)?)?.width
        }
    }

    /// The single declaration a resolved name refers to.
    fn resolved_decl(&self, name: &SyntaxNode) -> Option<DeclId> {
        let token = mark_token(name)?;
        let r = self
            .refs
            .iter()
            .rev()
            .take(256)
            .find(|r| r.range == token.text_range())?;
        match r.targets.as_slice() {
            [Target::Decl(id)] => Some(*id),
            _ => None,
        }
    }

    /// The index subtype of one index of an array definition.
    fn index_subtype(&self, index: &SyntaxNode) -> Option<DeclId> {
        match index.kind() {
            SyntaxKind::INDEX_SUBTYPE_DEF => {
                index.children().next().and_then(|n| self.type_mark(&n))
            }
            SyntaxKind::SUBTYPE_INDICATION => self.subtype_mark(index),
            SyntaxKind::NAME | SyntaxKind::SELECTED_NAME => self.type_mark(index),
            SyntaxKind::RANGE_EXPR => {
                if index.descendants_with_tokens().any(|e| {
                    matches!(
                        e.kind(),
                        SyntaxKind::DECIMAL_LITERAL | SyntaxKind::BASED_LITERAL
                    )
                }) {
                    return self.standard_type("integer");
                }
                // `low to high` with names: the type of the first bound.
                let first = index
                    .descendants()
                    .find(|n| n.kind() == SyntaxKind::NAME)
                    .and_then(|n| self.resolved_decl(&n))?;
                let decl = self.decl(first)?;
                match decl.kind {
                    DeclKind::EnumLiteral => decl.subtype,
                    _ if decl.kind.is_object() => decl.subtype,
                    _ => None,
                }
            }
            _ => None,
        }
    }

    /// What can be selected from `targets`: a package's declarations, a
    /// library unit's region.
    fn members_of(&self, targets: &[Target]) -> Option<Arc<Region>> {
        targets.iter().find_map(|t| match t {
            Target::Decl(id) => {
                let decl = self.decl(*id)?;
                match decl.kind {
                    DeclKind::Package
                    | DeclKind::PackageInstance
                    | DeclKind::GenericPackage
                    | DeclKind::Context
                    | DeclKind::Entity
                    | DeclKind::Component
                    | DeclKind::Type => decl.members.clone(),
                    _ => None,
                }
            }
            Target::Library(_) => None,
        })
    }

    fn error(&mut self, code: &'static str, range: TextRange, message: String) {
        if self.quiet == 0 {
            self.diags.push(Diagnostic {
                code,
                file: self.file,
                range,
                message,
                fix: None,
            });
        }
    }

    fn record(&mut self, range: TextRange, targets: Targets, access: Access) {
        self.refs.push(Reference {
            range,
            targets,
            access,
        });
    }

    // ---- Names ------------------------------------------------------------

    /// Resolves a name expression and records its references.
    fn name_expr(&mut self, node: &SyntaxNode, access: Access) -> Lookup {
        match node.kind() {
            SyntaxKind::NAME => self.simple_name(node, access),
            SyntaxKind::SELECTED_NAME => self.selected_name(node, access),
            SyntaxKind::CALL_EXPR => {
                let mut children = node.children();
                if let Some(prefix) = children.next() {
                    self.name_expr(&prefix, access);
                }
                for rest in children {
                    if rest.kind() == SyntaxKind::ASSOCIATION_LIST {
                        self.call_arguments(&rest);
                    } else {
                        self.visit(&rest);
                    }
                }
                Lookup::Unknown
            }
            SyntaxKind::ATTRIBUTE_NAME => {
                self.attribute_name(node);
                Lookup::Unknown
            }
            _ => {
                self.with_access(access, |s| s.visit(node));
                Lookup::Unknown
            }
        }
    }

    fn simple_name(&mut self, node: &SyntaxNode, access: Access) -> Lookup {
        let Some(token) = node
            .children_with_tokens()
            .filter_map(|e| e.into_token())
            .find(|t| is_designator(t.kind()))
        else {
            self.visit_children(node);
            return Lookup::Unknown;
        };
        if token.kind() == SyntaxKind::CHAR_LITERAL {
            return Lookup::Unknown;
        }
        let key = designator_key(&token);
        let result = self.lookup(&key);
        match &result {
            Lookup::Found(targets) => {
                self.record(token.text_range(), targets.clone(), access);
            }
            Lookup::NotFound if token.kind() != SyntaxKind::STRING_LITERAL && self.soft == 0 => {
                self.error(
                    codes::UNRESOLVED_NAME,
                    token.text_range(),
                    format!("`{}` is not declared", token.text()),
                );
            }
            _ => {}
        }
        result
    }

    fn selected_name(&mut self, node: &SyntaxNode, access: Access) -> Lookup {
        let Some(prefix) = node.children().next() else {
            return Lookup::Unknown;
        };
        let suffix = node
            .children_with_tokens()
            .filter_map(|e| e.into_token())
            .find(|t| is_designator(t.kind()) || t.kind() == SyntaxKind::ALL_KW);
        let prefix_result = self.name_expr(&prefix, access);
        let Some(suffix) = suffix else {
            return Lookup::Unknown;
        };
        if suffix.kind() == SyntaxKind::ALL_KW || suffix.kind() == SyntaxKind::CHAR_LITERAL {
            return Lookup::Unknown;
        }
        let key = designator_key(&suffix);
        let Lookup::Found(targets) = prefix_result else {
            return Lookup::Unknown;
        };
        if let [Target::Library(lib)] = targets.as_slice() {
            if let Some(unit) = self.ctx.unit_in_library(lib, &key, self.file) {
                let target = smallvec![Target::Decl(DeclId { unit, index: 0 })];
                if self.ctx.result(unit).is_none() {
                    return Lookup::Unknown;
                }
                self.record(suffix.text_range(), target.clone(), Access::Other);
                return Lookup::Found(target);
            }
            if self.ctx.library_known(lib) && !self.ctx.opaque_unit(lib, &key) {
                self.error(
                    codes::UNRESOLVED_UNIT,
                    suffix.text_range(),
                    format!("no unit `{}` in library `{lib}`", suffix.text()),
                );
                return Lookup::NotFound;
            }
            return Lookup::Unknown;
        }
        let package = targets.iter().find_map(|t| match t {
            Target::Decl(id) => self.decl(*id).filter(|d| {
                matches!(
                    d.kind,
                    DeclKind::Package | DeclKind::PackageInstance | DeclKind::GenericPackage
                )
            }),
            _ => None,
        });
        let Some(package) = package else {
            return Lookup::Unknown;
        };
        let Some(members) = package.members.clone() else {
            return Lookup::Unknown;
        };
        let package_name = package.spelling.clone();
        let ids = members.get(&key);
        if !ids.is_empty() {
            let found: Targets = ids.iter().map(|&id| Target::Decl(id)).collect();
            self.record(suffix.text_range(), found.clone(), access);
            return Lookup::Found(found);
        }
        if members.opaque || suffix.kind() == SyntaxKind::STRING_LITERAL {
            return Lookup::Unknown;
        }
        self.error(
            codes::UNRESOLVED_NAME,
            suffix.text_range(),
            format!(
                "`{}` is not declared in package `{package_name}`",
                suffix.text()
            ),
        );
        Lookup::NotFound
    }

    fn attribute_name(&mut self, node: &SyntaxNode) {
        let attribute = node
            .children_with_tokens()
            .filter_map(|e| e.into_token())
            .skip_while(|t| t.kind() != SyntaxKind::TICK)
            .nth(1)
            .map(|t| t.text().to_ascii_lowercase());
        let reads_value = matches!(
            attribute.as_deref(),
            Some(
                "event"
                    | "active"
                    | "last_event"
                    | "last_active"
                    | "last_value"
                    | "stable"
                    | "quiet"
                    | "delayed"
                    | "transaction"
                    | "driving"
                    | "driving_value"
            )
        );
        let mut children = node.children();
        if let Some(prefix) = children.next() {
            let access = if reads_value {
                Access::Read
            } else {
                Access::Other
            };
            self.name_expr(&prefix, access);
        }
        for rest in children {
            self.with_access(Access::Read, |s| s.visit(&rest));
        }
    }

    /// The argument list of a call or index: formals are not checked.
    fn call_arguments(&mut self, list: &SyntaxNode) {
        for element in list.children() {
            if element.kind() != SyntaxKind::ASSOCIATION_ELEMENT {
                self.visit(&element);
                continue;
            }
            let has_arrow = has_token(&element, SyntaxKind::ARROW);
            let mut past_arrow = !has_arrow;
            for child in element.children_with_tokens() {
                match child {
                    rowan::NodeOrToken::Token(t) if t.kind() == SyntaxKind::ARROW => {
                        past_arrow = true;
                    }
                    rowan::NodeOrToken::Node(n) if past_arrow => {
                        if self.in_subtype > 0 && n.kind() == SyntaxKind::CALL_EXPR {
                            // A record element constraint: `rec_t(field(7 downto 0))`.
                            let mut parts = n.children();
                            if let Some(prefix) = parts.next() {
                                self.soft += 1;
                                self.name_expr(&prefix, Access::Other);
                                self.soft -= 1;
                            }
                            for rest in parts {
                                if rest.kind() == SyntaxKind::ASSOCIATION_LIST {
                                    self.call_arguments(&rest);
                                } else {
                                    self.visit(&rest);
                                }
                            }
                        } else {
                            self.visit(&n);
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    // ---- Statements -------------------------------------------------------

    /// Records what an operator symbol may denote.
    fn operator_ref(&mut self, node: &SyntaxNode) {
        let Some(op) = operator_token(node) else {
            return;
        };
        let key = format!("\"{}\"", op.text().to_ascii_lowercase());
        match self.lookup(&key) {
            Lookup::Found(targets) => self.record(op.text_range(), targets, Access::Other),
            Lookup::Unknown => self.opaque_ops.push(op.text_range()),
            Lookup::NotFound => {}
        }
    }

    fn visit_children(&mut self, node: &SyntaxNode) {
        for child in node.children() {
            self.visit(&child);
        }
    }

    fn visit_children_except(&mut self, node: &SyntaxNode, skip: impl Fn(&SyntaxNode) -> bool) {
        for child in node.children() {
            if !skip(&child) {
                self.visit(&child);
            }
        }
    }

    /// Declares a statement's label, if it has one.
    fn label(&mut self, node: &SyntaxNode) -> Option<(SyntaxToken, DeclId)> {
        let label = node.children().find(|n| n.kind() == SyntaxKind::LABEL)?;
        let token = designator(&label)?;
        let id = self.declare(&token, DeclKind::Label, node);
        Some((token, id))
    }

    fn scoped_statement(&mut self, node: &SyntaxNode) {
        let label = self.label(node);
        if let Some((_, id)) = &label {
            self.parents.push(*id);
        }
        self.frames.push(Frame::default());
        if node.kind() == SyntaxKind::BLOCK_STMT && has_token(node, SyntaxKind::LPAREN) {
            // A guarded block declares the implicit signal GUARD.
            let id = self.push_decl(None, DeclKind::Signal, node);
            let decl = &mut self.decls[id.index as usize];
            decl.name = "guard".into();
            decl.spelling = "GUARD".into();
            decl.range = first_token(node, SyntaxKind::BLOCK_KW)
                .map_or(node.text_range(), |t| t.text_range());
            self.frames.last_mut().unwrap().locals.insert("guard", id);
            self.implicit.push(id);
        }
        self.visit_children_except(node, |n| n.kind() == SyntaxKind::LABEL);
        self.frames.pop();
        if let Some((token, id)) = label {
            self.parents.pop();
            self.end_label_refs(node, &token, id);
        }
    }

    fn assignment(&mut self, node: &SyntaxNode, operator: SyntaxKind) {
        self.label(node);
        let mut seen_operator = false;
        let mut target_done = false;
        let after_select = node.kind() == SyntaxKind::SELECTED_ASSIGN_STMT;
        let mut seen_select = false;
        for child in node.children_with_tokens() {
            match child {
                rowan::NodeOrToken::Token(t) => {
                    if t.kind() == operator {
                        seen_operator = true;
                    }
                    if t.kind() == SyntaxKind::SELECT_KW {
                        seen_select = true;
                    }
                }
                rowan::NodeOrToken::Node(n) => {
                    if n.kind() == SyntaxKind::LABEL {
                        continue;
                    }
                    let is_target =
                        !seen_operator && !target_done && (!after_select || seen_select);
                    if is_target {
                        target_done = true;
                        self.target(&n);
                    } else {
                        self.with_access(Access::Read, |s| s.visit(&n));
                    }
                }
            }
        }
    }

    /// An assignment target: the named object is written, indexes are read.
    fn target(&mut self, node: &SyntaxNode) {
        match node.kind() {
            SyntaxKind::NAME | SyntaxKind::SELECTED_NAME | SyntaxKind::CALL_EXPR => {
                self.name_expr(node, Access::Write);
            }
            SyntaxKind::AGGREGATE => {
                for element in node.children() {
                    for child in element.children() {
                        if child.kind() == SyntaxKind::CHOICES {
                            self.soft += 1;
                            self.visit(&child);
                            self.soft -= 1;
                        } else {
                            self.target(&child);
                        }
                    }
                }
            }
            SyntaxKind::PAREN_EXPR => {
                for child in node.children() {
                    self.target(&child);
                }
            }
            _ => self.with_access(Access::ReadWrite, |s| s.visit(node)),
        }
    }

    fn exit_or_next(&mut self, node: &SyntaxNode) {
        if let Some(token) = designator(node) {
            match self.lookup(&designator_key(&token)) {
                Lookup::Found(targets) => self.record(token.text_range(), targets, Access::Other),
                Lookup::NotFound => self.error(
                    codes::UNRESOLVED_NAME,
                    token.text_range(),
                    format!("no loop labeled `{}`", token.text()),
                ),
                Lookup::Unknown => {}
            }
        }
        self.with_access(Access::Read, |s| s.visit_children(node));
    }

    fn loop_stmt(&mut self, node: &SyntaxNode) {
        let label = self.label(node);
        self.frames.push(Frame::default());
        for child in node.children() {
            match child.kind() {
                SyntaxKind::LABEL => {}
                SyntaxKind::FOR_SCHEME => self.for_scheme(&child, DeclKind::LoopParameter),
                _ => self.visit(&child),
            }
        }
        self.frames.pop();
        if let Some((token, id)) = label {
            self.end_label_refs(node, &token, id);
        }
    }

    fn for_scheme(&mut self, node: &SyntaxNode, kind: DeclKind) {
        self.with_access(Access::Read, |s| s.visit_children(node));
        if let Some(param) = designator(node) {
            self.declare(&param, kind, node);
        }
    }

    fn for_generate(&mut self, node: &SyntaxNode) {
        let label = self.label(node);
        if let Some((_, id)) = &label {
            self.parents.push(*id);
        }
        self.frames.push(Frame::default());
        for child in node.children() {
            if child.kind() != SyntaxKind::LABEL && child.kind() != SyntaxKind::GENERATE_BODY {
                self.with_access(Access::Read, |s| s.visit(&child));
            }
        }
        if let Some(param) = designator(node) {
            self.declare(&param, DeclKind::GenerateParameter, node);
        }
        for body in node
            .children()
            .filter(|n| n.kind() == SyntaxKind::GENERATE_BODY)
        {
            self.generate_body(&body);
        }
        self.frames.pop();
        if let Some((token, id)) = label {
            self.parents.pop();
            self.end_label_refs(node, &token, id);
        }
    }

    fn generate_body(&mut self, body: &SyntaxNode) {
        let label = self.label(body);
        self.frames.push(Frame::default());
        self.visit_children_except(body, |n| n.kind() == SyntaxKind::LABEL);
        self.frames.pop();
        if let Some((token, id)) = label {
            self.end_label_refs(body, &token, id);
        }
    }

    fn conditional_generate(&mut self, node: &SyntaxNode) {
        let label = self.label(node);
        if let Some((_, id)) = &label {
            self.parents.push(*id);
        }
        for child in node.children() {
            match child.kind() {
                SyntaxKind::LABEL => {}
                SyntaxKind::GENERATE_BODY => self.generate_body(&child),
                SyntaxKind::CASE_ALTERNATIVE => {
                    for inner in child.children() {
                        if inner.kind() == SyntaxKind::GENERATE_BODY {
                            self.generate_body(&inner);
                        } else {
                            self.visit(&inner);
                        }
                    }
                }
                _ => self.with_access(Access::Read, |s| s.visit(&child)),
            }
        }
        if let Some((token, id)) = label {
            self.parents.pop();
            self.end_label_refs(node, &token, id);
        }
    }

    fn process(&mut self, node: &SyntaxNode) {
        let label = self.label(node);
        let id = match &label {
            Some((_, id)) => *id,
            None => {
                let id = self.push_decl(None, DeclKind::Label, node);
                self.decls[id.index as usize].range = first_token(node, SyntaxKind::PROCESS_KW)
                    .map_or(node.text_range(), |t| t.text_range());
                id
            }
        };
        self.parents.push(id);
        self.frames.push(Frame::default());
        let refs_start = self.refs.len();
        let mut sensitivity = None;
        for child in node.children() {
            match child.kind() {
                SyntaxKind::LABEL => {}
                SyntaxKind::SENSITIVITY_LIST => {
                    let start = self.refs.len();
                    self.with_access(Access::Read, |s| s.visit_children(&child));
                    sensitivity = Some((child.clone(), start..self.refs.len()));
                }
                _ => self.with_access(Access::Read, |s| s.visit(&child)),
            }
        }
        self.frames.pop();
        self.parents.pop();
        if let Some((list, sens_refs)) = sensitivity
            && !has_token(&list, SyntaxKind::ALL_KW)
        {
            self.check_sensitivity(node, &list, sens_refs, refs_start);
        }
        if let Some((token, id)) = label {
            self.end_label_refs(node, &token, id);
        }
    }

    fn instantiation(&mut self, node: &SyntaxNode) {
        self.label(node);
        let is_entity = has_token(node, SyntaxKind::ENTITY_KW);
        let is_configuration = has_token(node, SyntaxKind::CONFIGURATION_KW);
        let mut target: Option<(Arc<Region>, String)> = None;
        let mut known_target = false;
        for child in node.children() {
            match child.kind() {
                SyntaxKind::LABEL => {}
                SyntaxKind::NAME | SyntaxKind::SELECTED_NAME | SyntaxKind::CALL_EXPR
                    if !known_target && target.is_none() =>
                {
                    known_target = true;
                    let (name, arch) = if child.kind() == SyntaxKind::CALL_EXPR {
                        let mut parts = child.children();
                        (parts.next(), parts.next())
                    } else {
                        (Some(child.clone()), None)
                    };
                    let Some(name) = name else { continue };
                    let lookup = self.name_expr(&name, Access::Other);
                    if let Lookup::Found(targets) = &lookup {
                        let decl = targets.iter().find_map(|t| match t {
                            Target::Decl(id) => self.decl(*id).map(|d| (*id, d.clone())),
                            _ => None,
                        });
                        if let Some((id, decl)) = decl {
                            let expected = if is_entity {
                                DeclKind::Entity
                            } else if is_configuration {
                                DeclKind::Configuration
                            } else {
                                DeclKind::Component
                            };
                            if decl.kind == expected {
                                if let Some(members) = decl.members.clone()
                                    && expected != DeclKind::Configuration
                                {
                                    let what =
                                        if self.ctx.files[decl.file as usize].foreign.is_some() {
                                            "module"
                                        } else {
                                            decl.kind.describe()
                                        };
                                    target = Some((members, format!("{what} `{}`", decl.spelling)));
                                }
                            } else if self.quiet == 0 {
                                self.error(
                                    codes::UNRESOLVED_UNIT,
                                    name.text_range(),
                                    format!(
                                        "`{}` is a {}, not a {}",
                                        decl.spelling,
                                        decl.kind.describe(),
                                        expected.describe()
                                    ),
                                );
                            }
                            if let (true, Some(arch)) = (is_entity, arch) {
                                self.architecture_ref(&arch, id, &decl);
                            }
                        }
                    }
                }
                SyntaxKind::GENERIC_MAP_ASPECT => {
                    let list = child
                        .children()
                        .find(|n| n.kind() == SyntaxKind::ASSOCIATION_LIST);
                    if let Some(list) = list {
                        self.associations(&list, target.as_ref(), Interface::Generic);
                    }
                }
                SyntaxKind::PORT_MAP_ASPECT => {
                    let list = child
                        .children()
                        .find(|n| n.kind() == SyntaxKind::ASSOCIATION_LIST);
                    if let Some(list) = list {
                        self.associations(&list, target.as_ref(), Interface::Port);
                    }
                }
                _ => self.visit(&child),
            }
        }
        // Ports and generics that must be associated but have no map at all.
        if let Some(target) = &target {
            for (aspect, interface) in [
                (SyntaxKind::GENERIC_MAP_ASPECT, Interface::Generic),
                (SyntaxKind::PORT_MAP_ASPECT, Interface::Port),
            ] {
                if !node.children().any(|n| n.kind() == aspect) {
                    self.check_unassociated(node, target, interface, &HashSet::new());
                }
            }
        }
    }

    /// `use entity lib.e(arch) generic map (...) port map (...)`.
    fn binding_indication(&mut self, node: &SyntaxNode) {
        let is_entity = has_token(node, SyntaxKind::ENTITY_KW);
        for child in node.children() {
            match child.kind() {
                SyntaxKind::GENERIC_MAP_ASPECT => {
                    self.associations(&child, None, Interface::Generic)
                }
                SyntaxKind::PORT_MAP_ASPECT => self.associations(&child, None, Interface::Port),
                SyntaxKind::CALL_EXPR if is_entity => {
                    let mut parts = child.children();
                    let (Some(name), arch) = (parts.next(), parts.next()) else {
                        continue;
                    };
                    if let Lookup::Found(targets) = self.name_expr(&name, Access::Other) {
                        let entity = targets.iter().find_map(|t| match t {
                            Target::Decl(id) => self
                                .decl(*id)
                                .filter(|d| d.kind == DeclKind::Entity)
                                .map(|d| (*id, d.clone())),
                            _ => None,
                        });
                        if let (Some((id, decl)), Some(arch)) = (entity, arch) {
                            self.architecture_ref(&arch, id, &decl);
                        }
                    }
                }
                _ => {
                    self.name_expr(&child, Access::Other);
                }
            }
        }
    }

    fn architecture_ref(&mut self, arch: &SyntaxNode, entity: DeclId, decl: &Decl) {
        let token = arch
            .descendants_with_tokens()
            .filter_map(|e| e.into_token())
            .find(|t| matches!(t.kind(), SyntaxKind::IDENT | SyntaxKind::EXT_IDENT));
        let Some(token) = token else { return };
        let key = identifier_key(token.text());
        let library = &self.ctx.units[entity.unit as usize].library;
        match self.ctx.architecture(library, &decl.name, &key) {
            Some(unit) => self.record(
                token.text_range(),
                smallvec![Target::Decl(DeclId { unit, index: 0 })],
                Access::Other,
            ),
            None => self.error(
                codes::UNRESOLVED_UNIT,
                token.text_range(),
                format!(
                    "no architecture `{}` of entity `{}`",
                    token.text(),
                    decl.spelling
                ),
            ),
        }
    }

    /// The interface objects of `region` that `interface` maps, in order.
    fn interface_decls(&self, region: &Region, interface: Interface) -> Vec<DeclId> {
        region
            .iter()
            .filter(|&id| {
                self.decl(id).is_some_and(|d| match interface {
                    Interface::Port => matches!(d.kind, DeclKind::Port { .. }),
                    _ => matches!(
                        d.kind,
                        DeclKind::Generic { .. }
                            | DeclKind::GenericType
                            | DeclKind::GenericSubprogram
                            | DeclKind::GenericPackage
                    ),
                })
            })
            .collect()
    }

    /// A generic or port map. With a known target, formals are checked and
    /// actuals get the access their port's mode implies.
    fn associations(
        &mut self,
        list: &SyntaxNode,
        target: Option<&(Arc<Region>, String)>,
        interface: Interface,
    ) {
        if matches!(
            list.kind(),
            SyntaxKind::GENERIC_MAP_ASPECT | SyntaxKind::PORT_MAP_ASPECT
        ) {
            if let Some(inner) = list
                .children()
                .find(|n| n.kind() == SyntaxKind::ASSOCIATION_LIST)
            {
                self.associations(&inner, target, interface);
            }
            return;
        }
        let formals = target
            .map(|(region, _)| self.interface_decls(region, interface))
            .unwrap_or_default();
        let mut associated: HashSet<DeclId> = HashSet::new();
        let mut position = 0usize;
        for element in list.children() {
            if element.kind() != SyntaxKind::ASSOCIATION_ELEMENT {
                self.visit(&element);
                continue;
            }
            let has_arrow = has_token(&element, SyntaxKind::ARROW);
            let mut formal_decl = None;
            if has_arrow {
                if let Some(formal) = element.children().next() {
                    formal_decl = self.formal(&formal, target, &formals, interface);
                }
            } else {
                formal_decl = formals.get(position).copied();
                position += 1;
            }
            if let Some(id) = formal_decl {
                associated.insert(id);
            }
            let access = match interface {
                Interface::Port => match formal_decl.and_then(|id| self.decl(id)).map(|d| d.kind) {
                    Some(DeclKind::Port { mode, .. }) => {
                        match (mode.reads_actual(), mode.writes_actual()) {
                            (true, false) => Access::Read,
                            (false, true) => Access::Write,
                            _ => Access::ReadWrite,
                        }
                    }
                    _ => Access::ReadWrite,
                },
                _ => Access::Read,
            };
            let mut past_arrow = !has_arrow;
            for child in element.children_with_tokens() {
                match child {
                    rowan::NodeOrToken::Token(t) if t.kind() == SyntaxKind::ARROW => {
                        past_arrow = true
                    }
                    rowan::NodeOrToken::Node(n) if past_arrow => {
                        if access.writes() {
                            self.with_access(access, |s| s.target_or_expr(&n, access));
                        } else {
                            self.with_access(access, |s| s.visit(&n));
                        }
                    }
                    _ => {}
                }
            }
        }
        if let Some(target) = target {
            self.check_unassociated(list, target, interface, &associated);
        }
    }

    /// An actual for an output: the object is written; anything inside a
    /// call (a conversion function's argument, an index) is read.
    fn target_or_expr(&mut self, node: &SyntaxNode, access: Access) {
        match node.kind() {
            SyntaxKind::NAME | SyntaxKind::SELECTED_NAME => {
                self.name_expr(node, access);
            }
            SyntaxKind::CALL_EXPR => {
                // `s(3)` writes s; `to_slv(s)` would read it, but we cannot
                // tell the two apart here.
                self.name_expr(node, access);
            }
            _ => self.visit(node),
        }
    }

    fn formal(
        &mut self,
        formal: &SyntaxNode,
        target: Option<&(Arc<Region>, String)>,
        formals: &[DeclId],
        interface: Interface,
    ) -> Option<DeclId> {
        // The formal designator: `p`, `p(3)`, `p.field`, or `conv(p)`.
        let base = match formal.kind() {
            SyntaxKind::NAME => simple_name_token(formal),
            SyntaxKind::SELECTED_NAME | SyntaxKind::CALL_EXPR => {
                let mut node = formal.clone();
                loop {
                    let Some(first) = node.children().next() else {
                        break None;
                    };
                    if first.kind() == SyntaxKind::NAME {
                        break simple_name_token(&first);
                    }
                    node = first;
                }
            }
            _ => None,
        };
        let (region, what) = target?;
        let token = base?;
        let key = identifier_key(token.text());
        let find = |key: &str| {
            formals
                .iter()
                .copied()
                .find(|&id| self.decl(id).is_some_and(|d| d.name == key))
        };
        let mut found = find(&key);
        if found.is_none() && formal.kind() == SyntaxKind::CALL_EXPR {
            // A conversion function: `to_integer(p) => s`.
            let inner = formal
                .children()
                .find(|n| n.kind() == SyntaxKind::ASSOCIATION_LIST)
                .and_then(|l| l.descendants().find(|n| n.kind() == SyntaxKind::NAME))
                .and_then(|n| simple_name_token(&n));
            if let Some(inner) = inner
                && let Some(id) = find(&identifier_key(inner.text()))
            {
                self.record(token.text_range(), Targets::new(), Access::Other);
                self.record(
                    inner.text_range(),
                    smallvec![Target::Decl(id)],
                    Access::Other,
                );
                return Some(id);
            }
            // Not a conversion we can see through: leave the formal alone.
            return None;
        }
        match found.take() {
            Some(id) => {
                self.record(
                    token.text_range(),
                    smallvec![Target::Decl(id)],
                    Access::Other,
                );
                if formal.kind() == SyntaxKind::CALL_EXPR {
                    for list in formal.children().skip(1) {
                        self.with_access(Access::Read, |s| s.visit(&list));
                    }
                }
                Some(id)
            }
            None => {
                if !region.opaque {
                    let kind = if interface == Interface::Port {
                        "port"
                    } else {
                        "generic"
                    };
                    self.error(
                        codes::UNRESOLVED_NAME,
                        token.text_range(),
                        format!("{what} has no {kind} `{}`", token.text()),
                    );
                }
                None
            }
        }
    }

    fn check_unassociated(
        &mut self,
        at: &SyntaxNode,
        target: &(Arc<Region>, String),
        interface: Interface,
        associated: &HashSet<DeclId>,
    ) {
        if target.0.opaque {
            return;
        }
        let missing: Vec<String> = self
            .interface_decls(&target.0, interface)
            .into_iter()
            .filter(|id| !associated.contains(id))
            .filter_map(|id| self.decl(id))
            .filter(|d| match d.kind {
                DeclKind::Port {
                    mode: Mode::In,
                    has_default: false,
                } => true,
                DeclKind::Generic { has_default } => !has_default,
                _ => false,
            })
            .map(|d| format!("`{}`", d.spelling))
            .collect();
        if missing.is_empty() {
            return;
        }
        let kind = match (interface, missing.len()) {
            (Interface::Port, 1) => "port",
            (Interface::Port, _) => "ports",
            (_, 1) => "generic",
            _ => "generics",
        };
        let range = if at.kind() == SyntaxKind::ASSOCIATION_LIST {
            at.text_range()
        } else {
            at.children()
                .find(|n| n.kind() == SyntaxKind::LABEL)
                .map_or(at.text_range(), |n| n.text_range())
        };
        self.error(
            codes::UNASSOCIATED_PORT,
            range,
            format!(
                "{kind} {} of {} must be associated",
                missing.join(", "),
                target.1
            ),
        );
    }

    /// Missing and unneeded entries of a process's sensitivity list.
    fn check_sensitivity(
        &mut self,
        process: &SyntaxNode,
        list: &SyntaxNode,
        sens_refs: std::ops::Range<usize>,
        refs_start: usize,
    ) {
        if self.quiet > 0 {
            return;
        }
        let signal_of = |r: &Reference, me: &Self| -> Option<DeclId> {
            r.targets.iter().find_map(|t| match t {
                Target::Decl(id) => me.decl(*id).filter(|d| d.kind.is_signal()).map(|_| *id),
                _ => None,
            })
        };
        let mut listed: HashMap<DeclId, TextRange> = HashMap::new();
        for r in &self.refs[sens_refs.clone()] {
            if let Some(id) = signal_of(r, self) {
                listed.insert(id, r.range);
            }
        }
        let body: Vec<&Reference> = self.refs[refs_start..]
            .iter()
            .enumerate()
            .filter(|(i, _)| !sens_refs.contains(&(refs_start + i)))
            .map(|(_, r)| r)
            .collect();
        let required = required_ranges(process);
        let mut missing: Vec<(DeclId, String)> = Vec::new();
        let mut read: HashSet<DeclId> = HashSet::new();
        for r in &body {
            if !r.access.reads() {
                continue;
            }
            let Some(id) = signal_of(r, self) else {
                continue;
            };
            read.insert(id);
            let needed = match &required {
                Some(ranges) => ranges.iter().any(|range| range.contains_range(r.range)),
                None => false,
            };
            if needed && !listed.contains_key(&id) && !missing.iter().any(|(m, _)| *m == id) {
                let name = self
                    .decl(id)
                    .map(|d| d.spelling.clone())
                    .unwrap_or_default();
                missing.push((id, name));
            }
        }
        if !missing.is_empty() {
            let names: Vec<String> = missing.iter().map(|(_, n)| n.clone()).collect();
            let quoted: Vec<String> = names.iter().map(|n| format!("`{n}`")).collect();
            let close = list
                .children_with_tokens()
                .filter_map(|e| e.into_token())
                .find(|t| t.kind() == SyntaxKind::RPAREN);
            let fix = close.map(|close| Fix {
                title: format!("Add {} to the sensitivity list", quoted.join(", ")),
                edits: vec![Edit {
                    range: TextRange::empty(close.text_range().start()),
                    text: format!(", {}", names.join(", ")),
                }],
                safe: false,
            });
            self.diags.push(Diagnostic {
                code: codes::SENSITIVITY_LIST,
                file: self.file,
                range: list.text_range(),
                message: format!(
                    "the process reads {} but its sensitivity list does not name {}",
                    quoted.join(", "),
                    if missing.len() == 1 { "it" } else { "them" }
                ),
                fix,
            });
        }
        let mut unneeded: Vec<(DeclId, TextRange)> = listed
            .into_iter()
            .filter(|(id, _)| !read.contains(id))
            .collect();
        unneeded.sort_by_key(|(_, r)| r.start());
        for (id, range) in unneeded {
            let name = self
                .decl(id)
                .map(|d| d.spelling.clone())
                .unwrap_or_default();
            // A process without its list would never wait: keep the last.
            let fix = crate::diagnostic::list_item_removal(
                &self.ctx.files[self.file as usize].text,
                range,
            )
            .map(|removed| Fix {
                title: format!("Remove `{name}` from the sensitivity list"),
                edits: vec![Edit {
                    range: removed,
                    text: String::new(),
                }],
                safe: false,
            });
            self.diags.push(Diagnostic {
                code: codes::UNNEEDED_SENSITIVITY,
                file: self.file,
                range,
                message: format!(
                    "`{name}` is in the sensitivity list but the process never reads it"
                ),
                fix,
            });
        }
    }

    // ---- Dispatch ---------------------------------------------------------

    fn visit(&mut self, node: &SyntaxNode) {
        use SyntaxKind::*;
        match node.kind() {
            NAME => {
                self.simple_name(node, self.access);
            }
            SELECTED_NAME | CALL_EXPR => {
                self.name_expr(node, self.access);
            }
            ATTRIBUTE_NAME => self.attribute_name(node),
            QUALIFIED_EXPR => {
                for (i, child) in node.children().enumerate() {
                    if i == 0 {
                        self.name_expr(&child, Access::Other);
                    } else {
                        self.with_access(Access::Read, |s| s.visit(&child));
                    }
                }
            }
            BINARY_EXPR | UNARY_EXPR => {
                // A long chain (`a & b & ...`) nests to the left: walk its
                // left spine in a loop, so 16k operands don't overflow the
                // stack.
                let mut spine = vec![node.clone()];
                while let Some(left) = spine.last().and_then(|n| n.children().next())
                    && left.kind() == BINARY_EXPR
                {
                    spine.push(left);
                }
                for n in &spine {
                    self.operator_ref(n);
                }
                let (innermost, outer) = spine.split_last().expect("not empty");
                self.visit_children(innermost);
                for n in outer.iter().rev() {
                    for child in n.children().skip(1) {
                        self.visit(&child);
                    }
                }
            }
            AGGREGATE => {
                for element in node.children() {
                    for child in element.children() {
                        if child.kind() == CHOICES {
                            self.soft += 1;
                            self.visit(&child);
                            self.soft -= 1;
                        } else {
                            self.visit(&child);
                        }
                    }
                }
            }
            EXTERNAL_NAME => {
                for child in node.children() {
                    if child.kind() == SUBTYPE_INDICATION {
                        self.with_access(Access::Other, |s| s.visit(&child));
                    }
                }
            }
            SUBTYPE_INDICATION | SIGNATURE => {
                self.in_subtype += 1;
                self.with_access(Access::Other, |s| s.visit_children(node));
                self.in_subtype -= 1;
            }
            ERROR => {
                self.quiet += 1;
                self.visit_children(node);
                self.quiet -= 1;
            }
            PSL_DIRECTIVE => {}

            LIBRARY_CLAUSE => self.library_clause(node),
            USE_CLAUSE => self.use_clause(node),
            CONTEXT_REFERENCE => self.context_reference(node),

            GENERIC_CLAUSE => self.with_interface(Interface::Generic, |s| s.visit_children(node)),
            PORT_CLAUSE => self.with_interface(Interface::Port, |s| s.visit_children(node)),
            INTERFACE_OBJECT_DECL => self.interface_object(node),
            INTERFACE_TYPE_DECL => {
                if let Some(name) = designator(node) {
                    self.declare(&name, DeclKind::GenericType, node);
                }
            }
            INTERFACE_SUBPROGRAM_DECL => {
                for spec in node.children() {
                    if matches!(spec.kind(), FUNCTION_SPEC | PROCEDURE_SPEC) {
                        let saved = self.interface;
                        self.subprogram_spec(&spec, DeclKind::GenericSubprogram, node);
                        self.frames.pop();
                        self.parents.pop();
                        self.interface = saved;
                    } else {
                        self.visit(&spec);
                    }
                }
            }
            INTERFACE_PACKAGE_DECL => {
                let Some(name) = designator(node) else { return };
                let mut members = None;
                for child in node.children() {
                    match child.kind() {
                        SELECTED_NAME | NAME if members.is_none() => {
                            members = Some(match self.name_expr(&child, Access::Other) {
                                Lookup::Found(t) => self.members_of(&t),
                                _ => None,
                            });
                        }
                        GENERIC_MAP_ASPECT => {
                            let saved = self.interface;
                            self.associations(&child, None, Interface::Generic);
                            self.interface = saved;
                        }
                        _ => {}
                    }
                }
                let id = self.declare(&name, DeclKind::GenericPackage, node);
                self.decls[id.index as usize].members = Some(
                    members
                        .flatten()
                        .unwrap_or_else(|| Arc::new(Region::opaque())),
                );
            }
            GENERIC_MAP_ASPECT => self.associations(node, None, Interface::Generic),
            PORT_MAP_ASPECT => self.associations(node, None, Interface::Port),

            SIGNAL_DECL => self.object_decl(node, DeclKind::Signal),
            CONSTANT_DECL => self.object_decl(node, DeclKind::Constant),
            VARIABLE_DECL => self.object_decl(node, DeclKind::Variable),
            FILE_DECL => self.object_decl(node, DeclKind::File),
            TYPE_DECL => self.type_decl(node),
            SUBTYPE_DECL => {
                self.with_access(Access::Other, |s| s.visit_children(node));
                if let Some(name) = designator(node) {
                    let id = self.declare(&name, DeclKind::Subtype, node);
                    let indication = node.children().find(|n| n.kind() == SUBTYPE_INDICATION);
                    self.decls[id.index as usize].subtype =
                        indication.as_ref().and_then(|n| self.subtype_mark(n));
                    self.decls[id.index as usize].width =
                        indication.as_ref().and_then(|n| self.indication_width(n));
                }
            }
            ALIAS_DECL => {
                // An alias may stand for reads and writes of its object.
                self.with_access(Access::ReadWrite, |s| s.visit_children(node));
                if let Some(name) = designator(node) {
                    let id = self.declare(&name, DeclKind::Alias, node);
                    self.decls[id.index as usize].subtype = self.alias_subtype(node);
                }
            }
            ATTRIBUTE_DECL => {
                self.with_access(Access::Other, |s| s.visit_children(node));
                if let Some(name) = designator(node) {
                    self.declare(&name, DeclKind::Attribute, node);
                }
            }
            ATTRIBUTE_SPEC => {
                if let Some(name) = designator(node)
                    && let Lookup::Found(targets) = self.lookup(&designator_key(&name))
                {
                    self.record(name.text_range(), targets, Access::Other);
                }
                for child in node.children() {
                    if child.kind() == ENTITY_SPEC {
                        // `attribute a of s, t : signal is ...`: link, never report.
                        for token in child.children_with_tokens().filter_map(|e| e.into_token()) {
                            if matches!(token.kind(), IDENT | EXT_IDENT | STRING_LITERAL)
                                && let Lookup::Found(targets) = self.lookup(&designator_key(&token))
                            {
                                self.record(token.text_range(), targets, Access::Other);
                            }
                        }
                    } else {
                        self.with_access(Access::Read, |s| s.visit(&child));
                    }
                }
            }
            COMPONENT_DECL => self.component(node),
            SUBPROGRAM_DECL => {
                for spec in node.children() {
                    if matches!(spec.kind(), FUNCTION_SPEC | PROCEDURE_SPEC) {
                        let kind = Self::subprogram_kind(&spec);
                        self.subprogram_spec(&spec, kind, node);
                        self.frames.pop();
                        self.parents.pop();
                    }
                }
            }
            SUBPROGRAM_BODY => {
                let Some(spec) = node
                    .children()
                    .find(|n| matches!(n.kind(), FUNCTION_SPEC | PROCEDURE_SPEC))
                else {
                    return self.visit_children(node);
                };
                let kind = Self::subprogram_kind(&spec);
                let id = self.subprogram_spec(&spec, kind, node);
                for child in node.children() {
                    if child != spec {
                        self.with_access(Access::Read, |s| s.visit(&child));
                    }
                }
                self.frames.pop();
                self.parents.pop();
                if let (Some(id), Some(name)) = (id, designator(&spec)) {
                    // The body completes the declaration with the same
                    // profile, in a package or earlier in the region.
                    // References stay in source order: this one goes before
                    // the body's.
                    if let Some(declaration) = self.subprogram_declaration(id) {
                        let range = name.text_range();
                        let at = self
                            .refs
                            .partition_point(|r| r.range.start() < range.start());
                        self.refs.insert(
                            at,
                            Reference {
                                range,
                                targets: smallvec![Target::Decl(declaration)],
                                access: Access::Other,
                            },
                        );
                    }
                    self.end_label_refs(node, &name, id);
                }
            }
            SUBPROGRAM_INSTANTIATION_DECL => {
                self.with_access(Access::Other, |s| {
                    for child in node.children() {
                        if child.kind() == GENERIC_MAP_ASPECT {
                            s.associations(&child, None, Interface::Generic);
                        } else {
                            s.visit(&child);
                        }
                    }
                });
                if let Some(name) = designator(node) {
                    let kind = if has_token(node, PROCEDURE_KW) {
                        DeclKind::Procedure
                    } else {
                        DeclKind::Function
                    };
                    self.declare(&name, kind, node);
                }
            }
            PACKAGE_DECL | PACKAGE_BODY | PACKAGE_INSTANTIATION_DECL => self.nested_package(node),
            GROUP_TEMPLATE_DECL | GROUP_DECL => {
                if let Some(name) = designator(node) {
                    self.declare(&name, DeclKind::Group, node);
                }
            }
            MODE_VIEW_DECL => {
                if let Some(name) = designator(node) {
                    self.declare(&name, DeclKind::View, node);
                }
                for child in node.children() {
                    if child.kind() == SUBTYPE_INDICATION || child.kind() == NAME {
                        self.with_access(Access::Other, |s| s.visit(&child));
                    }
                }
            }
            CONFIGURATION_SPEC => {
                // The binding's maps may name the component's generics and ports.
                let mut frame = Frame::default();
                for child in node.children() {
                    match child.kind() {
                        BINDING_INDICATION => {
                            self.frames.push(std::mem::take(&mut frame));
                            self.binding_indication(&child);
                            self.frames.pop();
                        }
                        NAME | SELECTED_NAME => {
                            if let Lookup::Found(targets) = self.name_expr(&child, Access::Other)
                                && let Some(members) = self.members_of(&targets)
                            {
                                frame.inherited.push(members);
                            }
                        }
                        _ => {}
                    }
                }
            }
            BINDING_INDICATION
            | BLOCK_CONFIGURATION
            | COMPONENT_CONFIGURATION
            | DISCONNECTION_SPEC => {}

            PROCESS_STMT => self.process(node),
            BLOCK_STMT => self.scoped_statement(node),
            FOR_GENERATE_STMT => self.for_generate(node),
            IF_GENERATE_STMT | CASE_GENERATE_STMT => self.conditional_generate(node),
            GENERATE_BODY => self.generate_body(node),
            LOOP_STMT => self.loop_stmt(node),
            SEQUENTIAL_BLOCK_STMT => self.scoped_statement(node),
            COMPONENT_INSTANTIATION_STMT => self.instantiation(node),
            SIGNAL_ASSIGN_STMT | SELECTED_ASSIGN_STMT => self.assignment(node, LTE),
            VARIABLE_ASSIGN_STMT => self.assignment(node, COLON_EQ),
            PROCEDURE_CALL_STMT => {
                self.label(node);
                self.with_access(Access::ReadWrite, |s| {
                    s.visit_children_except(node, |n| n.kind() == LABEL)
                });
            }
            EXIT_STMT | NEXT_STMT => {
                self.label(node);
                self.exit_or_next(node);
            }
            IF_STMT | CASE_STMT | WAIT_STMT | ASSERT_STMT | REPORT_STMT | RETURN_STMT
            | NULL_STMT => {
                let label = self.label(node);
                self.with_access(Access::Read, |s| {
                    s.visit_children_except(node, |n| n.kind() == LABEL)
                });
                if let Some((token, id)) = label {
                    self.end_label_refs(node, &token, id);
                }
            }
            LABEL => {}
            _ => self.visit_children(node),
        }
    }
}

/// The parts of a process whose signal reads must be in its sensitivity
/// list, or `None` when the process has a shape the check does not
/// understand (it is then skipped).
///
/// A combinational process needs every signal it reads. A clocked process,
/// a single `if` whose branch tests a clock edge, needs the signals read in
/// the conditions up to and including the edge test, and in the branches
/// before it (an asynchronous reset).
fn required_ranges(process: &SyntaxNode) -> Option<Vec<TextRange>> {
    let begin = process
        .children_with_tokens()
        .position(|e| e.kind() == SyntaxKind::BEGIN_KW)?;
    let statements: Vec<SyntaxNode> = process
        .children_with_tokens()
        .skip(begin + 1)
        .filter_map(|e| e.into_node())
        .collect();
    let has_edge = |node: &SyntaxNode| node.descendants().any(|n| is_clock_edge(&n));
    let clocked = statements.iter().any(has_edge);
    if !clocked {
        return Some(statements.iter().map(SyntaxNode::text_range).collect());
    }
    let [single] = statements.as_slice() else {
        return None;
    };
    if single.kind() != SyntaxKind::IF_STMT {
        return None;
    }
    // Walk the branches: conditions follow `if`/`elsif`, bodies follow `then`/`else`.
    let mut ranges = Vec::new();
    let mut expect_condition = false;
    let mut in_body = false;
    for element in single.children_with_tokens() {
        match element {
            rowan::NodeOrToken::Token(t) => match t.kind() {
                SyntaxKind::IF_KW | SyntaxKind::ELSIF_KW => {
                    expect_condition = true;
                    in_body = false;
                }
                SyntaxKind::THEN_KW | SyntaxKind::ELSE_KW => in_body = true,
                SyntaxKind::END_KW => break,
                _ => {}
            },
            rowan::NodeOrToken::Node(n) => {
                if expect_condition {
                    expect_condition = false;
                    let edges: Vec<TextRange> = n
                        .descendants()
                        .filter(is_clock_edge)
                        .map(|e| e.text_range())
                        .collect();
                    if !edges.is_empty() {
                        // Only the clock itself: other signals tested with
                        // the edge are sampled on it.
                        ranges.extend(edges);
                        return Some(ranges);
                    }
                    ranges.push(n.text_range());
                } else if in_body {
                    ranges.push(n.text_range());
                }
            }
        }
    }
    None
}

/// `rising_edge(clk)`, `falling_edge(clk)` or `clk'event`.
pub(crate) fn is_clock_edge(node: &SyntaxNode) -> bool {
    match node.kind() {
        SyntaxKind::CALL_EXPR => node
            .children()
            .next()
            .and_then(|n| simple_name_token(&n))
            .is_some_and(|t| {
                matches!(
                    t.text().to_ascii_lowercase().as_str(),
                    "rising_edge" | "falling_edge"
                )
            }),
        SyntaxKind::ATTRIBUTE_NAME => node
            .children_with_tokens()
            .filter_map(|e| e.into_token())
            .skip_while(|t| t.kind() != SyntaxKind::TICK)
            .nth(1)
            .is_some_and(|t| t.text().eq_ignore_ascii_case("event")),
        _ => false,
    }
}

/// The token a type mark or name is resolved at: the identifier of a simple
/// name, the suffix of a selected name, the prefix of a constrained mark.
pub(crate) fn mark_token(mark: &SyntaxNode) -> Option<SyntaxToken> {
    match mark.kind() {
        SyntaxKind::NAME => mark
            .children_with_tokens()
            .filter_map(|e| e.into_token())
            .find(|t| is_designator(t.kind())),
        SyntaxKind::SELECTED_NAME => mark
            .children_with_tokens()
            .filter_map(|e| e.into_token())
            .filter(|t| is_designator(t.kind()))
            .last(),
        SyntaxKind::CALL_EXPR => mark_token(&mark.children().next()?),
        _ => None,
    }
}

/// The operator of a binary or unary expression.
pub(crate) fn operator_token(node: &SyntaxNode) -> Option<SyntaxToken> {
    node.children_with_tokens()
        .filter_map(|e| e.into_token())
        .find(|t| !t.kind().is_trivia())
}

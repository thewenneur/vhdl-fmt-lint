//! Name resolution for one design unit: walks the unit's syntax tree in
//! source order, declaring names into nested regions and resolving every
//! name it meets against them, the unit's use clauses, and the library
//! units of the project.
//!
//! Resolution is by name only, without overload resolution or types. A name
//! that cannot be checked (selected through a record, from a library that
//! is not part of the project, ...) resolves to "unknown" and is never
//! reported: false errors are worse than missed ones.

mod clauses;
mod declarations;
mod instances;
mod names;
mod sensitivity;
mod statements;
mod types;
mod visit;

use std::sync::Arc;

use rustc_hash::FxHashSet;
use smallvec::smallvec;
use vfl_syntax::green_walk::ChildTokens;
use vfl_syntax::{SyntaxKind, SyntaxNode, SyntaxToken, TextRange, identifier_key};

use crate::analysis::{Ctx, UnitResult};
use crate::decl::{
    Access, Decl, DeclId, DeclKind, FileId, Reference, Region, Target, Targets, UnitId,
};
use crate::diagnostic::{Diagnostic, codes};
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
    used_imports: FxHashSet<ImportId>,
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
            used_imports: FxHashSet::default(),
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
        // A vendor stub declares the primitives most often used: names it
        // lacks are not reported.
        let region = if self.ctx.is_stub(self.file) {
            let mut region = Arc::unwrap_or_clone(region);
            region.opaque = true;
            Arc::new(region)
        } else {
            region
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
            refs: {
                // In source order, for binary searches by position:
                // operators are recorded before their operands, and
                // implicit declarations last. Almost sorted already, so
                // this costs about one pass.
                let mut refs = self.refs;
                refs.sort_by_key(|r| r.range.start());
                refs
            },
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

    /// The innermost frame. The root frame is never popped (see
    /// [`Self::pop_frame`]), so there always is one.
    fn top_mut(&mut self) -> &mut Frame {
        if self.frames.is_empty() {
            debug_assert!(false, "the root frame is gone");
            self.frames.push(Frame::default());
        }
        let last = self.frames.len() - 1;
        &mut self.frames[last]
    }

    /// Leaves the innermost frame. An unbalanced pop, a bug, keeps the root
    /// frame and degrades name resolution instead of panicking analysis.
    fn pop_frame(&mut self) -> Frame {
        debug_assert!(self.frames.len() > 1, "popped the root frame");
        if self.frames.len() > 1 {
            self.frames.pop().unwrap_or_default()
        } else {
            Frame::default()
        }
    }

    /// Pops the unit's own region. Its use clauses stay visible to the
    /// unit's secondary units, so they move to the enclosing frame.
    fn pop_unit_frame(&mut self) -> Arc<Region> {
        let frame = self.pop_frame();
        let outer = self.top_mut();
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
        SyntaxKind::NAME => mark.child_token(is_designator),
        SyntaxKind::SELECTED_NAME => mark.last_child_token(is_designator),
        SyntaxKind::CALL_EXPR => mark_token(&mark.children().next()?),
        _ => None,
    }
}

/// The operator of a binary or unary expression.
pub(crate) fn operator_token(node: &SyntaxNode) -> Option<SyntaxToken> {
    node.child_token(|k| !k.is_trivia())
}

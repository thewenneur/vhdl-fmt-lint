//! Declarations: objects, interfaces, types, subprograms, components and
//! nested packages.

use std::sync::Arc;

use smallvec::smallvec;
use vfl_syntax::{SyntaxKind, SyntaxNode, SyntaxToken};

use super::{Frame, Import, ImportId, ImportKind, Interface, Lookup, Resolver};
use crate::decl::{
    Access, Decl, DeclId, DeclKind, Element, Mode, Reference, Region, Target, TypeDef,
};
use crate::diagnostic::codes;
use crate::syntax::*;

impl<'a> Resolver<'a> {
    // ---- Declarations -----------------------------------------------------

    /// Makes every failed lookup in the current scope unknown.
    pub(super) fn push_opaque(&mut self) {
        let id = self.new_import_id();
        self.top_mut().imports.push(Import {
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

    pub(super) fn push_decl(
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
    pub(super) fn declare(
        &mut self,
        name: &SyntaxToken,
        kind: DeclKind,
        node: &SyntaxNode,
    ) -> DeclId {
        let id = self.push_decl(Some(name), kind, node);
        let key = self.decls[id.index as usize].name.clone();
        let clash = self.frames.last().and_then(|frame| {
            frame.locals.get(&key).iter().copied().find(|&other| {
                let other_kind = self.decl(other).map(|d| d.kind);
                !kind.is_overloadable()
                    && !other_kind.is_some_and(DeclKind::is_overloadable)
                    && !matches!(
                        (other_kind, kind),
                        (Some(DeclKind::IncompleteType), DeclKind::Type)
                            | (Some(DeclKind::IncompleteType), DeclKind::IncompleteType)
                    )
            })
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
        self.top_mut().locals.insert(&key, id);
        id
    }

    pub(super) fn decl(&self, id: DeclId) -> Option<&Decl> {
        if id.unit == self.unit {
            self.decls.get(id.index as usize)
        } else {
            self.ctx.decl(id)
        }
    }

    /// The declaration that subprogram body `body` completes: one of the
    /// same kind, name and profile, declared earlier in the region or in
    /// the package or protected type the region is the body of.
    pub(super) fn subprogram_declaration(&self, body: DeclId) -> Option<DeclId> {
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
    pub(super) fn end_label_refs(&mut self, node: &SyntaxNode, name: &SyntaxToken, id: DeclId) {
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

    pub(super) fn object_decl(&mut self, node: &SyntaxNode, kind: DeclKind) {
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

    pub(super) fn interface_object(&mut self, node: &SyntaxNode) {
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

    pub(super) fn with_interface(&mut self, interface: Interface, f: impl FnOnce(&mut Self)) {
        let saved = std::mem::replace(&mut self.interface, interface);
        f(self);
        self.interface = saved;
    }

    pub(super) fn with_access(&mut self, access: Access, f: impl FnOnce(&mut Self)) {
        let saved = std::mem::replace(&mut self.access, access);
        f(self);
        self.access = saved;
    }

    pub(super) fn type_decl(&mut self, node: &SyntaxNode) {
        let Some(name) = designator(node) else {
            return self.visit_children(node);
        };
        let def = node
            .children()
            .find(|n| !matches!(n.kind(), SyntaxKind::LABEL));
        match def {
            None => {
                self.declare(&name, DeclKind::IncompleteType, node);
            }
            Some(def) if def.kind() == SyntaxKind::PROTECTED_TYPE_BODY => {
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
                self.pop_frame();
                self.parents.pop();
                self.end_label_refs(&def, &name, id);
            }
            Some(def) if def.kind() == SyntaxKind::PROTECTED_TYPE_DEF => {
                let id = self.declare(&name, DeclKind::Type, node);
                self.decls[id.index as usize].type_def = Some(Arc::new(TypeDef::Protected));
                self.parents.push(id);
                self.frames.push(Frame::default());
                self.visit_children(&def);
                let members = Arc::new(self.pop_frame().locals);
                self.parents.pop();
                self.decls[id.index as usize].members = Some(members);
                self.end_label_refs(&def, &name, id);
            }
            Some(def) => {
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
    pub(super) fn subprogram_spec(
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

    pub(super) fn subprogram_kind(spec: &SyntaxNode) -> DeclKind {
        if spec.kind() == SyntaxKind::PROCEDURE_SPEC {
            DeclKind::Procedure
        } else {
            DeclKind::Function
        }
    }

    pub(super) fn component(&mut self, node: &SyntaxNode) {
        let Some(name) = designator(node) else {
            return self.visit_children(node);
        };
        let id = self.declare(&name, DeclKind::Component, node);
        self.parents.push(id);
        self.frames.push(Frame::default());
        self.visit_children(node);
        let mut members = self.pop_frame().locals;
        // A vendor stub declares the ports and generics most often used:
        // others are not reported, nor ones left unassociated.
        members.opaque = self.ctx.is_stub(self.file);
        self.parents.pop();
        self.decls[id.index as usize].members = Some(Arc::new(members));
        self.end_label_refs(node, &name, id);
    }

    /// A nested package, package body or package instantiation.
    pub(super) fn nested_package(&mut self, node: &SyntaxNode) {
        let Some(name) = designator(node) else {
            return self.visit_children(node);
        };
        match node.kind() {
            SyntaxKind::PACKAGE_DECL => {
                let id = self.declare(&name, DeclKind::Package, node);
                self.parents.push(id);
                self.frames.push(Frame::default());
                self.visit_children(node);
                let members = Arc::new(self.pop_frame().locals);
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
                self.pop_frame();
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
}

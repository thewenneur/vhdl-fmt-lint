//! Instantiations, binding indications and association lists.

use std::sync::Arc;

use rustc_hash::FxHashSet;
use smallvec::smallvec;
use vfl_syntax::{SyntaxKind, SyntaxNode, identifier_key};

use super::{Interface, Lookup, Resolver};
use crate::decl::{Access, Decl, DeclId, DeclKind, Mode, Region, Target, Targets};
use crate::diagnostic::codes;
use crate::syntax::*;

impl<'a> Resolver<'a> {
    pub(super) fn instantiation(&mut self, node: &SyntaxNode) {
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
                    self.check_unassociated(node, target, interface, &FxHashSet::default());
                }
            }
        }
    }

    /// `use entity lib.e(arch) generic map (...) port map (...)`.
    pub(super) fn binding_indication(&mut self, node: &SyntaxNode) {
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
    pub(super) fn associations(
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
        let mut associated: FxHashSet<DeclId> = FxHashSet::default();
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
        associated: &FxHashSet<DeclId>,
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
}

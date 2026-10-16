//! Library, use and context clauses, and looking names up through them.

use smallvec::smallvec;
use vfl_syntax::green_walk::ChildTokens;
use vfl_syntax::{SyntaxKind, SyntaxNode, identifier_key};

use super::{Import, ImportId, ImportKind, Lookup, Resolver, UseItem};
use crate::decl::{Access, DeclId, DeclKind, Reference, Target, Targets};
use crate::diagnostic::{Diagnostic, codes};
use crate::syntax::*;

impl<'a> Resolver<'a> {
    // ---- Clauses ----------------------------------------------------------

    pub(super) fn library_clause(&mut self, node: &SyntaxNode) {
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
                self.top_mut().libraries.push(key);
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

    pub(super) fn use_clause(&mut self, node: &SyntaxNode) {
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
            if kinds.iter().any(|k| matches!(k, ImportKind::Opaque))
                && let Some(item) = self.use_items.last_mut()
            {
                item.resolved = false;
            }
            for kind in kinds {
                self.top_mut().imports.push(Import { id, kind });
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
                    .last_child_token(is_designator)
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

    pub(super) fn context_reference(&mut self, node: &SyntaxNode) {
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
                    let frame = self.top_mut();
                    frame.imports.extend(result.imports.iter().cloned());
                    frame.libraries.extend(result.libraries.iter().cloned());
                }
                None => self.push_opaque(),
            }
        }
    }

    // ---- Lookup -----------------------------------------------------------

    pub(super) fn lookup(&mut self, name: &str) -> Lookup {
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

    pub(super) fn is_overloadable(&self, id: DeclId) -> bool {
        self.decl(id).is_some_and(|d| d.kind.is_overloadable())
    }
}

//! Names: simple, selected and attribute names, and call arguments.

use smallvec::smallvec;
use vfl_syntax::green_walk::ChildTokens;
use vfl_syntax::{SyntaxKind, SyntaxNode};

use super::{Lookup, Resolver};
use crate::decl::{Access, DeclId, DeclKind, Target, Targets};
use crate::diagnostic::codes;
use crate::syntax::*;

impl<'a> Resolver<'a> {
    // ---- Names ------------------------------------------------------------

    /// Resolves a name expression and records its references.
    pub(super) fn name_expr(&mut self, node: &SyntaxNode, access: Access) -> Lookup {
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

    pub(super) fn simple_name(&mut self, node: &SyntaxNode, access: Access) -> Lookup {
        let Some(token) = node.child_token(is_designator) else {
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
        let suffix = node.child_token(|k| is_designator(k) || k == SyntaxKind::ALL_KW);
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

    pub(super) fn attribute_name(&mut self, node: &SyntaxNode) {
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
}

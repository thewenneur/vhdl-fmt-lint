//! Types: type marks, type definitions and static values.

use std::sync::Arc;

use vfl_syntax::{SyntaxKind, SyntaxNode, SyntaxToken, TextRange};

use super::{Resolver, mark_token};
use crate::decl::{Access, DeclId, DeclKind, Reference, Region, Target, Targets, TypeDef};
use crate::diagnostic::Diagnostic;
use crate::syntax::*;

impl<'a> Resolver<'a> {
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
    pub(super) fn type_mark(&self, mark: &SyntaxNode) -> Option<DeclId> {
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
    pub(super) fn alias_subtype(&self, node: &SyntaxNode) -> Option<DeclId> {
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
    pub(super) fn complete_incomplete_type(&mut self, name: &SyntaxToken, full: DeclId) {
        let key = designator_key(name);
        let incomplete = self.frames.last().and_then(|frame| {
            frame.locals.get(&key).iter().copied().find(|&d| {
                d != full
                    && d.unit == self.unit
                    && self.decls[d.index as usize].kind == DeclKind::IncompleteType
            })
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
    pub(super) fn type_definition(&self, def: &SyntaxNode) -> Option<TypeDef> {
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
    pub(super) fn named_value(&self, name: &SyntaxNode) -> Option<i64> {
        self.decl(self.resolved_decl(name)?)?.value
    }

    /// The value of a static integer expression, just resolved.
    pub(super) fn static_value(&self, expr: &SyntaxNode) -> Option<i64> {
        crate::rtl::static_int(expr, &|n| self.named_value(n))
    }

    /// The width of a subtype indication: `std_logic_vector(7 downto 0)`,
    /// or a subtype mark of known width.
    pub(super) fn indication_width(&self, indication: &SyntaxNode) -> Option<u32> {
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
    pub(super) fn members_of(&self, targets: &[Target]) -> Option<Arc<Region>> {
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

    pub(super) fn error(&mut self, code: &'static str, range: TextRange, message: String) {
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

    pub(super) fn record(&mut self, range: TextRange, targets: Targets, access: Access) {
        self.refs.push(Reference {
            range,
            targets,
            access,
        });
    }
}

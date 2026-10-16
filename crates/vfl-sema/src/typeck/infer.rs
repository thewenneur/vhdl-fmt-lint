//! Inference: the types an expression may have, bottom-up.

use vfl_syntax::{SyntaxKind, SyntaxNode, SyntaxToken};

use super::{
    Callee, Checker, MAX_DEPTH, Ty, TyS, Val, attribute_designator, literal, selected_suffix,
    single_type,
};
use crate::decl::{DeclId, DeclKind, ElementRef, TypeDef};
use crate::resolver::mark_token;

impl<'a> Checker<'a> {
    // ---- Inference ----------------------------------------------------------

    /// The values an expression may have, bottom-up.
    pub(super) fn infer(&mut self, node: &SyntaxNode) -> Val {
        let key = (node.text_range(), node.kind());
        if let Some(v) = self.cache.get(&key) {
            return v.clone();
        }
        if self.depth >= MAX_DEPTH {
            return Val::Unknown;
        }
        self.depth += 1;
        let val = self.infer_uncached(node);
        self.depth -= 1;
        self.cache.insert(key, val.clone());
        val
    }

    fn infer_uncached(&mut self, node: &SyntaxNode) -> Val {
        use SyntaxKind::*;
        match node.kind() {
            LITERAL => literal(node),
            PHYSICAL_LITERAL => node
                .children()
                .find(|n| n.kind() == NAME)
                .and_then(|n| mark_token(&n))
                .map(|t| self.name_val(&t))
                .unwrap_or(Val::Unknown),
            PAREN_EXPR => match node.children().next() {
                Some(inner) => self.infer(&inner),
                None => Val::Unknown,
            },
            AGGREGATE => Val::Aggregate,
            QUALIFIED_EXPR => node
                .children()
                .next()
                .and_then(|mark| self.type_mark_of(&mark))
                .map_or(Val::Unknown, |t| Val::Types(vec![t])),
            NAME => match mark_token(node) {
                Some(t) if t.kind() == CHAR_LITERAL => Val::Char(t.text().to_owned()),
                Some(t) => self.name_val(&t),
                None => Val::Unknown,
            },
            SELECTED_NAME => self.selected_val(node),
            ATTRIBUTE_NAME => self.attribute_val(node, None),
            CALL_EXPR => self.call_val(node),
            BINARY_EXPR | UNARY_EXPR => {
                let (sigs, vals, _) = self.operator(node, None);
                // The predefined operators of an operand's unknown type are
                // missing from the candidates: the result is unknown too.
                if vals
                    .iter()
                    .any(|v| matches!(v, Val::Unknown | Val::NotValue))
                {
                    return Val::Unknown;
                }
                self.rets(sigs.iter().map(|s| s.ret))
            }
            CONDITIONAL_EXPR => match node.children().next() {
                Some(first) => self.infer(&first),
                None => Val::Unknown,
            },
            EXTERNAL_NAME => node
                .children()
                .find(|n| n.kind() == SUBTYPE_INDICATION)
                .and_then(|n| n.children().find(|c| c.kind() != RESOLUTION_INDICATION))
                .and_then(|mark| self.type_mark_of(&mark))
                .map_or(Val::Unknown, |t| Val::Types(vec![t])),
            RANGE_EXPR | SUBTYPE_INDICATION => Val::NotValue,
            _ => Val::Unknown,
        }
    }

    /// The union of result types; unknown if any is.
    pub(super) fn rets(&self, rets: impl Iterator<Item = Option<TyS>>) -> Val {
        let mut types: Vec<TyS> = Vec::new();
        let mut any = false;
        for ret in rets {
            any = true;
            match ret {
                Some(t) => {
                    if !types.iter().any(|x| x.ty == t.ty) {
                        types.push(t);
                    }
                }
                None => return Val::Unknown,
            }
        }
        if !any || types.is_empty() {
            Val::Unknown
        } else {
            Val::Types(types)
        }
    }

    /// The type a type mark names.
    pub(super) fn type_mark_of(&self, mark: &SyntaxNode) -> Option<TyS> {
        let token = mark_token(mark)?;
        let decls = self.target_decls(&token);
        let mut found: Option<DeclId> = None;
        for id in decls {
            let decl = self.decl(id)?;
            if matches!(
                decl.kind,
                DeclKind::Type | DeclKind::Subtype | DeclKind::IncompleteType
            ) {
                match found {
                    Some(f) if !self.same_decl(f, id) => return None,
                    _ => found = Some(id),
                }
            }
        }
        self.mark_type(found?)
    }

    /// What a resolved name denotes as a value.
    fn name_val(&self, token: &SyntaxToken) -> Val {
        let decls = self.target_decls(token);
        if decls.is_empty() {
            return Val::Unknown;
        }
        let mut types: Vec<TyS> = Vec::new();
        let mut value = false;
        for id in decls {
            let Some(decl) = self.decl(id) else {
                return Val::Unknown;
            };
            let ty = match decl.kind {
                DeclKind::EnumLiteral | DeclKind::PhysicalUnit => self.decl_type(id),
                DeclKind::Alias => match decl.subtype {
                    Some(_) => self.decl_type(id),
                    None => return Val::Unknown,
                },
                DeclKind::Function | DeclKind::GenericSubprogram => {
                    if self.is_wildcard(id) {
                        return Val::Unknown;
                    }
                    if !self.callable_without_args(id) {
                        continue;
                    }
                    self.return_type(id)
                }
                kind if kind.is_object() => self.decl_type(id),
                DeclKind::Procedure => continue,
                _ => {
                    // Types, units, labels, ...: not values.
                    continue;
                }
            };
            value = true;
            match ty {
                Some(t) => {
                    if !types.iter().any(|x| x.ty == t.ty) {
                        types.push(t);
                    }
                }
                None => return Val::Unknown,
            }
        }
        if !value {
            return Val::NotValue;
        }
        Val::Types(types)
    }

    pub(super) fn callable_without_args(&self, id: DeclId) -> bool {
        self.decl(id)
            .and_then(|d| d.params.as_deref())
            .is_some_and(|ps| {
                ps.iter().all(|&p| {
                    self.decl(p).is_some_and(|d| {
                        matches!(
                            d.kind,
                            DeclKind::Parameter {
                                has_default: true,
                                ..
                            }
                        )
                    })
                })
            })
    }

    /// `pkg.x`, `rec.field`, `ptr.all`, `ptr.field`, `obj.method`.
    fn selected_val(&mut self, node: &SyntaxNode) -> Val {
        let Some(suffix) = selected_suffix(node) else {
            return Val::Unknown;
        };
        if self.ref_index(&suffix).is_some() {
            return self.name_val(&suffix);
        }
        let Some(prefix) = node.children().next() else {
            return Val::Unknown;
        };
        let prefix_val = self.infer(&prefix);
        let Some(ty) = single_type(&prefix_val) else {
            return Val::Unknown;
        };
        if let Some(candidates) = self.methods(ty, &suffix) {
            let rets: Vec<Option<TyS>> = candidates
                .iter()
                .filter(|&&c| self.is_function(c) && self.callable_without_args(c))
                .map(|&c| self.return_type(c))
                .collect();
            return self.rets(rets.into_iter());
        }
        if suffix.kind() == SyntaxKind::ALL_KW {
            return match self.def(ty.ty) {
                Some(TypeDef::Access(Some(d))) => self
                    .mark_type(*d)
                    .map_or(Val::Unknown, |t| Val::Types(vec![t])),
                _ => Val::Unknown,
            };
        }
        let key = crate::syntax::designator_key(&suffix);
        let element = self.element_type(ty.ty, &key);
        if element.is_some() {
            self.element_ref(&suffix, ty.ty, key);
        }
        match element {
            Some(Some(t)) => Val::Types(vec![t]),
            _ => Val::Unknown,
        }
    }

    /// The subprograms named `suffix` of a protected type.
    pub(super) fn methods(&self, ty: TyS, suffix: &SyntaxToken) -> Option<Vec<DeclId>> {
        let Some(TypeDef::Protected) = self.def(ty.ty) else {
            return None;
        };
        let Ty::Decl(id) = ty.ty else { return None };
        let members = self.decl(id)?.members.clone()?;
        let key = crate::syntax::designator_key(suffix);
        Some(
            members
                .get(&key)
                .iter()
                .copied()
                .filter(|&c| self.decl(c).is_some_and(|d| Self::is_subprogram(d.kind)))
                .collect(),
        )
    }

    /// Records that `token` names element `name` of record type `ty`, or
    /// of the record it designates.
    pub(super) fn element_ref(&mut self, token: &SyntaxToken, ty: Ty, name: String) {
        let record = match (ty, self.def(ty)) {
            (Ty::Decl(id), Some(TypeDef::Record(_))) => id,
            (_, Some(TypeDef::Access(Some(d)))) => match self.mark_type(*d) {
                Some(TyS {
                    ty: Ty::Decl(id), ..
                }) => id,
                _ => return,
            },
            _ => return,
        };
        self.elements.push(ElementRef {
            range: token.text_range(),
            record,
            name,
        });
    }

    /// The type of element `name` of a record (or of the record an access
    /// type designates): `Some(None)` when the element exists but its type is
    /// unknown, `None` when there is no such element or no record.
    pub(super) fn element_type(&self, ty: Ty, name: &str) -> Option<Option<TyS>> {
        match self.def(ty)? {
            TypeDef::Record(elements) => {
                let e = elements.iter().find(|e| e.name == name)?;
                Some(e.subtype.and_then(|s| self.mark_type(s)))
            }
            TypeDef::Access(Some(d)) => {
                let t = self.mark_type(*d)?;
                match self.def(t.ty)? {
                    TypeDef::Record(_) => self.element_type(t.ty, name),
                    _ => None,
                }
            }
            _ => None,
        }
    }

    /// An attribute's value; `args` are the arguments of `x'attr(args)`.
    fn attribute_val(&mut self, node: &SyntaxNode, _args: Option<&SyntaxNode>) -> Val {
        let Some(attribute) = attribute_designator(node) else {
            return Val::Unknown;
        };
        let prefix = node.children().next();
        let prefix_ty = || -> Option<TyS> { None };
        let _ = prefix_ty;
        let boolean = || {
            self.std_ty(self.std.boolean)
                .map_or(Val::Unknown, |t| Val::Types(vec![t]))
        };
        match attribute.as_str() {
            "event" | "active" | "stable" | "quiet" | "ascending" => boolean(),
            "last_event" | "last_active" => self
                .std_ty(self.std.time)
                .map_or(Val::Unknown, |t| Val::Types(vec![t])),
            "image" | "instance_name" | "path_name" | "simple_name" => self
                .std_ty(self.std.string)
                .map_or(Val::Unknown, |t| Val::Types(vec![t])),
            "length" | "pos" => Val::Types(vec![TyS::of(Ty::UInt)]),
            "range" | "reverse_range" | "subtype" | "base" | "element" => Val::NotValue,
            "last_value" | "delayed" | "driving_value" => match prefix {
                Some(p) => match self.infer(&p) {
                    v @ Val::Types(_) => v,
                    _ => Val::Unknown,
                },
                None => Val::Unknown,
            },
            "val" | "succ" | "pred" | "leftof" | "rightof" | "value" => {
                match prefix.as_ref().and_then(|p| self.type_mark_of(p)) {
                    Some(t) => Val::Types(vec![t]),
                    None => Val::Unknown,
                }
            }
            "high" | "low" | "left" | "right" => {
                let Some(p) = prefix else {
                    return Val::Unknown;
                };
                let t = match self.type_mark_of(&p) {
                    Some(t) => Some(t),
                    None => single_type(&self.infer(&p)),
                };
                match t {
                    Some(t) => match self.def(t.ty) {
                        Some(TypeDef::Array { indexes, .. }) if indexes.len() == 1 => indexes[0]
                            .and_then(|i| self.mark_type(i))
                            .map_or(Val::Unknown, |t| Val::Types(vec![t])),
                        Some(
                            TypeDef::Enum(_) | TypeDef::Integer | TypeDef::Real | TypeDef::Physical,
                        ) => Val::Types(vec![t]),
                        _ => Val::Unknown,
                    },
                    None => Val::Unknown,
                }
            }
            _ => Val::Unknown,
        }
    }

    /// What a call's prefix names. `procedure` selects procedures rather
    /// than functions.
    pub(super) fn callee(&mut self, prefix: &SyntaxNode, procedure: bool) -> Callee {
        match prefix.kind() {
            SyntaxKind::NAME | SyntaxKind::SELECTED_NAME => {}
            SyntaxKind::ATTRIBUTE_NAME => return Callee::Attribute,
            _ => return Callee::Value(self.infer(prefix)),
        }
        let Some(token) = mark_token(prefix) else {
            return Callee::Value(Val::Unknown);
        };
        if prefix.kind() == SyntaxKind::SELECTED_NAME && self.ref_index(&token).is_none() {
            // A protected type method, or an element of a record.
            if let Some(p) = prefix.children().next()
                && let Some(ty) = single_type(&self.infer(&p))
                && let Some(candidates) = self.methods(ty, &token)
            {
                let candidates: Vec<DeclId> = candidates
                    .into_iter()
                    .filter(|&c| procedure != self.is_function(c))
                    .collect();
                if candidates.is_empty() {
                    return Callee::Value(Val::Unknown);
                }
                return Callee::Subprograms {
                    candidates,
                    token: Some(token),
                    add_ref: true,
                };
            }
            return Callee::Value(self.infer(prefix));
        }
        let decls = self.target_decls(&token);
        if decls.is_empty() {
            return Callee::Value(Val::Unknown);
        }
        let mut candidates = Vec::new();
        let mut mark = None;
        for &id in &decls {
            let Some(decl) = self.decl(id) else {
                return Callee::Value(Val::Unknown);
            };
            match decl.kind {
                kind if Self::is_subprogram(kind) => {
                    if procedure != self.is_function(id) {
                        candidates.push(id);
                    }
                }
                DeclKind::Alias if decl.subtype.is_none() => return Callee::Value(Val::Unknown),
                DeclKind::Type | DeclKind::Subtype | DeclKind::IncompleteType => mark = Some(id),
                DeclKind::GenericType => return Callee::Value(Val::Unknown),
                kind if kind.is_object() || kind == DeclKind::Alias => {
                    return Callee::Value(self.name_val(&token));
                }
                _ => {}
            }
        }
        if !candidates.is_empty() {
            return Callee::Subprograms {
                candidates,
                token: Some(token),
                add_ref: false,
            };
        }
        if let Some(mark) = mark {
            return match self.mark_type(mark) {
                Some(t) => Callee::Conversion(t),
                None => Callee::Value(Val::Unknown),
            };
        }
        if decls
            .iter()
            .any(|&id| self.decl(id).is_some_and(|d| Self::is_subprogram(d.kind)))
        {
            // Only procedures where a function is needed, or the reverse.
            return Callee::Value(Val::Unknown);
        }
        Callee::Other
    }

    fn call_val(&mut self, node: &SyntaxNode) -> Val {
        let Some(prefix) = node.children().next() else {
            return Val::Unknown;
        };
        let list = node
            .children()
            .find(|n| n.kind() == SyntaxKind::ASSOCIATION_LIST);
        match self.callee(&prefix, false) {
            Callee::Subprograms { candidates, .. } => {
                let args = self.args(list.as_ref());
                let mut rets = Vec::new();
                for &c in &candidates {
                    if self.match_call(c, &args).is_ok() {
                        rets.push(self.return_type(c));
                    } else if self.indexable_result(c) {
                        rets.push(None);
                    }
                }
                self.rets(rets.into_iter())
            }
            Callee::Conversion(t) => Val::Types(vec![t]),
            Callee::Value(val) => self.index_val(&val, list.as_ref()),
            Callee::Attribute => self.attribute_val(&prefix, list.as_ref()),
            Callee::Other => Val::Unknown,
        }
    }

    /// The array type indexed or sliced by `list`, if known: `(array,
    /// is_slice)`.
    pub(super) fn indexed_array(
        &mut self,
        val: &Val,
        list: Option<&SyntaxNode>,
    ) -> Option<(TyS, bool)> {
        let mut ty = single_type(val)?;
        if let Some(TypeDef::Access(Some(d))) = self.def(ty.ty) {
            ty = self.mark_type(*d)?;
        }
        let Some(TypeDef::Array { .. }) = self.def(ty.ty) else {
            return None;
        };
        let elements: Vec<SyntaxNode> = list?
            .children()
            .filter(|n| n.kind() == SyntaxKind::ASSOCIATION_ELEMENT)
            .collect();
        let slice = match elements.as_slice() {
            [one] => one.children().next().is_some_and(|n| self.is_range(&n)),
            _ => false,
        };
        Some((ty, slice))
    }

    fn index_val(&mut self, val: &Val, list: Option<&SyntaxNode>) -> Val {
        let Some((ty, slice)) = self.indexed_array(val, list) else {
            return Val::Unknown;
        };
        if slice {
            return Val::Types(vec![ty]);
        }
        let count = list.map_or(0, |l| {
            l.children()
                .filter(|n| n.kind() == SyntaxKind::ASSOCIATION_ELEMENT)
                .count()
        });
        match self.def(ty.ty) {
            Some(TypeDef::Array {
                indexes,
                element: Some(element),
            }) if indexes.len() == count => self
                .mark_type(*element)
                .map_or(Val::Unknown, |t| Val::Types(vec![t])),
            _ => Val::Unknown,
        }
    }

    /// Whether a node is a discrete range: `a to b`, `t range ...`,
    /// `x'range`, or a type mark.
    pub(super) fn is_range(&self, node: &SyntaxNode) -> bool {
        match node.kind() {
            SyntaxKind::RANGE_EXPR | SyntaxKind::SUBTYPE_INDICATION => true,
            SyntaxKind::ATTRIBUTE_NAME => matches!(
                attribute_designator(node).as_deref(),
                Some("range" | "reverse_range")
            ),
            SyntaxKind::NAME | SyntaxKind::SELECTED_NAME => mark_token(node).is_some_and(|t| {
                let decls = self.target_decls(&t);
                !decls.is_empty()
                    && decls.iter().all(|&id| {
                        self.decl(id).is_some_and(|d| {
                            matches!(
                                d.kind,
                                DeclKind::Type
                                    | DeclKind::Subtype
                                    | DeclKind::IncompleteType
                                    | DeclKind::GenericType
                            )
                        })
                    })
            }),
            _ => false,
        }
    }
}

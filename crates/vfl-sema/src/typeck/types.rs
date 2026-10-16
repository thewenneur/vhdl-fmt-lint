//! Declarations and types: type marks, base types and compatibility.

use super::{Checker, Param, Ty, TyS, Val};
use crate::decl::{Decl, DeclId, DeclKind, TypeDef};

impl<'a> Checker<'a> {
    // ---- Declarations and types ---------------------------------------------

    pub(super) fn decl(&self, id: DeclId) -> Option<&'a Decl> {
        if id.unit == self.unit {
            self.result.decls.get(id.index as usize)
        } else {
            self.ctx.decl(id)
        }
    }

    /// The base type of a type or subtype declaration.
    fn base(&self, mut id: DeclId) -> Option<DeclId> {
        for _ in 0..32 {
            let decl = self.decl(id)?;
            match decl.kind {
                DeclKind::Type => return Some(id),
                DeclKind::Subtype | DeclKind::IncompleteType => id = decl.subtype?,
                _ => return None,
            }
        }
        None
    }

    /// A type mark's type, shown by the mark's name.
    pub(super) fn mark_type(&self, mark: DeclId) -> Option<TyS> {
        Some(TyS {
            ty: Ty::Decl(self.base(mark)?),
            shown: Some(mark),
        })
    }

    /// The type of an object, literal, unit or function result.
    pub(super) fn decl_type(&self, id: DeclId) -> Option<TyS> {
        if let Some(t) = self.local_types.get(&id) {
            return Some(*t);
        }
        self.mark_type(self.decl(id)?.subtype?)
    }

    pub(super) fn def(&self, ty: Ty) -> Option<&'a TypeDef> {
        match ty {
            Ty::Decl(id) => self.decl(id)?.type_def.as_deref(),
            _ => None,
        }
    }

    pub(super) fn std_ty(&self, id: Option<DeclId>) -> Option<TyS> {
        id.map(|id| TyS {
            ty: Ty::Decl(id),
            shown: Some(id),
        })
    }

    pub(super) fn is_std(&self, ty: Ty, id: Option<DeclId>) -> bool {
        match (ty, id) {
            (Ty::Decl(a), Some(b)) => self.same_decl(a, b),
            _ => false,
        }
    }

    /// The same type, also when a unit exists twice (a file in two
    /// libraries, alternate copies of a package).
    pub(super) fn same_decl(&self, a: DeclId, b: DeclId) -> bool {
        if a == b {
            return true;
        }
        let (Some(x), Some(y)) = (self.decl(a), self.decl(b)) else {
            return false;
        };
        x.name == y.name
            && self.ctx.units[a.unit as usize].name == self.ctx.units[b.unit as usize].name
            && x.kind == y.kind
    }

    /// `ieee.std_logic_1164.std_ulogic`.
    pub(super) fn is_std_ulogic(&self, ty: Ty) -> bool {
        match ty {
            Ty::Decl(id) => {
                self.decl(id).is_some_and(|d| d.name == "std_ulogic")
                    && self.ctx.units[id.unit as usize].name == "std_logic_1164"
            }
            _ => false,
        }
    }

    pub(super) fn is_integer(&self, ty: Ty) -> Option<bool> {
        match ty {
            Ty::UInt => Some(true),
            Ty::UReal => Some(false),
            Ty::Decl(_) => match self.def(ty)? {
                TypeDef::Integer => Some(true),
                TypeDef::Scalar => None,
                _ => Some(false),
            },
        }
    }

    pub(super) fn is_real(&self, ty: Ty) -> Option<bool> {
        match ty {
            Ty::UReal => Some(true),
            Ty::UInt => Some(false),
            Ty::Decl(_) => match self.def(ty)? {
                TypeDef::Real => Some(true),
                TypeDef::Scalar => None,
                _ => Some(false),
            },
        }
    }

    /// Whether a value of type `actual` can be used where `formal` is
    /// expected (universal types convert implicitly).
    pub(super) fn ty_compat(&self, actual: Ty, formal: Ty) -> bool {
        match (actual, formal) {
            (Ty::Decl(a), Ty::Decl(b)) => {
                self.same_decl(a, b) || self.def(actual).is_none() || self.def(formal).is_none()
            }
            (Ty::UInt, Ty::UInt) | (Ty::UReal, Ty::UReal) => true,
            (Ty::UInt, Ty::Decl(_)) => self.is_integer(formal) != Some(false),
            (Ty::UReal, Ty::Decl(_)) => self.is_real(formal) != Some(false),
            _ => false,
        }
    }

    /// Whether `val` may have type `ty`.
    pub(super) fn compat(&self, val: &Val, ty: Ty) -> bool {
        match val {
            Val::Unknown | Val::NotValue | Val::Null => true,
            Val::Types(types) => types.iter().any(|t| self.ty_compat(t.ty, ty)),
            Val::Str(text) => match self.def(ty) {
                None => !matches!(ty, Ty::UInt | Ty::UReal),
                Some(TypeDef::Array { indexes, element }) => {
                    if indexes.len() != 1 {
                        return false;
                    }
                    let Some(element) = element.and_then(|e| self.base(e)) else {
                        return true;
                    };
                    match self.def(Ty::Decl(element)) {
                        None => true,
                        // `character` has every character.
                        Some(TypeDef::Enum(_))
                            if self.is_std(Ty::Decl(element), self.std.character) =>
                        {
                            true
                        }
                        Some(TypeDef::Enum(literals)) => match text {
                            Some(text) => text.chars().all(|c| {
                                let mut key = [0u8; 3];
                                key[0] = b'\'';
                                key[2] = b'\'';
                                !c.is_ascii() || {
                                    key[1] = c as u8;
                                    let key = std::str::from_utf8(&key).unwrap_or_default();
                                    literals.iter().any(|l| l == key)
                                }
                            }),
                            None => true,
                        },
                        Some(_) => false,
                    }
                }
                Some(_) => false,
            },
            Val::Aggregate => match self.def(ty) {
                None => !matches!(ty, Ty::UInt | Ty::UReal),
                Some(def) => matches!(def, TypeDef::Array { .. } | TypeDef::Record(_)),
            },
            Val::Char(c) => match self.def(ty) {
                None => !matches!(ty, Ty::UInt | Ty::UReal),
                Some(TypeDef::Enum(literals)) => literals.contains(c),
                Some(_) => false,
            },
        }
    }

    pub(super) fn param_compat(&self, val: &Val, param: Param) -> bool {
        match param {
            Param::Ty(ty) => self.compat(val, ty),
            Param::Any => true,
            Param::AnyInteger | Param::AnyReal => match val {
                Val::Types(types) => types.iter().any(|t| {
                    let ok = if matches!(param, Param::AnyInteger) {
                        self.is_integer(t.ty)
                    } else {
                        self.is_real(t.ty)
                    };
                    ok != Some(false)
                }),
                Val::Unknown | Val::NotValue => true,
                _ => false,
            },
        }
    }

    /// Whether every type of `val` is known, so a mismatch is certain.
    pub(super) fn definite(&self, val: &Val) -> bool {
        match val {
            Val::Unknown | Val::NotValue | Val::Null => false,
            Val::Types(types) => types.iter().all(|t| match t.ty {
                Ty::Decl(_) => self.def(t.ty).is_some(),
                _ => true,
            }),
            _ => true,
        }
    }
}

//! Operators: the signatures, declared and predefined, an operator may
//! have.

use vfl_syntax::SyntaxNode;

use super::{Checker, Exp, OpSig, Param, Ty, TyS, Val};
use crate::decl::{DeclId, DeclKind, TypeDef};
use crate::resolver::operator_token;

impl<'a> Checker<'a> {
    // ---- Operators ----------------------------------------------------------

    /// The operator signatures that accept the operands of `node`, the
    /// operands' values, and whether candidates are unknown (an operator
    /// from a library outside the project).
    pub(super) fn operator(
        &mut self,
        node: &SyntaxNode,
        exp: Option<Exp>,
    ) -> (Vec<OpSig>, Vec<Val>, bool) {
        let operands: Vec<SyntaxNode> = node.children().collect();
        let vals: Vec<Val> = operands.iter().map(|n| self.infer(n)).collect();
        let Some(op) = operator_token(node) else {
            return (Vec::new(), vals, true);
        };
        let symbol = op.text().to_ascii_lowercase();
        let opaque = self
            .result
            .opaque_ops
            .binary_search_by_key(&op.text_range().start(), |r| r.start())
            .is_ok();
        let mut sigs = Vec::new();
        for id in self.target_decls(&op) {
            let Some(decl) = self.decl(id) else { continue };
            if decl.kind == DeclKind::Alias || self.is_wildcard(id) {
                sigs.push(OpSig {
                    decl: Some(id),
                    params: vec![Param::Any; vals.len()],
                    ret: None,
                });
                continue;
            }
            if !Self::is_subprogram(decl.kind) {
                continue;
            }
            let params: &[DeclId] = decl.params.as_deref().unwrap_or(&[]);
            if params.len() != vals.len() {
                continue;
            }
            sigs.push(OpSig {
                decl: Some(id),
                params: params.iter().map(|&p| self.param_type(p)).collect(),
                ret: self.return_type(id),
            });
        }
        let mut types: Vec<TyS> = Vec::new();
        for v in &vals {
            if let Val::Types(ts) = v {
                for t in ts {
                    if !types.iter().any(|x| x.ty == t.ty) {
                        types.push(*t);
                    }
                }
            }
        }
        if let Some(exp) = exp
            && !types.iter().any(|x| x.ty == exp.ty)
        {
            types.push(TyS {
                ty: exp.ty,
                shown: exp.shown,
            });
        }
        for t in types {
            self.predefined(&symbol, vals.len(), t, &mut sigs);
        }
        let viable: Vec<OpSig> = sigs
            .into_iter()
            .filter(|s| {
                s.params
                    .iter()
                    .zip(&vals)
                    .all(|(p, v)| self.param_compat(v, *p))
            })
            .collect();
        (viable, vals, opaque)
    }

    /// The predefined operators of type `t` named `op`.
    fn predefined(&self, op: &str, arity: usize, t: TyS, out: &mut Vec<OpSig>) {
        let ty = t.ty;
        let def = match ty {
            Ty::UInt => Some(&TypeDef::Integer),
            Ty::UReal => Some(&TypeDef::Real),
            Ty::Decl(_) => self.def(ty),
        };
        let Some(def) = def else { return };
        let boolean = self.std_ty(self.std.boolean);
        let same = Param::Ty(ty);
        let mut add = |params: Vec<Param>, ret: Option<TyS>| {
            out.push(OpSig {
                decl: None,
                params,
                ret,
            })
        };
        let integer = matches!(def, TypeDef::Integer | TypeDef::Scalar);
        let real = matches!(def, TypeDef::Real | TypeDef::Scalar);
        let numeric = integer || real || matches!(def, TypeDef::Physical);
        let scalar = numeric || matches!(def, TypeDef::Enum(_));
        let bitlike = self.is_std(ty, self.std.boolean) || self.is_std(ty, self.std.bit);
        // The matching operators are predefined for `bit` and `std_ulogic`.
        let matching = |t: Ty| self.is_std(t, self.std.bit) || self.is_std_ulogic(t);
        // A one-dimensional array and its element type.
        let array = match def {
            TypeDef::Array { indexes, element } if indexes.len() == 1 => {
                Some(element.and_then(|e| self.mark_type(e)))
            }
            _ => None,
        };
        let element_bitlike = matches!(array, Some(Some(e))
            if self.is_std(e.ty, self.std.boolean) || self.is_std(e.ty, self.std.bit));
        let element_discrete = match array {
            Some(Some(e)) => matches!(
                self.def(e.ty),
                None | Some(TypeDef::Enum(_) | TypeDef::Integer | TypeDef::Scalar)
            ),
            Some(None) => true,
            None => false,
        };
        let element = |e: Option<TyS>| e.map_or(Param::Any, |e| Param::Ty(e.ty));
        match (op, arity) {
            ("=" | "/=", 2) => {
                if !matches!(def, TypeDef::File(_) | TypeDef::Protected) {
                    add(vec![same, same], boolean);
                }
            }
            ("<" | "<=" | ">" | ">=", 2) => {
                if scalar || element_discrete {
                    add(vec![same, same], boolean);
                }
            }
            ("?=" | "?/=" | "?<" | "?<=" | "?>" | "?>=", 2) => {
                if matching(ty) {
                    add(vec![same, same], Some(t));
                } else if matches!(op, "?=" | "?/=")
                    && let Some(Some(e)) = array
                    && matching(e.ty)
                {
                    add(vec![same, same], Some(e));
                }
            }
            ("and" | "or" | "nand" | "nor" | "xor" | "xnor", 2) => {
                if bitlike {
                    add(vec![same, same], Some(t));
                } else if element_bitlike {
                    let e = element(array.flatten());
                    add(vec![same, same], Some(t));
                    add(vec![same, e], Some(t));
                    add(vec![e, same], Some(t));
                }
            }
            ("and" | "or" | "nand" | "nor" | "xor" | "xnor", 1) => {
                if element_bitlike {
                    add(vec![same], array.flatten());
                }
            }
            ("not", 1) => {
                if bitlike || element_bitlike {
                    add(vec![same], Some(t));
                }
            }
            ("sll" | "srl" | "sla" | "sra" | "rol" | "ror", 2) => {
                if element_bitlike {
                    add(vec![same, Param::AnyInteger], Some(t));
                }
            }
            ("+" | "-", 2) => {
                if numeric {
                    add(vec![same, same], Some(t));
                }
            }
            ("+" | "-" | "abs", 1) => {
                if numeric {
                    add(vec![same], Some(t));
                }
            }
            ("&", 2) => {
                if let Some(e) = array {
                    let e = element(e);
                    add(vec![same, same], Some(t));
                    add(vec![same, e], Some(t));
                    add(vec![e, same], Some(t));
                    add(vec![e, e], Some(t));
                }
            }
            ("*", 2) => {
                if integer || real {
                    add(vec![same, same], Some(t));
                }
                if ty == Ty::UReal {
                    add(vec![same, Param::Ty(Ty::UInt)], Some(t));
                    add(vec![Param::Ty(Ty::UInt), same], Some(t));
                }
                if matches!(def, TypeDef::Physical) {
                    add(vec![same, Param::AnyInteger], Some(t));
                    add(vec![same, Param::AnyReal], Some(t));
                    add(vec![Param::AnyInteger, same], Some(t));
                    add(vec![Param::AnyReal, same], Some(t));
                }
            }
            ("/", 2) => {
                if integer || real {
                    add(vec![same, same], Some(t));
                }
                if ty == Ty::UReal {
                    add(vec![same, Param::Ty(Ty::UInt)], Some(t));
                }
                if matches!(def, TypeDef::Physical) {
                    add(vec![same, Param::AnyInteger], Some(t));
                    add(vec![same, Param::AnyReal], Some(t));
                    add(vec![same, same], Some(TyS::of(Ty::UInt)));
                }
            }
            ("mod" | "rem", 2) => {
                if integer || matches!(def, TypeDef::Physical) {
                    add(vec![same, same], Some(t));
                }
            }
            ("**", 2) => {
                if integer || real {
                    add(vec![same, Param::AnyInteger], Some(t));
                }
            }
            ("??", 1) if self.is_std(ty, self.std.bit) => add(vec![same], boolean),
            _ => {}
        }
    }
}

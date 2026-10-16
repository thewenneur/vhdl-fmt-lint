//! Subprograms: matching calls against a subprogram's parameters.

use rowan::NodeOrToken;
use vfl_syntax::{SyntaxKind, SyntaxNode};

use super::{Arg, Checker, Formal, Param, Reject, TyS, Val};
use crate::decl::{DeclId, DeclKind, TypeDef};
use crate::resolver::mark_token;
use crate::syntax::has_token;

impl<'a> Checker<'a> {
    // ---- Subprograms --------------------------------------------------------

    pub(super) fn is_subprogram(kind: DeclKind) -> bool {
        matches!(
            kind,
            DeclKind::Function | DeclKind::Procedure | DeclKind::GenericSubprogram
        )
    }

    /// Operations `std.standard` declares once for every type (`to_string`,
    /// `minimum`, file operations): their parameters are not checked.
    fn is_implicit(&self, id: DeclId) -> bool {
        Some(id.unit) == self.ctx.standard
            && self
                .decl(id)
                .is_some_and(|d| Self::is_subprogram(d.kind) && d.name != "now")
    }

    /// Candidates whose parameters are unknown: they accept any call.
    pub(super) fn is_wildcard(&self, id: DeclId) -> bool {
        self.is_implicit(id) || self.decl(id).is_none_or(|d| d.params.is_none())
    }

    pub(super) fn is_function(&self, id: DeclId) -> bool {
        self.decl(id).is_some_and(|d| match d.kind {
            DeclKind::Function => true,
            DeclKind::GenericSubprogram => d.subtype.is_some() || d.params.is_none(),
            _ => false,
        })
    }

    /// A function's result type; `None` when unknown.
    pub(super) fn return_type(&self, id: DeclId) -> Option<TyS> {
        let decl = self.decl(id)?;
        if self.is_implicit(id) && matches!(decl.name.as_str(), "minimum" | "maximum") {
            return None;
        }
        self.decl_type(id)
    }

    pub(super) fn param_type(&self, param: DeclId) -> Param {
        match self.decl_type(param) {
            Some(t) => Param::Ty(t.ty),
            None => Param::Any,
        }
    }

    /// Matches a call's arguments to a subprogram's parameters.
    pub(super) fn match_call(
        &self,
        id: DeclId,
        args: &[Arg],
    ) -> Result<Vec<(usize, DeclId)>, Reject> {
        if self.is_wildcard(id) {
            return Ok(Vec::new());
        }
        let params: &[DeclId] = self
            .decl(id)
            .and_then(|d| d.params.as_deref())
            .unwrap_or(&[]);
        // Cheap rejections first: most candidates of a heavily overloaded
        // name fail on the argument count.
        let positional = args
            .iter()
            .take_while(|a| matches!(a.formal, Formal::Positional))
            .count();
        let simple = args
            .iter()
            .filter(|a| !matches!(a.formal, Formal::Complex))
            .count();
        // Partial associations (`r.a => x`, `v(0) => y`) may name a formal
        // several times, so only the simple ones count.
        if positional > params.len() || simple > params.len() {
            return Err(Reject::TooMany(params.len()));
        }
        let mut assigned: Vec<Option<usize>> = vec![None; params.len()];
        let mut complex = false;
        let mut position = 0;
        for (i, arg) in args.iter().enumerate() {
            match &arg.formal {
                Formal::Positional => {
                    if position >= params.len() {
                        return Err(Reject::TooMany(params.len()));
                    }
                    assigned[position] = Some(i);
                    position += 1;
                }
                Formal::Named(name) => {
                    let Some(p) = params
                        .iter()
                        .position(|&p| self.decl(p).is_some_and(|d| d.name == *name))
                    else {
                        return Err(Reject::NoParam(i));
                    };
                    if assigned[p].is_some() {
                        return Err(Reject::Twice(params[p]));
                    }
                    assigned[p] = Some(i);
                }
                Formal::Complex => complex = true,
            }
        }
        if !complex {
            for (p, a) in params.iter().zip(&assigned) {
                if a.is_none()
                    && let Some(d) = self.decl(*p)
                    && matches!(
                        d.kind,
                        DeclKind::Parameter {
                            has_default: false,
                            ..
                        }
                    )
                {
                    return Err(Reject::Missing(*p));
                }
            }
        }
        let mut pairs = Vec::new();
        for (p, a) in params.iter().zip(&assigned) {
            let Some(a) = *a else { continue };
            if !self.param_compat(&args[a].val, self.param_type(*p)) {
                return Err(Reject::Mismatch { arg: a, param: *p });
            }
            pairs.push((a, *p));
        }
        Ok(pairs)
    }

    /// Whether `id` may be a function without arguments whose result is
    /// indexed or sliced: `f(3)` calling `f` and indexing its array result.
    pub(super) fn indexable_result(&self, id: DeclId) -> bool {
        let Some(decl) = self.decl(id) else {
            return false;
        };
        let no_args = decl.params.as_deref().is_some_and(|ps| {
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
        });
        no_args
            && self.return_type(id).is_none_or(|t| {
                matches!(
                    self.def(t.ty),
                    None | Some(TypeDef::Array { .. } | TypeDef::Access(_))
                )
            })
    }

    /// The arguments of an association list, with their inferred values.
    pub(super) fn args(&mut self, list: Option<&SyntaxNode>) -> Vec<Arg> {
        let Some(list) = list else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for element in list
            .children()
            .filter(|n| n.kind() == SyntaxKind::ASSOCIATION_ELEMENT)
        {
            let has_arrow = has_token(&element, SyntaxKind::ARROW);
            let mut nodes = Vec::new();
            let mut formal_node = None;
            let mut past_arrow = !has_arrow;
            for child in element.children_with_tokens() {
                match child {
                    NodeOrToken::Token(t) if t.kind() == SyntaxKind::ARROW => past_arrow = true,
                    NodeOrToken::Node(n) if past_arrow => nodes.push(n),
                    NodeOrToken::Node(n) => formal_node = Some(n),
                    _ => {}
                }
            }
            let formal = match &formal_node {
                None => Formal::Positional,
                Some(n) if n.kind() == SyntaxKind::NAME => match mark_token(n) {
                    Some(t) if t.kind() != SyntaxKind::CHAR_LITERAL => {
                        Formal::Named(crate::syntax::designator_key(&t))
                    }
                    _ => Formal::Complex,
                },
                Some(_) => Formal::Complex,
            };
            let actual = nodes.into_iter().next();
            let val = match &actual {
                Some(n) => self.infer(n),
                None => Val::Unknown,
            };
            out.push(Arg {
                formal,
                actual,
                val,
            });
        }
        out
    }
}

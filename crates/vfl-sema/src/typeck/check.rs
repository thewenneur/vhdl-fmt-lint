//! Checking: expressions against the type their context expects,
//! resolving the overloads inside them.

use rowan::NodeOrToken;
use vfl_syntax::{SyntaxKind, SyntaxNode, SyntaxToken, identifier_key};

use super::{
    Arg, Callee, Checker, Exp, Formal, MAX_DEPTH, OpSig, Param, Reject, Ty, Val,
    attribute_designator, selected_suffix, single_type, trimmed_range, unique_sig,
};
use crate::decl::{Access, DeclId, DeclKind, Reference, Target, TypeDef};
use crate::diagnostic::codes;
use crate::resolver::{mark_token, operator_token};
use crate::syntax::is_designator;

impl<'a> Checker<'a> {
    // ---- Checking -----------------------------------------------------------

    /// Checks an expression against the type its context expects (if any),
    /// resolving the overloads inside it.
    pub(super) fn check(&mut self, node: &SyntaxNode, exp: Option<Exp>) {
        if self.depth >= MAX_DEPTH {
            return;
        }
        self.depth += 1;
        self.check_node(node, exp);
        self.depth -= 1;
    }

    fn check_node(&mut self, node: &SyntaxNode, exp: Option<Exp>) {
        use SyntaxKind::*;
        match node.kind() {
            PAREN_EXPR => {
                if let Some(inner) = node.children().next() {
                    self.check(&inner, exp);
                }
            }
            CALL_EXPR => self.check_call(node, exp, false),
            BINARY_EXPR | UNARY_EXPR => self.check_operator(node, exp),
            AGGREGATE => self.check_aggregate(node, exp),
            QUALIFIED_EXPR => {
                let mut children = node.children();
                let mark = children.next();
                let t = mark.as_ref().and_then(|m| self.type_mark_of(m));
                if let Some(operand) = children.next() {
                    match operand.kind() {
                        PAREN_EXPR => {
                            if let Some(inner) = operand.children().next() {
                                self.check(&inner, t.map(Exp::from));
                            }
                        }
                        _ => self.check(&operand, t.map(Exp::from)),
                    }
                }
                let val = self.infer(node);
                self.expect(node, &val, exp);
            }
            CONDITIONAL_EXPR => self.conditional(node, exp),
            NAME => {
                if let Some(token) = mark_token(node) {
                    self.check_name(node, &token, exp);
                }
            }
            SELECTED_NAME => self.check_selected(node, exp),
            ATTRIBUTE_NAME => {
                if let Some(prefix) = node.children().next() {
                    self.check_prefix(&prefix);
                }
                for rest in node.children().skip(1) {
                    self.check(&rest, None);
                }
                let val = self.infer(node);
                self.expect(node, &val, exp);
            }
            LITERAL | PHYSICAL_LITERAL => {
                let val = self.infer(node);
                self.expect(node, &val, exp);
            }
            RANGE_EXPR => {
                for child in node.children() {
                    self.check(&child, exp);
                }
            }
            ALLOCATOR => {
                // `new t'(x)`: the qualified expression checks itself.
                for child in node.children() {
                    for inner in child.children() {
                        if inner.kind() == QUALIFIED_EXPR {
                            self.check(&inner, None);
                        }
                    }
                }
            }
            ERROR => {
                self.quiet += 1;
                for child in node.children() {
                    self.check(&child, None);
                }
                self.quiet -= 1;
            }
            SUBTYPE_INDICATION | EXTERNAL_NAME => {}
            _ => {
                for child in node.children() {
                    self.check(&child, None);
                }
            }
        }
    }

    /// The prefix of an attribute: a type mark, an object, or a name.
    fn check_prefix(&mut self, prefix: &SyntaxNode) {
        match prefix.kind() {
            SyntaxKind::NAME => {}
            SyntaxKind::SELECTED_NAME | SyntaxKind::CALL_EXPR | SyntaxKind::ATTRIBUTE_NAME => {
                self.check(prefix, None)
            }
            _ => {}
        }
    }

    fn check_name(&mut self, node: &SyntaxNode, token: &SyntaxToken, exp: Option<Exp>) {
        let val = self.infer(node);
        if let Some(exp) = exp {
            // An overloaded enumeration literal or function: keep the ones
            // of the expected type.
            let decls = self.target_decls(token);
            if decls.len() > 1 {
                let keep: Vec<DeclId> = decls
                    .iter()
                    .copied()
                    .filter(|&id| {
                        let t = match self.decl(id).map(|d| d.kind) {
                            Some(DeclKind::Function | DeclKind::GenericSubprogram) => {
                                if !self.callable_without_args(id) {
                                    return false;
                                }
                                self.return_type(id)
                            }
                            _ => self.decl_type(id),
                        };
                        t.is_none_or(|t| self.ty_compat(t.ty, exp.ty))
                    })
                    .collect();
                self.narrow(token, &keep);
            }
        }
        self.expect(node, &val, exp);
    }

    fn check_selected(&mut self, node: &SyntaxNode, exp: Option<Exp>) {
        let Some(suffix) = selected_suffix(node) else {
            return;
        };
        if self.ref_index(&suffix).is_some() {
            self.check_name(node, &suffix, exp);
            return;
        }
        let Some(prefix) = node.children().next() else {
            return;
        };
        if let Some(ty) = single_type(&self.infer(&prefix))
            && let Some(candidates) = self.methods(ty, &suffix)
        {
            // `obj.f` calls a method without arguments.
            self.check(&prefix, None);
            let keep: Vec<DeclId> = candidates
                .iter()
                .copied()
                .filter(|&c| {
                    self.is_function(c)
                        && self.callable_without_args(c)
                        && exp.is_none_or(|e| {
                            self.return_type(c)
                                .is_none_or(|t| self.ty_compat(t.ty, e.ty))
                        })
                })
                .collect();
            let targets = if keep.is_empty() { &candidates } else { &keep };
            self.added.push(Reference {
                range: suffix.text_range(),
                targets: targets.iter().map(|&c| Target::Decl(c)).collect(),
                access: Access::Other,
            });
            let val = self.infer(node);
            self.expect(node, &val, exp);
            return;
        }
        self.check(&prefix, None);
        if is_designator(suffix.kind()) && suffix.kind() != SyntaxKind::CHAR_LITERAL {
            let prefix_val = self.infer(&prefix);
            if let Some(ty) = single_type(&prefix_val)
                && self.definite(&prefix_val)
            {
                let key = crate::syntax::designator_key(&suffix);
                let is_record = match self.def(ty.ty) {
                    Some(TypeDef::Record(_)) => true,
                    Some(TypeDef::Access(Some(d))) => self
                        .mark_type(*d)
                        .is_some_and(|t| matches!(self.def(t.ty), Some(TypeDef::Record(_)))),
                    _ => false,
                };
                if is_record && self.element_type(ty.ty, &key).is_none() {
                    let message = format!(
                        "`{}` is not an element of `{}`",
                        suffix.text(),
                        self.ty_name(ty.ty, ty.shown)
                    );
                    self.error(codes::UNRESOLVED_NAME, suffix.text_range(), message);
                    return;
                }
            }
        }
        let val = self.infer(node);
        self.expect(node, &val, exp);
    }

    pub(super) fn conditional(&mut self, node: &SyntaxNode, exp: Option<Exp>) {
        let mut after_when = false;
        for child in node.children_with_tokens() {
            match child {
                NodeOrToken::Token(t) => match t.kind() {
                    SyntaxKind::WHEN_KW => after_when = true,
                    SyntaxKind::ELSE_KW => after_when = false,
                    _ => {}
                },
                NodeOrToken::Node(n) => {
                    if after_when {
                        self.condition(&n);
                    } else {
                        self.check(&n, exp);
                    }
                }
            }
        }
    }

    /// A condition: boolean, or (VHDL-2008) a type with a `??` operator.
    pub(super) fn condition(&mut self, node: &SyntaxNode) {
        let boolean = self.std_ty(self.std.boolean).map(Exp::from);
        let val = self.infer(node);
        match boolean {
            Some(b) if self.compat(&val, b.ty) => self.check(node, Some(b)),
            Some(b) => {
                self.check(node, None);
                // `??` makes bit and std_ulogic (any enumeration with a `??`)
                // conditions; other types never are.
                if let Val::Types(types) = &val
                    && self.definite(&val)
                    && types
                        .iter()
                        .all(|t| !matches!(self.def(t.ty), Some(TypeDef::Enum(_))))
                {
                    self.mismatch(node, &val, b);
                }
            }
            None => self.check(node, None),
        }
    }

    fn check_operator(&mut self, node: &SyntaxNode, exp: Option<Exp>) {
        // The context may admit operators the operands alone don't show:
        // `"0" & x` builds an array of the expected type.
        let (viable, vals, opaque) = self.operator(node, exp);
        let operands: Vec<SyntaxNode> = node.children().collect();
        let Some(op) = operator_token(node) else {
            return;
        };
        let symbol = op.text().to_ascii_lowercase();
        if viable.is_empty() {
            for operand in &operands {
                self.check(operand, None);
            }
            // `&` builds arrays from elements, which only the context types.
            // Literals alone (`'A' /= 'a'`, `B"1100" sll 3`) take their
            // type from the context, which may not be known here.
            let known = vals.iter().all(|v| self.definite(v))
                && vals.iter().any(|v| {
                    matches!(v, Val::Types(ts) if ts.iter().any(|t| matches!(t.ty, Ty::Decl(_))))
                });
            if !opaque && known && symbol != "&" {
                let operands: Vec<String> = vals.iter().map(|v| self.describe(v)).collect();
                let message = format!("no `{}` operator for {}", op.text(), operands.join(" and "));
                self.error(codes::NO_MATCHING_OVERLOAD, op.text_range(), message);
            }
            return;
        }
        let mut chosen = viable;
        if let Some(exp) = exp {
            let fitting: Vec<OpSig> = chosen
                .iter()
                .filter(|s| s.ret.is_none_or(|r| self.ty_compat(r.ty, exp.ty)))
                .cloned()
                .collect();
            if fitting.is_empty() {
                let val = self.rets(chosen.iter().map(|s| s.ret));
                for operand in &operands {
                    self.check(operand, None);
                }
                self.expect(node, &val, Some(exp));
                return;
            }
            chosen = fitting;
        }
        // Narrow the operator's reference to the declarations chosen.
        if let Some(index) = self.ref_index(&op) {
            let explicit: Vec<DeclId> = chosen.iter().filter_map(|s| s.decl).collect();
            if explicit.is_empty() {
                self.dropped.push(index);
            } else if chosen.iter().all(|s| s.decl.is_some()) {
                self.narrow(&op, &explicit);
            }
        }
        let unique = unique_sig(&chosen);
        for (i, operand) in operands.iter().enumerate() {
            let param = unique.and_then(|s| s.params.get(i).copied());
            let exp = match param {
                Some(Param::Ty(ty)) => Some(Exp { ty, shown: None }),
                _ => None,
            };
            // A universal operand of a specific operator stays unchecked.
            self.check(
                operand,
                exp.filter(|e| !matches!(e.ty, Ty::UInt | Ty::UReal)),
            );
        }
    }

    pub(super) fn check_call(&mut self, node: &SyntaxNode, exp: Option<Exp>, procedure: bool) {
        let Some(prefix) = node.children().next() else {
            return;
        };
        let list = node
            .children()
            .find(|n| n.kind() == SyntaxKind::ASSOCIATION_LIST);
        match self.callee(&prefix, procedure) {
            Callee::Subprograms {
                candidates,
                token,
                add_ref,
            } => self.check_subprogram_call(
                node,
                &prefix,
                list.as_ref(),
                candidates,
                token,
                add_ref,
                exp,
                procedure,
            ),
            Callee::Conversion(t) => {
                if prefix.kind() == SyntaxKind::SELECTED_NAME {
                    self.check(&prefix, None);
                }
                let args = self.args(list.as_ref());
                for arg in &args {
                    if let Some(actual) = &arg.actual {
                        self.check(actual, None);
                    }
                }
                if let [arg] = args.as_slice()
                    && let Some(actual) = &arg.actual
                    && !self.convertible(&arg.val, t.ty)
                {
                    let message = format!(
                        "cannot convert {} to `{}`",
                        self.describe(&arg.val),
                        self.ty_name(t.ty, t.shown)
                    );
                    self.error(codes::TYPE_MISMATCH, trimmed_range(actual), message);
                }
                let val = Val::Types(vec![t]);
                self.expect(node, &val, exp);
            }
            Callee::Value(prefix_val) => {
                self.check(&prefix, None);
                self.check_indexes(&prefix_val, list.as_ref());
                let val = self.infer(node);
                self.expect(node, &val, exp);
            }
            Callee::Attribute => {
                self.check(&prefix, None);
                let args = self.args(list.as_ref());
                // `t'image(x)`, `t'pos(x)`, ...: the argument is a `t`.
                let attribute = attribute_designator(&prefix);
                let arg_exp = match attribute.as_deref() {
                    Some("image" | "pos" | "succ" | "pred" | "leftof" | "rightof") => prefix
                        .children()
                        .next()
                        .and_then(|p| self.type_mark_of(&p))
                        .map(Exp::from),
                    Some("value") => self.std_ty(self.std.string).map(Exp::from),
                    _ => None,
                };
                for arg in &args {
                    if let Some(actual) = &arg.actual {
                        self.check(actual, arg_exp);
                    }
                }
                let val = self.infer(node);
                self.expect(node, &val, exp);
            }
            Callee::Other => {
                for arg in self.args(list.as_ref()) {
                    if let Some(actual) = &arg.actual {
                        self.check(actual, None);
                    }
                }
            }
        }
    }

    /// Whether a type conversion from `val` to `to` is legal: between
    /// numeric types, between closely related arrays, or to the same type.
    fn convertible(&self, val: &Val, to: Ty) -> bool {
        let Val::Types(types) = val else {
            return true;
        };
        if !self.definite(val) {
            return true;
        }
        types.iter().any(|t| self.closely_related(t.ty, to, 0))
    }

    fn closely_related(&self, a: Ty, b: Ty, depth: u32) -> bool {
        if self.ty_compat(a, b) || depth > 8 {
            return true;
        }
        let numeric = |t: Ty| self.is_integer(t) != Some(false) || self.is_real(t) != Some(false);
        let (Some(x), Some(y)) = (self.def_of(a), self.def_of(b)) else {
            return true;
        };
        match (x, y) {
            (
                TypeDef::Integer | TypeDef::Real | TypeDef::Scalar,
                TypeDef::Integer | TypeDef::Real | TypeDef::Scalar,
            ) => numeric(a) && numeric(b),
            (
                TypeDef::Array {
                    indexes: i1,
                    element: e1,
                },
                TypeDef::Array {
                    indexes: i2,
                    element: e2,
                },
            ) => {
                i1.len() == i2.len()
                    && match (
                        e1.and_then(|e| self.mark_type(e)),
                        e2.and_then(|e| self.mark_type(e)),
                    ) {
                        (Some(e1), Some(e2)) => self.closely_related(e1.ty, e2.ty, depth + 1),
                        _ => true,
                    }
            }
            _ => false,
        }
    }

    fn def_of(&self, ty: Ty) -> Option<&'a TypeDef> {
        match ty {
            Ty::UInt => Some(&TypeDef::Integer),
            Ty::UReal => Some(&TypeDef::Real),
            Ty::Decl(_) => self.def(ty),
        }
    }

    /// The indexes of an indexed name or the range of a slice.
    fn check_indexes(&mut self, prefix_val: &Val, list: Option<&SyntaxNode>) {
        let Some(list) = list else { return };
        let array = self.indexed_array(prefix_val, Some(list));
        let elements: Vec<SyntaxNode> = list
            .children()
            .filter(|n| n.kind() == SyntaxKind::ASSOCIATION_ELEMENT)
            .collect();
        let indexes = match array.and_then(|(t, _)| self.def(t.ty)) {
            Some(TypeDef::Array { indexes, .. }) => indexes.clone(),
            _ => Vec::new(),
        };
        for (i, element) in elements.iter().enumerate() {
            let Some(actual) = element.children().last() else {
                continue;
            };
            let exp = if indexes.len() == elements.len() || array.is_some_and(|(_, s)| s) {
                indexes
                    .get(i)
                    .copied()
                    .flatten()
                    .and_then(|ix| self.mark_type(ix))
                    .map(Exp::from)
            } else {
                None
            };
            match actual.kind() {
                SyntaxKind::RANGE_EXPR => self.check(&actual, exp),
                _ if self.is_range(&actual) => self.check(&actual, None),
                _ => self.check(&actual, exp),
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn check_subprogram_call(
        &mut self,
        node: &SyntaxNode,
        prefix: &SyntaxNode,
        list: Option<&SyntaxNode>,
        candidates: Vec<DeclId>,
        token: Option<SyntaxToken>,
        add_ref: bool,
        exp: Option<Exp>,
        procedure: bool,
    ) {
        if prefix.kind() == SyntaxKind::SELECTED_NAME
            && let Some(p) = prefix.children().next()
        {
            self.check(&p, None);
        }
        let args = self.args(list);
        let mut rejects = Vec::new();
        let mut viable: Vec<(DeclId, Vec<(usize, DeclId)>)> = Vec::new();
        let mut indexed = false;
        for &c in &candidates {
            // `f(1)` may also call `f` without arguments and index its result.
            if !procedure && self.indexable_result(c) {
                indexed = true;
            }
            match self.match_call(c, &args) {
                Ok(pairs) => viable.push((c, pairs)),
                Err(reject) => rejects.push((c, reject)),
            }
        }
        let check_args_loosely = |me: &mut Self| {
            for arg in &args {
                if let Some(actual) = &arg.actual {
                    me.check(actual, None);
                }
            }
        };
        if viable.is_empty() {
            check_args_loosely(self);
            if !indexed {
                self.report_rejects(node, prefix, &args, &rejects);
            }
            if add_ref && let Some(token) = &token {
                self.added.push(Reference {
                    range: token.text_range(),
                    targets: candidates.iter().map(|&c| Target::Decl(c)).collect(),
                    access: Access::Other,
                });
            }
            return;
        }
        // Keep the functions whose result fits the context.
        if let Some(exp) = exp
            && !procedure
        {
            let fitting: Vec<(DeclId, Vec<(usize, DeclId)>)> = viable
                .iter()
                .filter(|(c, _)| {
                    self.return_type(*c)
                        .is_none_or(|t| self.ty_compat(t.ty, exp.ty))
                })
                .cloned()
                .collect();
            if fitting.is_empty() && !indexed {
                check_args_loosely(self);
                let val = self.rets(viable.iter().map(|(c, _)| self.return_type(*c)));
                self.expect(node, &val, Some(exp));
                return;
            }
            if !fitting.is_empty() {
                viable = fitting;
            }
        }
        let chosen: Vec<DeclId> = viable.iter().map(|(c, _)| *c).collect();
        if let Some(token) = &token {
            if add_ref {
                self.added.push(Reference {
                    range: token.text_range(),
                    targets: chosen.iter().map(|&c| Target::Decl(c)).collect(),
                    access: Access::Other,
                });
            } else if !indexed {
                self.narrow(token, &chosen);
            }
        }
        // With one candidate left, its parameter types guide the arguments.
        let unique = self.unique_candidate(&viable);
        let mut expected: Vec<Option<Exp>> = vec![None; args.len()];
        if let Some((_, pairs)) = unique {
            for &(a, p) in pairs {
                expected[a] = self.decl_type(p).map(Exp::from);
            }
        }
        for (arg, exp) in args.iter().zip(expected) {
            if let Some(actual) = &arg.actual {
                self.check(actual, exp);
            }
        }
    }

    /// The single candidate left, counting homographs (a subprogram
    /// declared in a package and again in its body) as one.
    fn unique_candidate<'v>(
        &self,
        viable: &'v [(DeclId, Vec<(usize, DeclId)>)],
    ) -> Option<&'v (DeclId, Vec<(usize, DeclId)>)> {
        let first = viable.first()?;
        let signature = |(c, pairs): &(DeclId, Vec<(usize, DeclId)>)| {
            let params: Vec<Option<Ty>> = pairs
                .iter()
                .map(|(_, p)| self.decl_type(*p).map(|t| t.ty))
                .collect();
            (params, self.return_type(*c).map(|t| t.ty))
        };
        let first_sig = signature(first);
        viable
            .iter()
            .all(|v| signature(v) == first_sig)
            .then_some(first)
    }

    pub(super) fn report_rejects(
        &mut self,
        node: &SyntaxNode,
        prefix: &SyntaxNode,
        args: &[Arg],
        rejects: &[(DeclId, Reject)],
    ) {
        let range = mark_token(prefix).map_or(trimmed_range(node), |t| t.text_range());
        let name = mark_token(prefix).map_or_else(String::new, |t| t.text().to_owned());
        if let [(c, reject)] = rejects {
            // One subprogram: say exactly what is wrong.
            let what = self.decl(*c).map_or("subprogram", |d| d.kind.describe());
            match reject {
                Reject::TooMany(n) => self.error(
                    codes::NO_MATCHING_OVERLOAD,
                    range,
                    format!(
                        "too many arguments: {what} `{name}` takes {n}, not {}",
                        args.len()
                    ),
                ),
                Reject::NoParam(a) => {
                    let p = match &args[*a].formal {
                        Formal::Named(p) => p.clone(),
                        _ => String::new(),
                    };
                    self.error(
                        codes::NO_MATCHING_OVERLOAD,
                        range,
                        format!("{what} `{name}` has no parameter `{p}`"),
                    )
                }
                Reject::Twice(p) => {
                    let p = self.decl(*p).map_or("", |d| d.spelling.as_str());
                    self.error(
                        codes::NO_MATCHING_OVERLOAD,
                        range,
                        format!("parameter `{p}` of `{name}` is associated twice"),
                    )
                }
                Reject::Missing(p) => {
                    let p = self.decl(*p).map_or("", |d| d.spelling.as_str());
                    self.error(
                        codes::NO_MATCHING_OVERLOAD,
                        range,
                        format!("missing argument for parameter `{p}` of `{name}`"),
                    )
                }
                Reject::Mismatch { arg, param } => {
                    if let Some(actual) = &args[*arg].actual {
                        let t = self.decl_type(*param);
                        if let Some(t) = t {
                            let val = args[*arg].val.clone();
                            self.mismatch(actual, &val, t.into());
                        }
                    }
                }
            }
            return;
        }
        // Several overloads: argument mismatches only when every argument
        // is typed, so the list of types says why.
        let types: Vec<String> = args
            .iter()
            .map(|a| match &a.formal {
                Formal::Named(n) => format!("{n} => {}", self.describe(&a.val)),
                _ => self.describe(&a.val),
            })
            .collect();
        let arity_only = rejects
            .iter()
            .all(|(_, r)| !matches!(r, Reject::Mismatch { .. }));
        if !arity_only && !args.iter().all(|a| self.definite(&a.val)) {
            return;
        }
        let message = if args.is_empty() {
            format!("no overload of `{name}` takes no arguments")
        } else {
            format!("no overload of `{name}` accepts ({})", types.join(", "))
        };
        self.error(codes::NO_MATCHING_OVERLOAD, range, message);
    }

    fn check_aggregate(&mut self, node: &SyntaxNode, exp: Option<Exp>) {
        let val = Val::Aggregate;
        let def = exp.and_then(|e| self.def(e.ty));
        let elements: Vec<SyntaxNode> = node
            .children()
            .filter(|n| n.kind() == SyntaxKind::ELEMENT_ASSOCIATION)
            .collect();
        let parts = |e: &SyntaxNode| -> (Option<SyntaxNode>, Option<SyntaxNode>) {
            let choices = e.children().find(|n| n.kind() == SyntaxKind::CHOICES);
            let value = e
                .children()
                .filter(|n| n.kind() != SyntaxKind::CHOICES)
                .last();
            (choices, value)
        };
        match (exp, def) {
            (Some(exp), Some(TypeDef::Array { indexes, element })) if indexes.len() == 1 => {
                let element = element.and_then(|e| self.mark_type(e));
                let index = indexes[0].and_then(|i| self.mark_type(i)).map(Exp::from);
                for e in &elements {
                    let (choices, value) = parts(e);
                    if let Some(choices) = choices {
                        for choice in choices.children() {
                            if self.is_range(&choice) && choice.kind() != SyntaxKind::RANGE_EXPR {
                                continue;
                            }
                            self.check(&choice, index);
                        }
                    }
                    let Some(value) = value else { continue };
                    let v = self.infer(&value);
                    match element {
                        Some(el) if self.compat(&v, el.ty) => self.check(&value, Some(el.into())),
                        Some(el) => {
                            if self.compat(&v, exp.ty) {
                                self.check(&value, Some(exp));
                            } else {
                                self.check(&value, None);
                                self.expect(&value, &v, Some(el.into()));
                            }
                        }
                        None => self.check(&value, None),
                    }
                }
            }
            (Some(exp), Some(TypeDef::Record(fields))) => {
                let mut position = 0;
                for e in &elements {
                    let (choices, value) = parts(e);
                    let field = match &choices {
                        None => {
                            position += 1;
                            fields.get(position - 1).map(|f| Some(f.clone()))
                        }
                        Some(choices) => {
                            let names: Vec<SyntaxNode> = choices.children().collect();
                            match names.as_slice() {
                                [n] if n.kind() == SyntaxKind::NAME => {
                                    let token = mark_token(n);
                                    let key = token
                                        .as_ref()
                                        .map(|t| identifier_key(t.text()))
                                        .unwrap_or_default();
                                    match fields.iter().find(|f| f.name == key) {
                                        Some(f) => {
                                            if let Some(token) = &token {
                                                self.element_ref(token, exp.ty, key.clone());
                                            }
                                            Some(Some(f.clone()))
                                        }
                                        None => {
                                            if let Some(token) = token {
                                                let message = format!(
                                                    "`{}` is not an element of `{}`",
                                                    token.text(),
                                                    self.ty_name(exp.ty, exp.shown)
                                                );
                                                self.error(
                                                    codes::UNRESOLVED_NAME,
                                                    token.text_range(),
                                                    message,
                                                );
                                            }
                                            Some(None)
                                        }
                                    }
                                }
                                _ => Some(None),
                            }
                        }
                    };
                    let Some(value) = value else { continue };
                    let exp = field
                        .flatten()
                        .and_then(|f| f.subtype)
                        .and_then(|s| self.mark_type(s))
                        .map(Exp::from);
                    self.check(&value, exp);
                }
            }
            _ => {
                for e in &elements {
                    let (choices, value) = parts(e);
                    if let Some(choices) = choices
                        && exp.is_none()
                    {
                        // Record element names are not expressions.
                        for choice in choices.children() {
                            if choice.kind() != SyntaxKind::NAME {
                                self.check(&choice, None);
                            }
                        }
                    }
                    if let Some(value) = value {
                        self.check(&value, None);
                    }
                }
                self.expect(node, &val, exp);
            }
        }
    }
}

//! Statements and declarations: what each expects of its expressions.

use rowan::NodeOrToken;
use vfl_syntax::{SyntaxKind, SyntaxNode};

use super::{
    Callee, Checker, Exp, Reject, Ty, TyS, Val, attribute_designator, is_expression, node_after,
    single_type,
};
use crate::decl::{Access, DeclId, DeclKind, Mode, Reference, Target, TypeDef};
use crate::resolver::mark_token;
use crate::syntax::{designator, has_token};

impl<'a> Checker<'a> {
    // ---- Statements and declarations ------------------------------------------

    pub(super) fn walk(&mut self, node: &SyntaxNode) {
        use SyntaxKind::*;
        match node.kind() {
            SIGNAL_ASSIGN_STMT | VARIABLE_ASSIGN_STMT | SELECTED_ASSIGN_STMT => {
                self.assignment(node)
            }
            IF_STMT | IF_GENERATE_STMT => {
                let mut condition_next = false;
                for child in node.children_with_tokens() {
                    match child {
                        NodeOrToken::Token(t) if t.kind().is_trivia() => {}
                        NodeOrToken::Token(t) => {
                            condition_next = matches!(t.kind(), IF_KW | ELSIF_KW)
                        }
                        NodeOrToken::Node(n) => {
                            if condition_next && n.kind() != LABEL {
                                condition_next = false;
                                self.condition(&n);
                            } else {
                                self.walk(&n);
                            }
                        }
                    }
                }
            }
            CASE_STMT | CASE_GENERATE_STMT => {
                let mut selector = None;
                for child in node.children() {
                    match child.kind() {
                        LABEL => {}
                        CASE_ALTERNATIVE => {
                            for inner in child.children() {
                                if inner.kind() == CHOICES {
                                    for choice in inner.children() {
                                        if self.is_range(&choice) && choice.kind() != RANGE_EXPR {
                                            continue;
                                        }
                                        self.check(&choice, selector);
                                    }
                                } else {
                                    self.walk(&inner);
                                }
                            }
                        }
                        _ if selector.is_none() => {
                            self.check(&child, None);
                            let val = self.infer(&child);
                            selector = single_type(&val).map(Exp::from);
                        }
                        _ => self.walk(&child),
                    }
                }
            }
            LOOP_STMT => {
                for child in node.children() {
                    match child.kind() {
                        WHILE_SCHEME => {
                            if let Some(c) = child.children().next() {
                                self.condition(&c);
                            }
                        }
                        FOR_SCHEME => self.for_scheme(&child, &child),
                        _ => self.walk(&child),
                    }
                }
            }
            FOR_GENERATE_STMT => {
                for child in node.children() {
                    if child.kind() != GENERATE_BODY && child.kind() != LABEL {
                        self.for_range(node, &child);
                    } else {
                        self.walk(&child);
                    }
                }
            }
            EXIT_STMT | NEXT_STMT => {
                for child in node.children() {
                    if child.kind() != LABEL {
                        self.condition(&child);
                    }
                }
            }
            WAIT_STMT => {
                let mut clause = None;
                for child in node.children_with_tokens() {
                    match child {
                        NodeOrToken::Token(t) => {
                            if matches!(t.kind(), UNTIL_KW | FOR_KW | ON_KW) {
                                clause = Some(t.kind());
                            }
                        }
                        NodeOrToken::Node(n) => match clause {
                            Some(UNTIL_KW) => self.condition(&n),
                            Some(FOR_KW) => {
                                let time = self.std_ty(self.std.time).map(Exp::from);
                                self.check(&n, time)
                            }
                            _ => {}
                        },
                    }
                }
            }
            ASSERT_STMT | REPORT_STMT => {
                let mut clause = if node.kind() == ASSERT_STMT {
                    Some(ASSERT_KW)
                } else {
                    Some(REPORT_KW)
                };
                for child in node.children_with_tokens() {
                    match child {
                        NodeOrToken::Token(t) => {
                            if matches!(t.kind(), REPORT_KW | SEVERITY_KW) {
                                clause = Some(t.kind());
                            }
                        }
                        NodeOrToken::Node(n) if n.kind() == LABEL => {}
                        NodeOrToken::Node(n) => match clause {
                            Some(ASSERT_KW) => self.condition(&n),
                            Some(REPORT_KW) => {
                                let string = self.std_ty(self.std.string).map(Exp::from);
                                self.check(&n, string)
                            }
                            Some(SEVERITY_KW) => {
                                let severity = self.std_ty(self.std.severity_level).map(Exp::from);
                                self.check(&n, severity)
                            }
                            _ => {}
                        },
                    }
                }
            }
            RETURN_STMT => {
                let exp = self.returns.last().copied().flatten().map(Exp::from);
                for child in node.children() {
                    if child.kind() != LABEL {
                        if child.kind() == CONDITIONAL_EXPR {
                            self.conditional(&child, exp);
                        } else {
                            self.check(&child, exp);
                        }
                    }
                }
            }
            PROCEDURE_CALL_STMT => {
                for child in node.children() {
                    match child.kind() {
                        LABEL => {}
                        CALL_EXPR => self.check_call(&child, None, true),
                        NAME | SELECTED_NAME => self.procedure_without_args(&child),
                        _ => self.check(&child, None),
                    }
                }
            }
            COMPONENT_INSTANTIATION_STMT => self.instantiation(node),
            BLOCK_STMT => {
                // A guard condition sits between `block (` and `)`.
                for child in node.children() {
                    if child.kind() == LABEL {
                        continue;
                    }
                    if is_expression(child.kind()) {
                        self.condition(&child);
                    } else {
                        self.walk(&child);
                    }
                }
            }
            SIGNAL_DECL | CONSTANT_DECL | VARIABLE_DECL | INTERFACE_OBJECT_DECL => {
                self.object_default(node)
            }
            SUBPROGRAM_BODY => {
                let ret = node
                    .children()
                    .find(|n| matches!(n.kind(), FUNCTION_SPEC | PROCEDURE_SPEC))
                    .and_then(|spec| designator(&spec))
                    .and_then(|name| self.decls_at.get(&name.text_range()).copied())
                    .and_then(|id| {
                        if self.is_function(id) {
                            self.return_type(id)
                        } else {
                            None
                        }
                    });
                self.returns.push(ret);
                for child in node.children() {
                    self.walk(&child);
                }
                self.returns.pop();
            }
            ERROR => {
                self.quiet += 1;
                for child in node.children() {
                    self.walk(&child);
                }
                self.quiet -= 1;
            }
            // Not expressions, or not checked.
            SUBTYPE_INDICATION
            | TYPE_DECL
            | SUBTYPE_DECL
            | ALIAS_DECL
            | ATTRIBUTE_DECL
            | ATTRIBUTE_SPEC
            | USE_CLAUSE
            | LIBRARY_CLAUSE
            | CONTEXT_REFERENCE
            | LABEL
            | SENSITIVITY_LIST
            | PSL_DIRECTIVE
            | CONFIGURATION_SPEC
            | BINDING_INDICATION
            | BLOCK_CONFIGURATION
            | COMPONENT_CONFIGURATION
            | DISCONNECTION_SPEC
            | PACKAGE_INSTANTIATION_DECL
            | SUBPROGRAM_INSTANTIATION_DECL
            | GROUP_DECL
            | GROUP_TEMPLATE_DECL
            | MODE_VIEW_DECL
            | FILE_DECL
            | GENERIC_MAP_ASPECT
            | PORT_MAP_ASPECT
            | INTERFACE_PACKAGE_DECL
            | INTERFACE_TYPE_DECL
            | FUNCTION_SPEC
            | PROCEDURE_SPEC
            | EXTERNAL_NAME
            | SIGNATURE => {
                if matches!(node.kind(), FUNCTION_SPEC | PROCEDURE_SPEC) {
                    // Parameter defaults.
                    for child in node.descendants() {
                        if child.kind() == INTERFACE_OBJECT_DECL {
                            self.object_default(&child);
                        }
                    }
                }
            }
            kind if is_expression(kind) => self.check(node, None),
            _ => {
                for child in node.children() {
                    self.walk(&child);
                }
            }
        }
    }

    /// `p;` or `obj.p;`: a procedure call without arguments.
    fn procedure_without_args(&mut self, name: &SyntaxNode) {
        let Callee::Subprograms {
            candidates,
            token,
            add_ref,
        } = self.callee(name, true)
        else {
            self.check(name, None);
            return;
        };
        let viable: Vec<DeclId> = candidates
            .iter()
            .copied()
            .filter(|&c| self.match_call(c, &[]).is_ok())
            .collect();
        if let Some(token) = &token {
            if add_ref {
                let targets = if viable.is_empty() {
                    &candidates
                } else {
                    &viable
                };
                self.added.push(Reference {
                    range: token.text_range(),
                    targets: targets.iter().map(|&c| Target::Decl(c)).collect(),
                    access: Access::Other,
                });
            } else {
                self.narrow(token, &viable);
            }
        }
        if viable.is_empty() {
            let rejects: Vec<(DeclId, Reject)> = candidates
                .iter()
                .filter_map(|&c| self.match_call(c, &[]).err().map(|r| (c, r)))
                .collect();
            self.report_rejects(name, name, &[], &rejects);
        }
        if name.kind() == SyntaxKind::SELECTED_NAME
            && let Some(p) = name.children().next()
        {
            self.check(&p, None);
        }
    }

    /// `:= default` of an object declaration.
    fn object_default(&mut self, node: &SyntaxNode) {
        let Some(value) = node_after(node, SyntaxKind::COLON_EQ) else {
            return;
        };
        let exp = node
            .children()
            .find(|n| n.kind() == SyntaxKind::IDENT_LIST)
            .and_then(|list| crate::syntax::idents(&list).into_iter().next())
            .and_then(|t| self.decls_at.get(&t.text_range()).copied())
            .and_then(|id| self.decl_type(id))
            .map(Exp::from);
        if value.kind() == SyntaxKind::CONDITIONAL_EXPR {
            self.conditional(&value, exp);
        } else {
            self.check(&value, exp);
        }
    }

    fn assignment(&mut self, node: &SyntaxNode) {
        use SyntaxKind::*;
        let operator = if node.kind() == VARIABLE_ASSIGN_STMT {
            COLON_EQ
        } else {
            LTE
        };
        let selected = node.kind() == SELECTED_ASSIGN_STMT;
        let mut target_exp = None;
        let mut selector = None;
        let mut seen_select = !selected;
        let mut seen_operator = false;
        let mut target_done = false;
        let mut force = false;
        for child in node.children_with_tokens() {
            let n = match child {
                NodeOrToken::Token(t) => {
                    match t.kind() {
                        SELECT_KW => seen_select = true,
                        FORCE_KW => force = true,
                        k if k == operator => seen_operator = true,
                        COLON_EQ if selected => seen_operator = true,
                        _ => {}
                    }
                    continue;
                }
                NodeOrToken::Node(n) => n,
            };
            if n.kind() == LABEL {
                continue;
            }
            if !seen_select {
                // `with selector select`.
                self.check(&n, None);
                selector = single_type(&self.infer(&n)).map(Exp::from);
                continue;
            }
            if !seen_operator && !target_done {
                target_done = true;
                if n.kind() != AGGREGATE {
                    self.check(&n, None);
                    target_exp = single_type(&self.infer(&n)).map(Exp::from);
                } else {
                    self.check(&n, None);
                }
                continue;
            }
            match n.kind() {
                DELAY_MECHANISM => {
                    let time = self.std_ty(self.std.time).map(Exp::from);
                    for e in n.children() {
                        self.check(&e, time);
                    }
                }
                WAVEFORM => self.waveform(&n, target_exp),
                CONDITIONAL_WAVEFORMS => self.conditional_waveforms(&n, target_exp),
                SELECTED_WAVEFORMS => self.selected_waveforms(&n, target_exp, selector),
                CONDITIONAL_EXPR => self.conditional(&n, target_exp),
                _ => {
                    let _ = force;
                    self.check(&n, target_exp)
                }
            }
        }
    }

    fn waveform(&mut self, node: &SyntaxNode, exp: Option<Exp>) {
        let time = self.std_ty(self.std.time).map(Exp::from);
        for element in node
            .children()
            .filter(|n| n.kind() == SyntaxKind::WAVEFORM_ELEMENT)
        {
            let mut after = false;
            for child in element.children_with_tokens() {
                match child {
                    NodeOrToken::Token(t) => after |= t.kind() == SyntaxKind::AFTER_KW,
                    NodeOrToken::Node(n) => {
                        if after {
                            self.check(&n, time);
                        } else if n.kind() == SyntaxKind::LITERAL
                            && has_token(&n, SyntaxKind::NULL_KW)
                        {
                            // A null transaction.
                        } else {
                            self.check(&n, exp);
                        }
                    }
                }
            }
        }
    }

    fn conditional_waveforms(&mut self, node: &SyntaxNode, exp: Option<Exp>) {
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
                    } else if n.kind() == SyntaxKind::WAVEFORM {
                        self.waveform(&n, exp);
                    } else {
                        self.check(&n, exp);
                    }
                }
            }
        }
    }

    fn selected_waveforms(&mut self, node: &SyntaxNode, exp: Option<Exp>, selector: Option<Exp>) {
        for child in node.children() {
            match child.kind() {
                SyntaxKind::WAVEFORM => self.waveform(&child, exp),
                SyntaxKind::CHOICES => {
                    for choice in child.children() {
                        if self.is_range(&choice) && choice.kind() != SyntaxKind::RANGE_EXPR {
                            continue;
                        }
                        self.check(&choice, selector);
                    }
                }
                _ => self.check(&child, exp),
            }
        }
    }

    /// A for loop's range: its parameter gets the range's type.
    fn for_scheme(&mut self, scheme: &SyntaxNode, _loop: &SyntaxNode) {
        for child in scheme.children() {
            self.for_range(scheme, &child);
        }
    }

    fn for_range(&mut self, owner: &SyntaxNode, range: &SyntaxNode) {
        let param = designator(owner).and_then(|t| self.decls_at.get(&t.text_range()).copied());
        let ty = self.range_type(range);
        if let (Some(param), Some(ty)) = (param, ty) {
            self.local_types.insert(param, ty);
        }
        match range.kind() {
            SyntaxKind::RANGE_EXPR => self.check(range, ty.map(Exp::from)),
            SyntaxKind::ATTRIBUTE_NAME => self.check(range, None),
            _ => {}
        }
    }

    /// The type of a discrete range.
    fn range_type(&mut self, range: &SyntaxNode) -> Option<TyS> {
        match range.kind() {
            SyntaxKind::RANGE_EXPR => {
                let bounds: Vec<Val> = range.children().map(|b| self.infer(&b)).collect();
                let mut found = None;
                let mut universal = false;
                for b in &bounds {
                    match single_type(b) {
                        Some(t) if matches!(t.ty, Ty::UInt) => universal = true,
                        Some(t) if matches!(t.ty, Ty::Decl(_)) => {
                            found.get_or_insert(t);
                        }
                        _ => return None,
                    }
                }
                found.or_else(|| {
                    if universal {
                        self.std_ty(self.std.integer)
                    } else {
                        None
                    }
                })
            }
            SyntaxKind::SUBTYPE_INDICATION => range
                .children()
                .find(|n| n.kind() != SyntaxKind::RESOLUTION_INDICATION)
                .and_then(|m| self.type_mark_of(&m)),
            SyntaxKind::NAME | SyntaxKind::SELECTED_NAME => self.type_mark_of(range),
            SyntaxKind::ATTRIBUTE_NAME => {
                let attribute = attribute_designator(range)?;
                if !matches!(attribute.as_str(), "range" | "reverse_range") {
                    return None;
                }
                let prefix = range.children().next()?;
                let t = match self.type_mark_of(&prefix) {
                    Some(t) => t,
                    None => single_type(&self.infer(&prefix))?,
                };
                match self.def(t.ty)? {
                    TypeDef::Array { indexes, .. } => {
                        // `x'range(n)` is not modeled: only one dimension.
                        if indexes.len() != 1 {
                            return None;
                        }
                        indexes[0].and_then(|i| self.mark_type(i))
                    }
                    TypeDef::Enum(_) | TypeDef::Integer | TypeDef::Physical | TypeDef::Real => {
                        Some(t)
                    }
                    _ => None,
                }
            }
            _ => None,
        }
    }

    fn instantiation(&mut self, node: &SyntaxNode) {
        // The instantiated unit's generics and ports, in order.
        let unit = node.children().find(|n| {
            matches!(
                n.kind(),
                SyntaxKind::NAME | SyntaxKind::SELECTED_NAME | SyntaxKind::CALL_EXPR
            )
        });
        let region = unit
            .as_ref()
            .and_then(mark_token)
            .and_then(|t| {
                self.target_decls(&t).into_iter().find_map(|id| {
                    self.decl(id)
                        .filter(|d| matches!(d.kind, DeclKind::Entity | DeclKind::Component))
                })
            })
            .and_then(|d| d.members.clone());
        for aspect in node.children() {
            let generic = match aspect.kind() {
                SyntaxKind::GENERIC_MAP_ASPECT => true,
                SyntaxKind::PORT_MAP_ASPECT => false,
                _ => continue,
            };
            let formals: Vec<DeclId> = region
                .as_ref()
                .map(|r| {
                    r.iter()
                        .filter(|&id| {
                            self.decl(id).is_some_and(|d| match d.kind {
                                DeclKind::Port { .. } => !generic,
                                DeclKind::Generic { .. }
                                | DeclKind::GenericType
                                | DeclKind::GenericSubprogram
                                | DeclKind::GenericPackage => generic,
                                _ => false,
                            })
                        })
                        .collect()
                })
                .unwrap_or_default();
            let Some(list) = aspect
                .children()
                .find(|n| n.kind() == SyntaxKind::ASSOCIATION_LIST)
            else {
                continue;
            };
            let mut position = 0;
            for element in list
                .children()
                .filter(|n| n.kind() == SyntaxKind::ASSOCIATION_ELEMENT)
            {
                let has_arrow = has_token(&element, SyntaxKind::ARROW);
                let nodes: Vec<SyntaxNode> = element.children().collect();
                let (formal_node, actual) = if has_arrow {
                    match nodes.as_slice() {
                        [f, a] => (Some(f.clone()), Some(a.clone())),
                        [f] if node_after(&element, SyntaxKind::ARROW).is_none() => {
                            (Some(f.clone()), None)
                        }
                        [a] => (None, Some(a.clone())),
                        _ => (None, None),
                    }
                } else {
                    (None, nodes.into_iter().next())
                };
                let formal = if has_arrow {
                    formal_node
                        .as_ref()
                        .filter(|f| f.kind() == SyntaxKind::NAME)
                        .and_then(mark_token)
                        .and_then(|t| {
                            self.target_decls(&t)
                                .into_iter()
                                .find(|id| formals.contains(id))
                        })
                } else {
                    position += 1;
                    formals.get(position - 1).copied()
                };
                let Some(actual) = actual else { continue };
                let exp = formal.and_then(|f| {
                    let decl = self.decl(f)?;
                    match decl.kind {
                        DeclKind::Generic { .. } => self.decl_type(f),
                        DeclKind::Port { mode, .. } => {
                            // An output's actual may be wrapped in a
                            // conversion from the port's type.
                            if mode != Mode::In && actual.kind() == SyntaxKind::CALL_EXPR {
                                let prefix = actual.children().next()?;
                                if !matches!(self.callee(&prefix, false), Callee::Value(_)) {
                                    return None;
                                }
                            }
                            if mode == Mode::View {
                                return None;
                            }
                            self.decl_type(f)
                        }
                        _ => None,
                    }
                });
                if exp.is_none()
                    && formal.is_some_and(|f| {
                        self.decl(f).is_some_and(|d| {
                            matches!(
                                d.kind,
                                DeclKind::GenericType
                                    | DeclKind::GenericSubprogram
                                    | DeclKind::GenericPackage
                            )
                        })
                    })
                {
                    // A type, subprogram or package actual.
                    continue;
                }
                let actual = if actual.kind() == SyntaxKind::ERROR {
                    continue;
                } else {
                    actual
                };
                self.check(&actual, exp.map(Exp::from));
            }
        }
    }
}

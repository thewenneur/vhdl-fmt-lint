//! Statements: labels, assignments, loops, generates and processes.

use vfl_syntax::{SyntaxKind, SyntaxNode, SyntaxToken};

use super::{Frame, Lookup, Resolver, operator_token};
use crate::decl::{Access, DeclId, DeclKind};
use crate::diagnostic::codes;
use crate::syntax::*;

impl<'a> Resolver<'a> {
    // ---- Statements -------------------------------------------------------

    /// Records what an operator symbol may denote.
    pub(super) fn operator_ref(&mut self, node: &SyntaxNode) {
        let Some(op) = operator_token(node) else {
            return;
        };
        let key = format!("\"{}\"", op.text().to_ascii_lowercase());
        match self.lookup(&key) {
            Lookup::Found(targets) => self.record(op.text_range(), targets, Access::Other),
            Lookup::Unknown => self.opaque_ops.push(op.text_range()),
            Lookup::NotFound => {}
        }
    }

    pub(super) fn visit_children(&mut self, node: &SyntaxNode) {
        for child in node.children() {
            self.visit(&child);
        }
    }

    pub(super) fn visit_children_except(
        &mut self,
        node: &SyntaxNode,
        skip: impl Fn(&SyntaxNode) -> bool,
    ) {
        for child in node.children() {
            if !skip(&child) {
                self.visit(&child);
            }
        }
    }

    /// Declares a statement's label, if it has one.
    pub(super) fn label(&mut self, node: &SyntaxNode) -> Option<(SyntaxToken, DeclId)> {
        let label = node.children().find(|n| n.kind() == SyntaxKind::LABEL)?;
        let token = designator(&label)?;
        let id = self.declare(&token, DeclKind::Label, node);
        Some((token, id))
    }

    pub(super) fn scoped_statement(&mut self, node: &SyntaxNode) {
        let label = self.label(node);
        if let Some((_, id)) = &label {
            self.parents.push(*id);
        }
        self.frames.push(Frame::default());
        if node.kind() == SyntaxKind::BLOCK_STMT && has_token(node, SyntaxKind::LPAREN) {
            // A guarded block declares the implicit signal GUARD.
            let id = self.push_decl(None, DeclKind::Signal, node);
            let decl = &mut self.decls[id.index as usize];
            decl.name = "guard".into();
            decl.spelling = "GUARD".into();
            decl.range = first_token(node, SyntaxKind::BLOCK_KW)
                .map_or(node.text_range(), |t| t.text_range());
            self.top_mut().locals.insert("guard", id);
            self.implicit.push(id);
        }
        self.visit_children_except(node, |n| n.kind() == SyntaxKind::LABEL);
        self.pop_frame();
        if let Some((token, id)) = label {
            self.parents.pop();
            self.end_label_refs(node, &token, id);
        }
    }

    pub(super) fn assignment(&mut self, node: &SyntaxNode, operator: SyntaxKind) {
        self.label(node);
        let mut seen_operator = false;
        let mut target_done = false;
        let after_select = node.kind() == SyntaxKind::SELECTED_ASSIGN_STMT;
        let mut seen_select = false;
        for child in node.children_with_tokens() {
            match child {
                rowan::NodeOrToken::Token(t) => {
                    if t.kind() == operator {
                        seen_operator = true;
                    }
                    if t.kind() == SyntaxKind::SELECT_KW {
                        seen_select = true;
                    }
                }
                rowan::NodeOrToken::Node(n) => {
                    if n.kind() == SyntaxKind::LABEL {
                        continue;
                    }
                    let is_target =
                        !seen_operator && !target_done && (!after_select || seen_select);
                    if is_target {
                        target_done = true;
                        self.target(&n);
                    } else {
                        self.with_access(Access::Read, |s| s.visit(&n));
                    }
                }
            }
        }
    }

    /// An assignment target: the named object is written, indexes are read.
    fn target(&mut self, node: &SyntaxNode) {
        match node.kind() {
            SyntaxKind::NAME | SyntaxKind::SELECTED_NAME | SyntaxKind::CALL_EXPR => {
                self.name_expr(node, Access::Write);
            }
            SyntaxKind::AGGREGATE => {
                for element in node.children() {
                    for child in element.children() {
                        if child.kind() == SyntaxKind::CHOICES {
                            self.soft += 1;
                            self.visit(&child);
                            self.soft -= 1;
                        } else {
                            self.target(&child);
                        }
                    }
                }
            }
            SyntaxKind::PAREN_EXPR => {
                for child in node.children() {
                    self.target(&child);
                }
            }
            _ => self.with_access(Access::ReadWrite, |s| s.visit(node)),
        }
    }

    pub(super) fn exit_or_next(&mut self, node: &SyntaxNode) {
        if let Some(token) = designator(node) {
            match self.lookup(&designator_key(&token)) {
                Lookup::Found(targets) => self.record(token.text_range(), targets, Access::Other),
                Lookup::NotFound => self.error(
                    codes::UNRESOLVED_NAME,
                    token.text_range(),
                    format!("no loop labeled `{}`", token.text()),
                ),
                Lookup::Unknown => {}
            }
        }
        self.with_access(Access::Read, |s| s.visit_children(node));
    }

    pub(super) fn loop_stmt(&mut self, node: &SyntaxNode) {
        let label = self.label(node);
        self.frames.push(Frame::default());
        for child in node.children() {
            match child.kind() {
                SyntaxKind::LABEL => {}
                SyntaxKind::FOR_SCHEME => self.for_scheme(&child, DeclKind::LoopParameter),
                _ => self.visit(&child),
            }
        }
        self.pop_frame();
        if let Some((token, id)) = label {
            self.end_label_refs(node, &token, id);
        }
    }

    fn for_scheme(&mut self, node: &SyntaxNode, kind: DeclKind) {
        self.with_access(Access::Read, |s| s.visit_children(node));
        if let Some(param) = designator(node) {
            self.declare(&param, kind, node);
        }
    }

    pub(super) fn for_generate(&mut self, node: &SyntaxNode) {
        let label = self.label(node);
        if let Some((_, id)) = &label {
            self.parents.push(*id);
        }
        self.frames.push(Frame::default());
        for child in node.children() {
            if child.kind() != SyntaxKind::LABEL && child.kind() != SyntaxKind::GENERATE_BODY {
                self.with_access(Access::Read, |s| s.visit(&child));
            }
        }
        if let Some(param) = designator(node) {
            self.declare(&param, DeclKind::GenerateParameter, node);
        }
        for body in node
            .children()
            .filter(|n| n.kind() == SyntaxKind::GENERATE_BODY)
        {
            self.generate_body(&body);
        }
        self.pop_frame();
        if let Some((token, id)) = label {
            self.parents.pop();
            self.end_label_refs(node, &token, id);
        }
    }

    pub(super) fn generate_body(&mut self, body: &SyntaxNode) {
        let label = self.label(body);
        self.frames.push(Frame::default());
        self.visit_children_except(body, |n| n.kind() == SyntaxKind::LABEL);
        self.pop_frame();
        if let Some((token, id)) = label {
            self.end_label_refs(body, &token, id);
        }
    }

    pub(super) fn conditional_generate(&mut self, node: &SyntaxNode) {
        let label = self.label(node);
        if let Some((_, id)) = &label {
            self.parents.push(*id);
        }
        for child in node.children() {
            match child.kind() {
                SyntaxKind::LABEL => {}
                SyntaxKind::GENERATE_BODY => self.generate_body(&child),
                SyntaxKind::CASE_ALTERNATIVE => {
                    for inner in child.children() {
                        if inner.kind() == SyntaxKind::GENERATE_BODY {
                            self.generate_body(&inner);
                        } else {
                            self.visit(&inner);
                        }
                    }
                }
                _ => self.with_access(Access::Read, |s| s.visit(&child)),
            }
        }
        if let Some((token, id)) = label {
            self.parents.pop();
            self.end_label_refs(node, &token, id);
        }
    }

    pub(super) fn process(&mut self, node: &SyntaxNode) {
        let label = self.label(node);
        let id = match &label {
            Some((_, id)) => *id,
            None => {
                let id = self.push_decl(None, DeclKind::Label, node);
                self.decls[id.index as usize].range = first_token(node, SyntaxKind::PROCESS_KW)
                    .map_or(node.text_range(), |t| t.text_range());
                id
            }
        };
        self.parents.push(id);
        self.frames.push(Frame::default());
        let refs_start = self.refs.len();
        let mut sensitivity = None;
        for child in node.children() {
            match child.kind() {
                SyntaxKind::LABEL => {}
                SyntaxKind::SENSITIVITY_LIST => {
                    let start = self.refs.len();
                    self.with_access(Access::Read, |s| s.visit_children(&child));
                    sensitivity = Some((child.clone(), start..self.refs.len()));
                }
                _ => self.with_access(Access::Read, |s| s.visit(&child)),
            }
        }
        self.pop_frame();
        self.parents.pop();
        if let Some((list, sens_refs)) = sensitivity
            && !has_token(&list, SyntaxKind::ALL_KW)
        {
            self.check_sensitivity(node, &list, sens_refs, refs_start);
        }
        if let Some((token, id)) = label {
            self.end_label_refs(node, &token, id);
        }
    }
}

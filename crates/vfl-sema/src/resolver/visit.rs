//! Dispatch: what to do for each kind of node.

use std::sync::Arc;

use smallvec::smallvec;
use vfl_syntax::{SyntaxKind, SyntaxNode};

use super::{Frame, Interface, Lookup, Resolver};
use crate::decl::{Access, DeclKind, Reference, Region, Target};
use crate::syntax::*;

impl<'a> Resolver<'a> {
    // ---- Dispatch ---------------------------------------------------------

    pub(super) fn visit(&mut self, node: &SyntaxNode) {
        use SyntaxKind::*;
        match node.kind() {
            NAME => {
                self.simple_name(node, self.access);
            }
            SELECTED_NAME | CALL_EXPR => {
                self.name_expr(node, self.access);
            }
            ATTRIBUTE_NAME => self.attribute_name(node),
            QUALIFIED_EXPR => {
                for (i, child) in node.children().enumerate() {
                    if i == 0 {
                        self.name_expr(&child, Access::Other);
                    } else {
                        self.with_access(Access::Read, |s| s.visit(&child));
                    }
                }
            }
            BINARY_EXPR | UNARY_EXPR => {
                // A long chain (`a & b & ...`) nests to the left: walk its
                // left spine in a loop, so 16k operands don't overflow the
                // stack.
                let mut spine = vec![node.clone()];
                while let Some(left) = spine.last().and_then(|n| n.children().next())
                    && left.kind() == BINARY_EXPR
                {
                    spine.push(left);
                }
                for n in &spine {
                    self.operator_ref(n);
                }
                let (innermost, outer) = spine.split_last().expect("not empty");
                self.visit_children(innermost);
                for n in outer.iter().rev() {
                    for child in n.children().skip(1) {
                        self.visit(&child);
                    }
                }
            }
            AGGREGATE => {
                for element in node.children() {
                    for child in element.children() {
                        if child.kind() == CHOICES {
                            self.soft += 1;
                            self.visit(&child);
                            self.soft -= 1;
                        } else {
                            self.visit(&child);
                        }
                    }
                }
            }
            EXTERNAL_NAME => {
                for child in node.children() {
                    if child.kind() == SUBTYPE_INDICATION {
                        self.with_access(Access::Other, |s| s.visit(&child));
                    }
                }
            }
            SUBTYPE_INDICATION | SIGNATURE => {
                self.in_subtype += 1;
                self.with_access(Access::Other, |s| s.visit_children(node));
                self.in_subtype -= 1;
            }
            ERROR => {
                self.quiet += 1;
                self.visit_children(node);
                self.quiet -= 1;
            }
            PSL_DIRECTIVE => {}

            LIBRARY_CLAUSE => self.library_clause(node),
            USE_CLAUSE => self.use_clause(node),
            CONTEXT_REFERENCE => self.context_reference(node),

            GENERIC_CLAUSE => self.with_interface(Interface::Generic, |s| s.visit_children(node)),
            PORT_CLAUSE => self.with_interface(Interface::Port, |s| s.visit_children(node)),
            INTERFACE_OBJECT_DECL => self.interface_object(node),
            INTERFACE_TYPE_DECL => {
                if let Some(name) = designator(node) {
                    self.declare(&name, DeclKind::GenericType, node);
                }
            }
            INTERFACE_SUBPROGRAM_DECL => {
                for spec in node.children() {
                    if matches!(spec.kind(), FUNCTION_SPEC | PROCEDURE_SPEC) {
                        let saved = self.interface;
                        self.subprogram_spec(&spec, DeclKind::GenericSubprogram, node);
                        self.pop_frame();
                        self.parents.pop();
                        self.interface = saved;
                    } else {
                        self.visit(&spec);
                    }
                }
            }
            INTERFACE_PACKAGE_DECL => {
                let Some(name) = designator(node) else { return };
                let mut members = None;
                for child in node.children() {
                    match child.kind() {
                        SELECTED_NAME | NAME if members.is_none() => {
                            members = Some(match self.name_expr(&child, Access::Other) {
                                Lookup::Found(t) => self.members_of(&t),
                                _ => None,
                            });
                        }
                        GENERIC_MAP_ASPECT => {
                            let saved = self.interface;
                            self.associations(&child, None, Interface::Generic);
                            self.interface = saved;
                        }
                        _ => {}
                    }
                }
                let id = self.declare(&name, DeclKind::GenericPackage, node);
                self.decls[id.index as usize].members = Some(
                    members
                        .flatten()
                        .unwrap_or_else(|| Arc::new(Region::opaque())),
                );
            }
            GENERIC_MAP_ASPECT => self.associations(node, None, Interface::Generic),
            PORT_MAP_ASPECT => self.associations(node, None, Interface::Port),

            SIGNAL_DECL => self.object_decl(node, DeclKind::Signal),
            CONSTANT_DECL => self.object_decl(node, DeclKind::Constant),
            VARIABLE_DECL => self.object_decl(node, DeclKind::Variable),
            FILE_DECL => self.object_decl(node, DeclKind::File),
            TYPE_DECL => self.type_decl(node),
            SUBTYPE_DECL => {
                self.with_access(Access::Other, |s| s.visit_children(node));
                if let Some(name) = designator(node) {
                    let id = self.declare(&name, DeclKind::Subtype, node);
                    let indication = node.children().find(|n| n.kind() == SUBTYPE_INDICATION);
                    self.decls[id.index as usize].subtype =
                        indication.as_ref().and_then(|n| self.subtype_mark(n));
                    self.decls[id.index as usize].width =
                        indication.as_ref().and_then(|n| self.indication_width(n));
                }
            }
            ALIAS_DECL => {
                // An alias may stand for reads and writes of its object.
                self.with_access(Access::ReadWrite, |s| s.visit_children(node));
                if let Some(name) = designator(node) {
                    let id = self.declare(&name, DeclKind::Alias, node);
                    self.decls[id.index as usize].subtype = self.alias_subtype(node);
                }
            }
            ATTRIBUTE_DECL => {
                self.with_access(Access::Other, |s| s.visit_children(node));
                if let Some(name) = designator(node) {
                    self.declare(&name, DeclKind::Attribute, node);
                }
            }
            ATTRIBUTE_SPEC => {
                if let Some(name) = designator(node)
                    && let Lookup::Found(targets) = self.lookup(&designator_key(&name))
                {
                    self.record(name.text_range(), targets, Access::Other);
                }
                for child in node.children() {
                    if child.kind() == ENTITY_SPEC {
                        // `attribute a of s, t : signal is ...`: link, never report.
                        for token in child.children_with_tokens().filter_map(|e| e.into_token()) {
                            if matches!(token.kind(), IDENT | EXT_IDENT | STRING_LITERAL)
                                && let Lookup::Found(targets) = self.lookup(&designator_key(&token))
                            {
                                self.record(token.text_range(), targets, Access::Other);
                            }
                        }
                    } else {
                        self.with_access(Access::Read, |s| s.visit(&child));
                    }
                }
            }
            COMPONENT_DECL => self.component(node),
            SUBPROGRAM_DECL => {
                for spec in node.children() {
                    if matches!(spec.kind(), FUNCTION_SPEC | PROCEDURE_SPEC) {
                        let kind = Self::subprogram_kind(&spec);
                        self.subprogram_spec(&spec, kind, node);
                        self.pop_frame();
                        self.parents.pop();
                    }
                }
            }
            SUBPROGRAM_BODY => {
                let Some(spec) = node
                    .children()
                    .find(|n| matches!(n.kind(), FUNCTION_SPEC | PROCEDURE_SPEC))
                else {
                    return self.visit_children(node);
                };
                let kind = Self::subprogram_kind(&spec);
                let id = self.subprogram_spec(&spec, kind, node);
                for child in node.children() {
                    if child != spec {
                        self.with_access(Access::Read, |s| s.visit(&child));
                    }
                }
                self.pop_frame();
                self.parents.pop();
                if let (Some(id), Some(name)) = (id, designator(&spec)) {
                    // The body completes the declaration with the same
                    // profile, in a package or earlier in the region.
                    // References stay in source order: this one goes before
                    // the body's.
                    if let Some(declaration) = self.subprogram_declaration(id) {
                        let range = name.text_range();
                        let at = self
                            .refs
                            .partition_point(|r| r.range.start() < range.start());
                        self.refs.insert(
                            at,
                            Reference {
                                range,
                                targets: smallvec![Target::Decl(declaration)],
                                access: Access::Other,
                            },
                        );
                    }
                    self.end_label_refs(node, &name, id);
                }
            }
            SUBPROGRAM_INSTANTIATION_DECL => {
                self.with_access(Access::Other, |s| {
                    for child in node.children() {
                        if child.kind() == GENERIC_MAP_ASPECT {
                            s.associations(&child, None, Interface::Generic);
                        } else {
                            s.visit(&child);
                        }
                    }
                });
                if let Some(name) = designator(node) {
                    let kind = if has_token(node, PROCEDURE_KW) {
                        DeclKind::Procedure
                    } else {
                        DeclKind::Function
                    };
                    self.declare(&name, kind, node);
                }
            }
            PACKAGE_DECL | PACKAGE_BODY | PACKAGE_INSTANTIATION_DECL => self.nested_package(node),
            GROUP_TEMPLATE_DECL | GROUP_DECL => {
                if let Some(name) = designator(node) {
                    self.declare(&name, DeclKind::Group, node);
                }
            }
            MODE_VIEW_DECL => {
                if let Some(name) = designator(node) {
                    self.declare(&name, DeclKind::View, node);
                }
                for child in node.children() {
                    if child.kind() == SUBTYPE_INDICATION || child.kind() == NAME {
                        self.with_access(Access::Other, |s| s.visit(&child));
                    }
                }
            }
            CONFIGURATION_SPEC => {
                // The binding's maps may name the component's generics and ports.
                let mut frame = Frame::default();
                for child in node.children() {
                    match child.kind() {
                        BINDING_INDICATION => {
                            self.frames.push(std::mem::take(&mut frame));
                            self.binding_indication(&child);
                            self.pop_frame();
                        }
                        NAME | SELECTED_NAME => {
                            if let Lookup::Found(targets) = self.name_expr(&child, Access::Other)
                                && let Some(members) = self.members_of(&targets)
                            {
                                frame.inherited.push(members);
                            }
                        }
                        _ => {}
                    }
                }
            }
            BINDING_INDICATION
            | BLOCK_CONFIGURATION
            | COMPONENT_CONFIGURATION
            | DISCONNECTION_SPEC => {}

            PROCESS_STMT => self.process(node),
            BLOCK_STMT => self.scoped_statement(node),
            FOR_GENERATE_STMT => self.for_generate(node),
            IF_GENERATE_STMT | CASE_GENERATE_STMT => self.conditional_generate(node),
            GENERATE_BODY => self.generate_body(node),
            LOOP_STMT => self.loop_stmt(node),
            SEQUENTIAL_BLOCK_STMT => self.scoped_statement(node),
            COMPONENT_INSTANTIATION_STMT => self.instantiation(node),
            SIGNAL_ASSIGN_STMT | SELECTED_ASSIGN_STMT => self.assignment(node, LTE),
            VARIABLE_ASSIGN_STMT => self.assignment(node, COLON_EQ),
            PROCEDURE_CALL_STMT => {
                self.label(node);
                self.with_access(Access::ReadWrite, |s| {
                    s.visit_children_except(node, |n| n.kind() == LABEL)
                });
            }
            EXIT_STMT | NEXT_STMT => {
                self.label(node);
                self.exit_or_next(node);
            }
            IF_STMT | CASE_STMT | WAIT_STMT | ASSERT_STMT | REPORT_STMT | RETURN_STMT
            | NULL_STMT => {
                let label = self.label(node);
                self.with_access(Access::Read, |s| {
                    s.visit_children_except(node, |n| n.kind() == LABEL)
                });
                if let Some((token, id)) = label {
                    self.end_label_refs(node, &token, id);
                }
            }
            LABEL => {}
            _ => self.visit_children(node),
        }
    }
}

//! The formatter's rules for VHDL.

use rowan::NodeOrToken;
use vfl_config::{
    AlignConfig, AlignScope, Case, EndKeyword, EndLabel, FormatConfig, ListLayout, Separator,
};
use vfl_syntax::SyntaxKind::{self, *};
use vfl_syntax::{Standard, SyntaxError, SyntaxNode, SyntaxToken, Vhdl};

use crate::align::{Context, Kind, child_token, context_in};
use crate::elems::{Elem, apply_case};
use crate::lang::Lang;
use crate::layout::{Analysis, Line};

impl Lang for Vhdl {
    type Std<'a> = Standard;

    const WHITESPACE: SyntaxKind = WHITESPACE;
    const LPAREN: SyntaxKind = LPAREN;
    const RPAREN: SyntaxKind = RPAREN;
    const SEMI: SyntaxKind = SEMI;

    #[inline]
    fn parse(text: &str, std: Standard) -> (SyntaxNode, Vec<SyntaxError>) {
        let parsed = vfl_syntax::parse(text, std);
        (parsed.syntax(), parsed.errors().to_vec())
    }

    #[inline]
    fn lex(text: &str, std: Standard) -> Vec<(SyntaxKind, usize)> {
        vfl_syntax::lex(text, std)
            .0
            .into_iter()
            .map(|t| (t.kind, usize::from(t.len)))
            .collect()
    }

    #[inline]
    fn is_item_kind(kind: SyntaxKind) -> bool {
        is_item_kind(kind)
    }

    #[inline(always)]
    fn is_container(kind: SyntaxKind) -> bool {
        is_container(kind)
    }

    #[inline]
    fn is_flat_container(kind: SyntaxKind) -> bool {
        matches!(kind, SOURCE_FILE | DESIGN_UNIT | CONTEXT_CLAUSE)
    }

    #[inline]
    fn is_section_keyword(kind: SyntaxKind, parent: SyntaxKind) -> bool {
        matches!(kind, BEGIN_KW | END_KW | ELSE_KW | ELSIF_KW) && is_container(parent)
    }

    #[inline]
    fn is_comment(kind: SyntaxKind) -> bool {
        is_comment(kind)
    }

    #[inline]
    fn ends_line(kind: SyntaxKind) -> bool {
        matches!(kind, LINE_COMMENT | TOOL_DIRECTIVE)
    }

    #[inline]
    fn is_chunk(kind: SyntaxKind) -> bool {
        kind == PSL_DIRECTIVE
    }

    #[inline(always)]
    fn case_of(kind: SyntaxKind, config: &FormatConfig) -> Case {
        if kind.is_keyword() {
            config.keyword_case
        } else if kind == IDENT {
            config.identifier_case
        } else {
            Case::Preserve
        }
    }

    #[inline(always)]
    fn space_between(prev: &Elem<Vhdl>, cur: &Elem<Vhdl>, config: &FormatConfig) -> bool {
        crate::spacing::space_between(prev, cur, config)
    }

    #[inline(always)]
    fn ends_label(elem: &Elem<Vhdl>) -> bool {
        let label = elem.parent.as_ref().filter(|l| l.kind() == LABEL);
        elem.is_token(COLON)
            && label.is_some_and(|l| l.parent().and_then(|s| s.first_child()).as_ref() == Some(l))
    }

    #[inline]
    fn is_alternative(kind: SyntaxKind) -> bool {
        kind == CASE_ALTERNATIVE
    }

    #[inline]
    fn is_simple_statement(kind: SyntaxKind) -> bool {
        is_simple_statement(kind)
    }

    #[inline]
    fn is_hanging(kind: SyntaxKind) -> bool {
        matches!(
            kind,
            CONDITIONAL_WAVEFORMS | SELECTED_WAVEFORMS | CONDITIONAL_EXPR
        )
    }

    #[inline(always)]
    fn opens_body(token: &SyntaxToken, parent: &SyntaxNode) -> bool {
        match token.kind() {
            RECORD_KW | PROTECTED_KW => {
                matches!(
                    parent.kind(),
                    RECORD_TYPE_DEF | PROTECTED_TYPE_DEF | PROTECTED_TYPE_BODY
                ) && parent.first_token().as_ref() == Some(token)
            }
            UNITS_KW => {
                parent.kind() == PHYSICAL_TYPE_DEF
                    && parent
                        .children_with_tokens()
                        .find(|e| e.kind() == UNITS_KW)
                        .and_then(NodeOrToken::into_token)
                        .as_ref()
                        == Some(token)
            }
            _ => false,
        }
    }

    #[inline(always)]
    fn opens_list(
        token: &SyntaxToken,
        parent: &SyntaxNode,
        config: &FormatConfig,
    ) -> Option<ListLayout> {
        (is_list(parent.kind()) && parent.first_token().as_ref() == Some(token))
            .then(|| list_layout(parent, config))
    }

    #[inline]
    fn is_separator(kind: SyntaxKind) -> bool {
        matches!(kind, COMMA | SEMI)
    }

    #[inline]
    fn is_list(kind: SyntaxKind) -> bool {
        is_list(kind)
    }

    #[inline]
    fn separator(list: &SyntaxNode, config: &FormatConfig) -> Separator {
        match list.kind() {
            INTERFACE_LIST => config.interface_separator,
            _ => config.association_separator,
        }
    }

    #[inline]
    fn edit_end_clauses(elems: Vec<Elem<Vhdl>>, config: &FormatConfig) -> Vec<Elem<Vhdl>> {
        edit_end_clauses(elems, config)
    }

    #[inline]
    fn anchors_of(
        node: &SyntaxNode,
        elems: &[Elem<Vhdl>],
        an: &Analysis<Vhdl>,
        line: &Line,
        config: &AlignConfig,
        add: &mut impl FnMut(Kind, Context, usize),
    ) -> bool {
        anchors_of(node, elems, an, line, config, add)
    }

    #[inline]
    fn is_align_block(kind: SyntaxKind) -> bool {
        matches!(
            kind,
            ENTITY_DECL
                | ARCHITECTURE_BODY
                | PACKAGE_DECL
                | PACKAGE_BODY
                | CONFIGURATION_DECL
                | INTERFACE_LIST
                | COMPONENT_DECL
                | SUBPROGRAM_BODY
                | PROCESS_STMT
                | BLOCK_STMT
                | GENERATE_BODY
                | RECORD_TYPE_DEF
                | PROTECTED_TYPE_DEF
                | PROTECTED_TYPE_BODY
                | MODE_VIEW_DECL
                | SEQUENTIAL_BLOCK_STMT
        )
    }

    #[inline]
    fn inline_anchor(elem: &Elem<Vhdl>, config: &AlignConfig) -> Option<(Kind, SyntaxNode)> {
        if !config.when_clause || !elem.is_token(WHEN_KW) {
            return None;
        }
        let parent = elem.parent.as_ref().filter(|p| {
            matches!(
                p.kind(),
                CONDITIONAL_WAVEFORMS | SELECTED_WAVEFORMS | CONDITIONAL_EXPR
            )
        })?;
        Some((Kind::When, parent.clone()))
    }

    #[inline]
    fn comment_body(elem: &Elem<Vhdl>) -> Option<&str> {
        if elem.kind != LINE_COMMENT {
            return None;
        }
        elem.text.strip_prefix("--")
    }

    #[inline(always)]
    fn normalize(kind: SyntaxKind, text: &str) -> String {
        if kind.is_keyword() || kind == IDENT {
            text.to_ascii_lowercase()
        } else {
            text.to_owned()
        }
    }

    #[inline(always)]
    fn starts_end_tail(token: &SyntaxToken) -> bool {
        token.kind() == END_KW && token.parent().is_some_and(|p| is_container(p.kind()))
    }

    #[inline]
    fn lexical_end_tail(kind: SyntaxKind) -> bool {
        kind == END_KW
    }

    #[inline]
    fn cuts_inside(node: &SyntaxNode, starts_line: &dyn Fn(Option<SyntaxToken>) -> bool) -> bool {
        let shifted = match node.kind() {
            RECORD_TYPE_DEF | PROTECTED_TYPE_DEF | PROTECTED_TYPE_BODY => {
                starts_line(node.first_token())
            }
            PHYSICAL_TYPE_DEF => starts_line(
                node.children_with_tokens()
                    .filter_map(NodeOrToken::into_token)
                    .find(|t| t.kind() == UNITS_KW),
            ),
            _ => false,
        };
        // A type's declaration holds its record or protected body.
        (is_container(node.kind())
            || Self::is_flat_container(node.kind())
            || node.kind() == TYPE_DECL)
            && !shifted
    }
}

/// Nodes that start their own line: statements, declarations, clauses,
/// design units. They are only items inside a [`container`](is_container).
fn is_item_kind(kind: SyntaxKind) -> bool {
    matches!(
        kind,
        DESIGN_UNIT
            | LIBRARY_CLAUSE
            | USE_CLAUSE
            | CONTEXT_REFERENCE
            | ENTITY_DECL
            | ARCHITECTURE_BODY
            | PACKAGE_DECL
            | PACKAGE_BODY
            | PACKAGE_INSTANTIATION_DECL
            | CONTEXT_DECL
            | CONFIGURATION_DECL
            | BLOCK_CONFIGURATION
            | COMPONENT_CONFIGURATION
            | BINDING_INDICATION
            | CONFIGURATION_SPEC
            | GENERIC_CLAUSE
            | PORT_CLAUSE
            | GENERIC_MAP_ASPECT
            | PORT_MAP_ASPECT
            | TYPE_DECL
            | SUBTYPE_DECL
            | CONSTANT_DECL
            | SIGNAL_DECL
            | VARIABLE_DECL
            | FILE_DECL
            | ALIAS_DECL
            | ATTRIBUTE_DECL
            | ATTRIBUTE_SPEC
            | COMPONENT_DECL
            | SUBPROGRAM_DECL
            | SUBPROGRAM_BODY
            | SUBPROGRAM_INSTANTIATION_DECL
            | GROUP_TEMPLATE_DECL
            | GROUP_DECL
            | DISCONNECTION_SPEC
            | MODE_VIEW_DECL
            | MODE_VIEW_ELEMENT
            | PSL_DIRECTIVE
            | ELEMENT_DECL
            | UNIT_DECL
            | PROCESS_STMT
            | BLOCK_STMT
            | COMPONENT_INSTANTIATION_STMT
            | FOR_GENERATE_STMT
            | IF_GENERATE_STMT
            | CASE_GENERATE_STMT
            | SIGNAL_ASSIGN_STMT
            | SELECTED_ASSIGN_STMT
            | VARIABLE_ASSIGN_STMT
            | PROCEDURE_CALL_STMT
            | ASSERT_STMT
            | WAIT_STMT
            | REPORT_STMT
            | IF_STMT
            | CASE_STMT
            | LOOP_STMT
            | NEXT_STMT
            | EXIT_STMT
            | RETURN_STMT
            | NULL_STMT
            | SEQUENTIAL_BLOCK_STMT
            | CASE_ALTERNATIVE
    )
}

/// Nodes whose items are indented one level deeper than the node itself.
fn is_container(kind: SyntaxKind) -> bool {
    matches!(
        kind,
        ENTITY_DECL
            | ARCHITECTURE_BODY
            | PACKAGE_DECL
            | PACKAGE_BODY
            | PACKAGE_INSTANTIATION_DECL
            | CONTEXT_DECL
            | CONFIGURATION_DECL
            | BLOCK_CONFIGURATION
            | COMPONENT_CONFIGURATION
            | BINDING_INDICATION
            | COMPONENT_DECL
            | SUBPROGRAM_BODY
            | SUBPROGRAM_INSTANTIATION_DECL
            | PROCESS_STMT
            | BLOCK_STMT
            | SEQUENTIAL_BLOCK_STMT
            | COMPONENT_INSTANTIATION_STMT
            | IF_STMT
            | CASE_STMT
            | CASE_ALTERNATIVE
            | LOOP_STMT
            | FOR_GENERATE_STMT
            | IF_GENERATE_STMT
            | CASE_GENERATE_STMT
            | GENERATE_BODY
            | RECORD_TYPE_DEF
            | PHYSICAL_TYPE_DEF
            | PROTECTED_TYPE_DEF
            | PROTECTED_TYPE_BODY
            | MODE_VIEW_DECL
    )
}

/// Parenthesized lists the formatter can lay out one element per line.
fn is_list(kind: SyntaxKind) -> bool {
    matches!(
        kind,
        INTERFACE_LIST | ASSOCIATION_LIST | AGGREGATE | ENUM_TYPE_DEF
    )
}

/// Line and block comments, and VHDL-2019 tool directives.
fn is_comment(kind: SyntaxKind) -> bool {
    matches!(kind, LINE_COMMENT | BLOCK_COMMENT | TOOL_DIRECTIVE)
}

fn is_simple_statement(kind: SyntaxKind) -> bool {
    matches!(
        kind,
        SIGNAL_ASSIGN_STMT
            | VARIABLE_ASSIGN_STMT
            | PROCEDURE_CALL_STMT
            | NULL_STMT
            | NEXT_STMT
            | EXIT_STMT
            | RETURN_STMT
            | REPORT_STMT
            | ASSERT_STMT
            | WAIT_STMT
    )
}

fn list_layout(list: &SyntaxNode, config: &FormatConfig) -> ListLayout {
    let parent = list.parent();
    let parent_kind = parent.as_ref().map(SyntaxNode::kind);
    let grandparent_kind = parent.as_ref().and_then(|p| p.parent()).map(|g| g.kind());
    match (list.kind(), parent_kind) {
        (INTERFACE_LIST, Some(PORT_CLAUSE | GENERIC_CLAUSE))
            if !matches!(grandparent_kind, Some(FUNCTION_SPEC | PROCEDURE_SPEC)) =>
        {
            config.interface_list
        }
        (ASSOCIATION_LIST, Some(GENERIC_MAP_ASPECT | PORT_MAP_ASPECT | PARAMETER_MAP_ASPECT)) => {
            config.association_list
        }
        _ => ListLayout::Preserve,
    }
}

/// What may follow `end` for a construct: the optional keywords that can be
/// added or dropped, and the name that can be repeated.
fn end_info(container: &SyntaxNode) -> Option<(Vec<SyntaxKind>, Option<SyntaxToken>)> {
    let first_ident = |node: &SyntaxNode| {
        node.children_with_tokens()
            .filter_map(NodeOrToken::into_token)
            .find(|t| matches!(t.kind(), IDENT | EXT_IDENT))
    };
    let label = || {
        container
            .children()
            .find(|n| n.kind() == LABEL)
            .and_then(|l| l.first_token())
    };
    Some(match container.kind() {
        ENTITY_DECL => (vec![ENTITY_KW], first_ident(container)),
        ARCHITECTURE_BODY => (vec![ARCHITECTURE_KW], first_ident(container)),
        PACKAGE_DECL => (vec![PACKAGE_KW], first_ident(container)),
        PACKAGE_BODY => (vec![PACKAGE_KW, BODY_KW], first_ident(container)),
        CONTEXT_DECL => (vec![CONTEXT_KW], first_ident(container)),
        CONFIGURATION_DECL => (vec![CONFIGURATION_KW], first_ident(container)),
        COMPONENT_DECL | MODE_VIEW_DECL => (vec![], first_ident(container)),
        SUBPROGRAM_BODY => {
            let spec = container
                .children()
                .find(|n| matches!(n.kind(), FUNCTION_SPEC | PROCEDURE_SPEC))?;
            let keyword = if spec.kind() == FUNCTION_SPEC {
                FUNCTION_KW
            } else {
                PROCEDURE_KW
            };
            let name = spec
                .children_with_tokens()
                .filter_map(NodeOrToken::into_token)
                .find(|t| matches!(t.kind(), IDENT | EXT_IDENT | STRING_LITERAL));
            (vec![keyword], name)
        }
        PROCESS_STMT
        | BLOCK_STMT
        | SEQUENTIAL_BLOCK_STMT
        | IF_STMT
        | CASE_STMT
        | LOOP_STMT
        | FOR_GENERATE_STMT
        | IF_GENERATE_STMT
        | CASE_GENERATE_STMT => (vec![], label()),
        RECORD_TYPE_DEF | PHYSICAL_TYPE_DEF | PROTECTED_TYPE_DEF | PROTECTED_TYPE_BODY => {
            let decl = container.parent().filter(|p| p.kind() == TYPE_DECL)?;
            (vec![], first_ident(&decl))
        }
        _ => return None,
    })
}

/// Adds or removes the optional keywords and names after `end`, as configured.
fn edit_end_clauses(elems: Vec<Elem<Vhdl>>, config: &FormatConfig) -> Vec<Elem<Vhdl>> {
    if config.end_label == EndLabel::Preserve && config.end_keyword == EndKeyword::Preserve {
        return elems;
    }
    let mut out = Vec::with_capacity(elems.len());
    let mut i = 0;
    while i < elems.len() {
        let elem = &elems[i];
        out.push(elem.clone());
        i += 1;
        let container = match &elem.parent {
            Some(p) if elem.is_token(END_KW) && is_container(p.kind()) => p.clone(),
            _ => continue,
        };
        let Some((keywords, name)) = end_info(&container) else {
            continue;
        };
        // The tail: everything up to the `;`.
        let tail_end = (i..elems.len())
            .find(|&j| elems[j].kind == SEMI)
            .unwrap_or(elems.len());
        let tail = &elems[i..tail_end];
        if tail.iter().any(|e| e.is_comment() || e.syntax.is_none()) {
            continue;
        }
        let designator = tail
            .last()
            .filter(|e| matches!(e.kind, IDENT | EXT_IDENT | STRING_LITERAL | CHAR_LITERAL));
        let tail_keywords: Vec<SyntaxKind> = tail
            .iter()
            .take(tail.len() - usize::from(designator.is_some()))
            .map(|e| e.kind)
            .collect();
        let synthetic = |kind: SyntaxKind, text: String| Elem {
            kind,
            text,
            syntax: None,
            parent: Some(container.clone()),
            src: None,
            src_newlines: 0,
            src_col: 0,
        };
        let keyword_case = match config.keyword_case {
            Case::Preserve if elem.text.chars().any(|c| c.is_ascii_uppercase()) => Case::Upper,
            Case::Preserve => Case::Lower,
            case => case,
        };

        let mut new_tail: Vec<Elem<Vhdl>> = tail.to_vec();
        let keywords_present = !keywords.is_empty() && tail_keywords.starts_with(&keywords);
        match config.end_keyword {
            EndKeyword::Add if !keywords.is_empty() && tail_keywords.is_empty() => {
                let added = keywords.iter().map(|&kw| {
                    synthetic(kw, apply_case(kw.text().unwrap_or_default(), keyword_case))
                });
                new_tail.splice(0..0, added);
            }
            EndKeyword::Remove if keywords_present && tail_keywords.len() == keywords.len() => {
                new_tail.drain(0..keywords.len());
            }
            _ => {}
        }
        match config.end_label {
            EndLabel::Add if designator.is_none() => {
                if let Some(name) = &name {
                    let text = if name.kind() == IDENT {
                        apply_case(name.text(), config.identifier_case)
                    } else {
                        name.text().to_owned()
                    };
                    new_tail.push(synthetic(name.kind(), text));
                }
            }
            EndLabel::Remove if designator.is_some() => {
                new_tail.pop();
            }
            _ => {}
        }
        out.extend(new_tail);
        i = tail_end;
    }
    out
}

/// Adds the anchors of `node` if it is a construct that has any. Returns
/// whether it was one.
fn anchors_of(
    node: &SyntaxNode,
    elems: &[Elem<Vhdl>],
    an: &Analysis<Vhdl>,
    line: &Line,
    config: &AlignConfig,
    add: &mut impl FnMut(Kind, Context, usize),
) -> bool {
    let parent = node.parent();
    let Some(parent) = parent else {
        return false;
    };
    let ctx = context_in(&parent, config);
    match node.kind() {
        INTERFACE_OBJECT_DECL => {
            let generic = parent
                .parent()
                .is_some_and(|clause| clause.kind() == GENERIC_CLAUSE);
            let colon = child_token(node, an, &[COLON]);
            if let Some(colon) = colon {
                if (generic && config.generic_colon) || (!generic && config.port_colon) {
                    add(Kind::InterfaceColon, ctx, colon);
                }
                if !generic && config.port_mode {
                    let mode =
                        child_token(node, an, &[IN_KW, OUT_KW, INOUT_KW, BUFFER_KW, LINKAGE_KW]);
                    let before_type = mode.unwrap_or(colon);
                    if before_type + 1 < line.end && !elems[before_type + 1].is_comment() {
                        add(Kind::InterfaceType, ctx, before_type + 1);
                    }
                }
            }
            if config.interface_default
                && let Some(default) = child_token(node, an, &[COLON_EQ])
            {
                add(Kind::InterfaceDefault, ctx, default);
            }
            true
        }
        SIGNAL_DECL | CONSTANT_DECL | VARIABLE_DECL | FILE_DECL | ELEMENT_DECL | ATTRIBUTE_DECL
        | MODE_VIEW_ELEMENT => {
            if config.declaration {
                if let Some(colon) = child_token(node, an, &[COLON]) {
                    add(Kind::DeclColon, ctx, colon);
                }
                if let Some(default) = child_token(node, an, &[COLON_EQ]) {
                    add(Kind::DeclDefault, ctx, default);
                }
            }
            true
        }
        ALIAS_DECL => {
            if config.declaration {
                match child_token(node, an, &[COLON]) {
                    Some(colon) => add(Kind::DeclColon, ctx, colon),
                    None => {
                        if let Some(is) = child_token(node, an, &[IS_KW]) {
                            add(Kind::DeclIs, ctx, is);
                        }
                    }
                }
            }
            true
        }
        TYPE_DECL | SUBTYPE_DECL => {
            if config.declaration
                && let Some(is) = child_token(node, an, &[IS_KW])
            {
                add(Kind::DeclIs, ctx, is);
            }
            true
        }
        SIGNAL_ASSIGN_STMT | VARIABLE_ASSIGN_STMT => {
            if config.assignment
                && let Some(op) = child_token(node, an, &[LTE, COLON_EQ])
            {
                add(Kind::Assign, ctx, op);
            }
            true
        }
        ASSOCIATION_ELEMENT | ELEMENT_ASSOCIATION => {
            if config.association
                && let Some(arrow) = child_token(node, an, &[ARROW])
            {
                add(Kind::Arrow, ctx, arrow);
            }
            true
        }
        CASE_ALTERNATIVE => {
            // Only when a statement follows on the same line, or with
            // `scope = "block"` on every line.
            let Some(arrow) = child_token(node, an, &[ARROW]).filter(|&a| {
                config.scope == AlignScope::Block
                    || (a + 1 < line.end && !elems[a + 1].is_comment())
            }) else {
                return true;
            };
            if config.case_choice {
                add(Kind::CaseArrow, ctx, arrow);
            }
            // Aligning the statements only makes sense when the choices are.
            if config.case_choice
                && config.assignment
                && arrow + 1 < line.end
                && let Some(op) = statement_assignment(&elems[arrow + 1], an)
            {
                add(Kind::AlternativeAssign, ctx, op);
            }
            true
        }
        _ => false,
    }
}

/// The `<=` or `:=` of the assignment statement starting at `elem`.
fn statement_assignment(elem: &Elem<Vhdl>, an: &Analysis<Vhdl>) -> Option<usize> {
    let token = elem.syntax.as_ref()?.as_token()?.clone();
    let start = token.text_range().start();
    token
        .parent_ancestors()
        .take_while(|n| n.text_range().start() == start)
        .find(|n| matches!(n.kind(), SIGNAL_ASSIGN_STMT | VARIABLE_ASSIGN_STMT))
        .and_then(|stmt| child_token(&stmt, an, &[LTE, COLON_EQ]))
}

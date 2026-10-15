//! The formatter's rules for Verilog and SystemVerilog.
//!
//! Each module item and statement starts a line; the items of a module,
//! package, interface, function or task, the statements of a
//! `begin`/`end` or `fork`/`join` block, the items of a `case` and the
//! members of a struct are indented one level. `begin` stays where it is
//! written (`if (x) begin` or on a line of its own), `end`, `endcase`,
//! `endmodule` and the like start a line. A statement controlled by an
//! `if`, a loop or an event without `begin` keeps its line, or is indented
//! as a continuation.
//!
//! Port, parameter and connection lists are laid out like VHDL's interface
//! and association lists. Directives (`` `ifdef ``, `` `define ``) are
//! laid out like comments, and code in a branch that is not compiled is
//! kept as written.

use rowan::{Language, NodeOrToken};
use vfl_config::{AlignConfig, Case, FormatConfig, ListLayout, Separator};
use vfl_syntax::SyntaxError;
use vfl_verilog_syntax::SyntaxKind::{self, *};
use vfl_verilog_syntax::{Options, SyntaxNode, SyntaxToken, Verilog};

use crate::align::{Context, Kind, child_token, context_in};
use crate::elems::Elem;
use crate::lang::Lang;
use crate::layout::{Analysis, Line};

impl Lang for Verilog {
    type Std<'a> = Options<'a>;

    const WHITESPACE: SyntaxKind = WHITESPACE;
    const LPAREN: SyntaxKind = LPAREN;
    const RPAREN: SyntaxKind = RPAREN;
    const SEMI: SyntaxKind = SEMI;

    fn parse(text: &str, options: Options<'_>) -> (SyntaxNode, Vec<SyntaxError>) {
        let parsed = vfl_verilog_syntax::parse_with(text, &options);
        (parsed.syntax(), parsed.errors().to_vec())
    }

    fn lex(text: &str, options: Options<'_>) -> Vec<(SyntaxKind, usize)> {
        let mut tokens = Vec::new();
        lex_into(text, &options, &mut tokens);
        tokens
    }

    fn is_item_kind(kind: SyntaxKind) -> bool {
        matches!(
            kind,
            MODULE_DECL
                | INTERFACE_DECL
                | PROGRAM_DECL
                | PACKAGE_DECL
                | PRIMITIVE_DECL
                | CONFIG_DECL
                | CLASS_DECL
                | CHECKER_DECL
                | EXTERN_DECL
                | PACKAGE_IMPORT
                | PACKAGE_EXPORT
                | DPI_DECL
                | TIMEUNIT_DECL
                | PARAM_DECL
                | PORT_DECL
                | NET_DECL
                | DATA_DECL
                | TYPEDEF_DECL
                | GENVAR_DECL
                | FUNCTION_DECL
                | TASK_DECL
                | MODPORT_DECL
                | DEFPARAM_DECL
                | OPAQUE_DECL
                | SPECIFY_BLOCK
                | MACRO_ITEM
                | CONTINUOUS_ASSIGN
                | PROCEDURE
                | INSTANTIATION
                | GENERATE_REGION
                | GENERATE_FOR
                | GENERATE_IF
                | GENERATE_CASE
                | GENERATE_BLOCK
                | ASSERTION_ITEM
                | STRUCT_MEMBER
                | BLOCK_STMT
                | FORK_STMT
                | IF_STMT
                | CASE_STMT
                | CASE_ITEM
                | FOR_STMT
                | FOREACH_STMT
                | WHILE_STMT
                | DO_WHILE_STMT
                | REPEAT_STMT
                | FOREVER_STMT
                | BLOCKING_ASSIGN
                | NONBLOCKING_ASSIGN
                | COMPOUND_ASSIGN
                | EXPR_STMT
                | TIMED_STMT
                | WAIT_STMT
                | DISABLE_STMT
                | EVENT_TRIGGER
                | RETURN_STMT
                | BREAK_STMT
                | CONTINUE_STMT
                | NULL_STMT
                | PROCEDURAL_CONTINUOUS
                | IMMEDIATE_ASSERT
                | LOCAL_DECL
                | OPAQUE_STMT
        )
    }

    fn in_header(node: &SyntaxNode) -> bool {
        // `module m import p::*; (...);`
        node.kind() == PACKAGE_IMPORT
            && node.parent().is_some_and(|module| {
                module
                    .children_with_tokens()
                    .find(|e| e.kind() == SEMI)
                    .is_some_and(|semi| semi.text_range().start() > node.text_range().start())
            })
    }

    fn is_container(kind: SyntaxKind) -> bool {
        matches!(
            kind,
            MODULE_DECL
                | INTERFACE_DECL
                | PROGRAM_DECL
                | PACKAGE_DECL
                | FUNCTION_DECL
                | TASK_DECL
                | BLOCK_STMT
                | FORK_STMT
                | GENERATE_REGION
                | GENERATE_BLOCK
                | GENERATE_CASE
                | CASE_STMT
                | STRUCT_TYPE
        )
    }

    fn is_flat_container(kind: SyntaxKind) -> bool {
        kind == SOURCE_FILE
    }

    fn is_section_keyword(kind: SyntaxKind, parent: SyntaxKind) -> bool {
        match kind {
            END_KW | ENDMODULE_KW | ENDINTERFACE_KW | ENDPROGRAM_KW | ENDPACKAGE_KW
            | ENDFUNCTION_KW | ENDTASK_KW | ENDGENERATE_KW | ENDCASE_KW | JOIN_KW | JOIN_ANY_KW
            | JOIN_NONE_KW => Self::is_container(parent),
            RBRACE => parent == STRUCT_TYPE,
            _ => false,
        }
    }

    fn is_flush_keyword(kind: SyntaxKind, parent: SyntaxKind) -> bool {
        matches!(
            (kind, parent),
            (BEGIN_KW, BLOCK_STMT | GENERATE_BLOCK) | (FORK_KW, FORK_STMT) | (ELSE_KW, _)
        )
    }

    fn is_comment(kind: SyntaxKind) -> bool {
        matches!(kind, LINE_COMMENT | BLOCK_COMMENT | DIRECTIVE | INACTIVE)
    }

    fn ends_line(kind: SyntaxKind) -> bool {
        matches!(kind, LINE_COMMENT | DIRECTIVE)
    }

    fn starts_line(kind: SyntaxKind) -> bool {
        kind == DIRECTIVE
    }

    fn keeps_column(kind: SyntaxKind) -> bool {
        kind == DIRECTIVE
    }

    fn is_chunk(kind: SyntaxKind) -> bool {
        matches!(
            kind,
            PRIMITIVE_DECL
                | CONFIG_DECL
                | CLASS_DECL
                | CHECKER_DECL
                | OPAQUE_DECL
                | SPECIFY_BLOCK
                | OPAQUE_STMT
                | PROPERTY_EXPR
                | PATTERN
        )
    }

    fn case_of(_kind: SyntaxKind, _config: &FormatConfig) -> Case {
        // Verilog is case sensitive.
        Case::Preserve
    }

    fn space_between(prev: &Elem<Verilog>, cur: &Elem<Verilog>, config: &FormatConfig) -> bool {
        space_between(prev, cur, config)
    }

    fn stays_on_line(elems: &[Elem<Verilog>], i: usize) -> bool {
        let prev = &elems[i - 1];
        // Code that is not compiled holds its own line breaks, up to the
        // directive after it.
        elems[i].kind == INACTIVE
            || prev.kind == INACTIVE
            || (prev.kind == ATTRIBUTE && elems[i].src_newlines == 0)
    }

    fn ends_label(elem: &Elem<Verilog>) -> bool {
        let label = elem.parent.as_ref().filter(|l| l.kind() == LABEL);
        elem.is_token(COLON)
            && label.is_some_and(|l| l.parent().and_then(|s| s.first_child()).as_ref() == Some(l))
    }

    const NESTS_BODIES: bool = true;

    fn nests_body(parent: SyntaxKind, child: SyntaxKind) -> bool {
        // A `begin` block sits at its statement's level; its statements
        // are indented as the items of a container.
        matches!(
            parent,
            IF_STMT
                | ELSE_CLAUSE
                | FOR_STMT
                | FOREACH_STMT
                | WHILE_STMT
                | DO_WHILE_STMT
                | REPEAT_STMT
                | FOREVER_STMT
                | TIMED_STMT
                | WAIT_STMT
                | PROCEDURE
                | CASE_ITEM
                | GENERATE_FOR
                | GENERATE_IF
        ) && Self::is_item_kind(child)
            && !matches!(child, BLOCK_STMT | GENERATE_BLOCK | FORK_STMT)
    }

    fn is_alternative(kind: SyntaxKind) -> bool {
        kind == CASE_ITEM
    }

    fn is_simple_statement(kind: SyntaxKind) -> bool {
        matches!(
            kind,
            BLOCKING_ASSIGN
                | NONBLOCKING_ASSIGN
                | COMPOUND_ASSIGN
                | EXPR_STMT
                | NULL_STMT
                | RETURN_STMT
                | BREAK_STMT
                | CONTINUE_STMT
                | DISABLE_STMT
                | EVENT_TRIGGER
        )
    }

    fn opens_list(
        token: &SyntaxToken,
        parent: &SyntaxNode,
        config: &FormatConfig,
    ) -> Option<ListLayout> {
        let layout = match parent.kind() {
            PORT_LIST | PARAM_PORT_LIST => {
                let routine = parent
                    .parent()
                    .is_some_and(|p| matches!(p.kind(), FUNCTION_DECL | TASK_DECL));
                if routine {
                    ListLayout::Preserve
                } else {
                    config.interface_list
                }
            }
            INSTANCE | PARAM_VALUES => config.association_list,
            ARG_LIST => ListLayout::Preserve,
            _ => return None,
        };
        let first = parent
            .children_with_tokens()
            .filter_map(NodeOrToken::into_token)
            .find(|t| t.kind() == LPAREN);
        (first.as_ref() == Some(token)).then_some(layout)
    }

    fn is_separator(kind: SyntaxKind) -> bool {
        kind == COMMA
    }

    fn is_list(kind: SyntaxKind) -> bool {
        matches!(
            kind,
            PORT_LIST | PARAM_PORT_LIST | INSTANCE | PARAM_VALUES | ARG_LIST
        )
    }

    fn separator(list: &SyntaxNode, config: &FormatConfig) -> Separator {
        match list.kind() {
            PORT_LIST | PARAM_PORT_LIST => config.interface_separator,
            _ => config.association_separator,
        }
    }

    fn joins_previous_element(elem: &Elem<Verilog>) -> bool {
        // `input a, b`: a port that takes the direction and type of the one
        // before it, in a list of ports that have them.
        let Some(port) = elem.parent.as_ref().and_then(|p| p.parent()) else {
            return false;
        };
        let bare = |p: &SyntaxNode| {
            p.first_token()
                .and_then(|t| t.parent())
                .is_some_and(|n| n.kind() == NAME)
        };
        port.kind() == PORT
            && bare(&port)
            && port
                .first_token()
                .is_some_and(|t| Some(t.text_range()) == elem.src)
            && port
                .prev_sibling()
                .is_some_and(|p| p.kind() == PORT && !bare(&p))
    }

    fn is_align_block(kind: SyntaxKind) -> bool {
        matches!(
            kind,
            MODULE_DECL
                | INTERFACE_DECL
                | PROGRAM_DECL
                | PACKAGE_DECL
                | PARAM_PORT_LIST
                | PORT_LIST
                | STRUCT_TYPE
                | FUNCTION_DECL
                | TASK_DECL
                | PROCEDURE
                | GENERATE_BLOCK
        )
    }

    #[inline]
    fn anchors_of(
        node: &SyntaxNode,
        elems: &[Elem<Verilog>],
        an: &Analysis<Verilog>,
        line: &Line,
        config: &AlignConfig,
        add: &mut impl FnMut(Kind, Context, usize),
    ) -> bool {
        anchors_of(node, elems, an, line, config, add)
    }

    fn comment_body(elem: &Elem<Verilog>) -> Option<&str> {
        if elem.kind != LINE_COMMENT {
            return None;
        }
        elem.text.strip_prefix("//")
    }

    fn normalize(_kind: SyntaxKind, text: &str) -> String {
        text.to_owned()
    }

    fn cuts_inside(node: &SyntaxNode, _starts_line: &dyn Fn(Option<SyntaxToken>) -> bool) -> bool {
        Self::is_container(node.kind()) || Self::is_flat_container(node.kind())
    }
}

/// The tokens of `text`, with those of code that is not compiled: a part
/// of a file lexed alone misses the macros defined above it, so code that
/// is compiled may look like it is not. Included files are not read.
fn lex_into(text: &str, options: &Options, tokens: &mut Vec<(SyntaxKind, usize)>) {
    let options = Options {
        include: None,
        ..*options
    };
    let mut pos = 0;
    for token in vfl_verilog_syntax::lex(text, &options).tokens {
        let len = usize::from(token.len);
        let inner = &text[pos..pos + len];
        pos += len;
        if token.kind == INACTIVE && len < text.len() {
            lex_into(inner, &options, tokens);
        } else {
            tokens.push((token.kind, len));
        }
    }
}

/// Whether there was white space between two elements in the source.
fn spaced_in_source(prev: &Elem<Verilog>, cur: &Elem<Verilog>) -> bool {
    match (prev.src, cur.src) {
        (Some(p), Some(c)) => p.end() < c.start(),
        _ => true,
    }
}

/// Whether `prev` and `cur`, both operators, lex differently once
/// written together.
fn would_join(prev: &Elem<Verilog>, cur: &Elem<Verilog>) -> bool {
    let operator =
        |text: &str| !text.is_empty() && text.bytes().all(|b| b"+-*/%<>=!~&|^?:@#'.".contains(&b));
    if !operator(&prev.text) || !operator(&cur.text) {
        return false;
    }
    let joined = format!("{}{}", prev.text, cur.text);
    let lexed = vfl_verilog_syntax::lex(&joined, &Options::default());
    !matches!(&lexed.tokens[..], [a, b] if a.kind == prev.kind && b.kind == cur.kind)
}

/// Whether `elem` is the operator of a prefix expression: `~a`, `-1`, `!x`.
fn is_prefix_operator(elem: &Elem<Verilog>) -> bool {
    elem.parent.as_ref().is_some_and(|p| {
        p.kind() == UNARY_EXPR
            && p.first_token()
                .is_some_and(|t| Some(t.text_range()) == elem.src)
    })
}

/// Whether `elem` is a token of an expression between brackets: an index,
/// a range or a dimension.
fn in_brackets(elem: &Elem<Verilog>) -> bool {
    let Some(parent) = &elem.parent else {
        return false;
    };
    let mut child: Option<SyntaxNode> = None;
    for n in parent.ancestors() {
        match n.kind() {
            ARG_LIST | PAREN_EXPR | CONCAT_EXPR => return false,
            DIMENSION | VALUE_RANGE => return true,
            // `x.y[i]`: the indexed name is not between the brackets.
            INDEX_EXPR if child.is_some() && child != n.first_child() => return true,
            _ => {}
        }
        child = Some(n);
    }
    false
}

/// The `:` of a range, `+:` and `-:`: no space around them.
fn is_range_colon(elem: &Elem<Verilog>) -> bool {
    match elem.kind {
        PLUS_COLON | MINUS_COLON => true,
        COLON => matches!(
            elem.parent_kind(),
            Some(DIMENSION | INDEX_EXPR | VALUE_RANGE | MINTYPMAX_EXPR)
        ),
        _ => false,
    }
}

fn is_binary_operator(elem: &Elem<Verilog>) -> bool {
    elem.parent_kind() == Some(BINARY_EXPR) && !elem.kind.is_node() && elem.kind != IDENT
}

/// Whether a space goes between `prev` and `cur` when they share a line.
fn space_between(prev: &Elem<Verilog>, cur: &Elem<Verilog>, config: &FormatConfig) -> bool {
    if prev.kind == INACTIVE || cur.kind == INACTIVE {
        return false;
    }
    if prev.is_comment() || cur.is_comment() {
        return true;
    }
    // An escaped identifier ends at white space.
    if prev.kind == ESC_IDENT {
        return true;
    }
    // Operators written apart that would read as one: `- -a`, `~ &b`.
    if spaced_in_source(prev, cur) && would_join(prev, cur) {
        return true;
    }
    // The arguments of a macro are kept as written.
    if cur.parent_kind() == Some(MACRO_ARGS) {
        return cur.kind != LPAREN && spaced_in_source(prev, cur);
    }
    if cur.kind == SEMI && cur.parent_kind() == Some(NULL_STMT) && prev.kind == COLON {
        // `default: ;`
        return true;
    }
    if matches!(cur.kind, SEMI | COMMA | RPAREN | RBRACKET) {
        return false;
    }
    if cur.kind == RBRACE {
        return cur.parent_kind() == Some(STRUCT_TYPE);
    }
    if matches!(
        prev.kind,
        LPAREN | LBRACKET | LBRACE | DOT | HASH | HASH2 | AT | AT2 | COLON2
    ) {
        return false;
    }
    if cur.kind == COLON2 {
        return false;
    }
    if is_range_colon(prev) || is_range_colon(cur) {
        return false;
    }
    if is_prefix_operator(prev) {
        return false;
    }
    match cur.kind {
        DOT => !matches!(cur.parent_kind(), Some(FIELD_EXPR | NAME_REF)),
        TICK => {
            // `'{...}` starts a value; `t'(x)` and `t'{...}` are one.
            cur.parent.as_ref().is_some_and(|p| {
                p.kind() == ASSIGNMENT_PATTERN
                    && p.first_token()
                        .is_some_and(|t| Some(t.text_range()) == cur.src)
            })
        }
        _ if prev.kind == TICK => false,
        COLON => matches!(cur.parent_kind(), Some(CONDITIONAL_EXPR | END_LABEL)),
        PLUS2 | MINUS2 if cur.parent_kind() == Some(POSTFIX_EXPR) => false,
        LBRACKET => {
            if cur.parent_kind() == Some(INDEX_EXPR) || prev.kind == RBRACKET {
                false
            } else if prev.parent_kind() == Some(NAME) {
                // An unpacked dimension after a declared name.
                spaced_in_source(prev, cur)
            } else {
                true
            }
        }
        LBRACE => {
            let grandparent = cur
                .parent
                .as_ref()
                .and_then(|p| p.parent())
                .map(|g| g.kind());
            match (cur.parent_kind(), grandparent) {
                (Some(CONCAT_EXPR), Some(REPLICATION_EXPR)) => false,
                (Some(CONCAT_EXPR), Some(STREAMING_EXPR)) => spaced_in_source(prev, cur),
                _ => true,
            }
        }
        LPAREN => {
            if matches!(
                cur.parent_kind(),
                Some(ARG_LIST | CONNECTION | NAMED_ARG | CAST_EXPR)
            ) {
                return false;
            }
            if cur.parent_kind() == Some(PORT_LIST)
                && cur
                    .parent
                    .as_ref()
                    .and_then(|p| p.parent())
                    .is_some_and(|p| matches!(p.kind(), FUNCTION_DECL | TASK_DECL))
            {
                return false;
            }
            if matches!(
                prev.kind,
                IF_KW
                    | FOR_KW
                    | FOREACH_KW
                    | WHILE_KW
                    | CASE_KW
                    | CASEX_KW
                    | CASEZ_KW
                    | REPEAT_KW
                    | WAIT_KW
            ) {
                return config.space_after_keyword;
            }
            true
        }
        _ if (is_binary_operator(prev) || is_binary_operator(cur))
            && (in_brackets(prev) || in_brackets(cur)) =>
        {
            spaced_in_source(prev, cur)
        }
        _ => true,
    }
}

/// The element index of the first token of `node`'s first child node of
/// kind `kind`.
fn child_node(node: &SyntaxNode, an: &Analysis<Verilog>, kind: SyntaxKind) -> Option<usize> {
    node.children()
        .find(|c| c.kind() == kind)
        .and_then(|c| c.first_token())
        .and_then(|t| an.index.get(&t.text_range().start()).copied())
}

/// The anchors of a declaration: its type (after a direction or
/// `parameter`), the name it declares and its initial value.
fn declaration_anchors(
    node: &SyntaxNode,
    an: &Analysis<Verilog>,
    ctx: Context,
    (ty, name, default): (bool, bool, bool),
    add: &mut impl FnMut(Kind, Context, usize),
) {
    let keyword = child_token(
        node,
        an,
        &[
            INPUT_KW,
            OUTPUT_KW,
            INOUT_KW,
            REF_KW,
            PARAMETER_KW,
            LOCALPARAM_KW,
        ],
    );
    // A port's name is a direct child; a declaration's is in its first
    // declarator, with its initial value.
    let declarator = node.children().find(|c| c.kind() == DECLARATOR);
    let holder = declarator.as_ref().unwrap_or(node);
    let name_at = child_node(holder, an, NAME);
    if ty && keyword.is_some() {
        // Without a type, the name sits where the type would.
        if let Some(t) = child_node(node, an, DATA_TYPE).or(name_at) {
            add(Kind::PortType, ctx, t);
        }
    }
    if name && let Some(n) = name_at {
        add(Kind::Name, ctx, n);
    }
    if default && let Some(eq) = child_token(holder, an, &[EQ]) {
        add(Kind::InterfaceDefault, ctx, eq);
    }
}

fn anchors_of(
    node: &SyntaxNode,
    elems: &[Elem<Verilog>],
    an: &Analysis<Verilog>,
    line: &Line,
    config: &AlignConfig,
    add: &mut impl FnMut(Kind, Context, usize),
) -> bool {
    let Some(parent) = node.parent() else {
        return false;
    };
    let ctx = context_in(&parent, config);
    match node.kind() {
        PORT => {
            let wants = (
                config.port_mode,
                config.port_colon,
                config.interface_default,
            );
            declaration_anchors(node, an, ctx, wants, add);
            true
        }
        PARAM_DECL if parent.kind() == PARAM_PORT_LIST => {
            let wants = (
                config.port_mode,
                config.generic_colon,
                config.interface_default,
            );
            declaration_anchors(node, an, ctx, wants, add);
            true
        }
        PARAM_DECL | PORT_DECL | NET_DECL | DATA_DECL | STRUCT_MEMBER => {
            // Port declarations, parameters and other declarations each
            // line up among themselves.
            let class = match node.kind() {
                NET_DECL => DATA_DECL,
                kind => kind,
            };
            let ctx = (ctx.0, Verilog::kind_to_raw(class).0);
            let d = config.declaration;
            declaration_anchors(node, an, ctx, (d, d, d), add);
            true
        }
        CONTINUOUS_ASSIGN => {
            if config.assignment
                && let Some(op) = node
                    .children()
                    .find(|c| c.kind() == ASSIGN_EXPR)
                    .and_then(|a| child_token(&a, an, &[EQ]))
            {
                add(Kind::Assign, ctx, op);
            }
            true
        }
        BLOCKING_ASSIGN | NONBLOCKING_ASSIGN | COMPOUND_ASSIGN => {
            if config.assignment
                && let Some(op) = assignment_operator(node, an)
            {
                add(Kind::Assign, ctx, op);
            }
            true
        }
        CONNECTION => {
            if config.association
                && let Some(open) = child_token(node, an, &[LPAREN])
            {
                add(Kind::Arrow, ctx, open);
            }
            true
        }
        CASE_ITEM => {
            // Only when a statement follows on the same line.
            let Some(colon) = child_token(node, an, &[COLON])
                .filter(|&c| c + 1 < line.end && !elems[c + 1].is_comment())
            else {
                return true;
            };
            if config.case_choice {
                add(Kind::CaseArrow, ctx, colon + 1);
            }
            if config.case_choice
                && config.assignment
                && let Some(op) = statement_assignment(&elems[colon + 1], an)
            {
                add(Kind::AlternativeAssign, ctx, op);
            }
            true
        }
        _ => false,
    }
}

fn assignment_operator(node: &SyntaxNode, an: &Analysis<Verilog>) -> Option<usize> {
    node.children_with_tokens()
        .filter_map(NodeOrToken::into_token)
        .find(|t| {
            matches!(
                t.kind(),
                EQ | LTE
                    | PLUS_EQ
                    | MINUS_EQ
                    | STAR_EQ
                    | SLASH_EQ
                    | PERCENT_EQ
                    | AMP_EQ
                    | PIPE_EQ
                    | CARET_EQ
                    | SHL_EQ
                    | SHR_EQ
                    | ASHL_EQ
                    | ASHR_EQ
            )
        })
        .and_then(|t| an.index.get(&t.text_range().start()).copied())
}

/// The operator of the assignment statement starting at `elem`.
fn statement_assignment(elem: &Elem<Verilog>, an: &Analysis<Verilog>) -> Option<usize> {
    let token = elem.syntax.as_ref()?.as_token()?.clone();
    let start = token.text_range().start();
    token
        .parent_ancestors()
        .take_while(|n| n.text_range().start() == start)
        .find(|n| {
            matches!(
                n.kind(),
                BLOCKING_ASSIGN | NONBLOCKING_ASSIGN | COMPOUND_ASSIGN
            )
        })
        .and_then(|stmt| assignment_operator(&stmt, an))
}

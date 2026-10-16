//! Rules that only look at the text and the syntax tree of one file.

use regex::Regex;
use vfl_sema::{Edit, Fix};
use vfl_syntax::green_walk::{ChildTokens, descendants_where};
use vfl_syntax::{Standard, SyntaxKind, SyntaxNode, TextRange, TextSize};

use crate::Finding;

/// Context for the style rules of one file.
pub struct StyleContext<'a> {
    pub text: &'a str,
    pub root: &'a SyntaxNode,
    pub file_stem: Option<String>,
    /// The file's extension, for the name `style/file-name` suggests.
    pub extension: Option<String>,
    /// `style/line-length`'s limit.
    pub max_line_length: usize,
    /// Columns per indentation level: what a tab becomes.
    pub indent: usize,
    pub standard: Standard,
    /// `style/magic-number`'s numbers that may be written as they are.
    pub magic_allow: &'a [i64],
    /// `style/file-header`'s pattern.
    pub header: Option<&'a Regex>,
    /// `style/port-order`'s names of clock and reset ports.
    pub clocks: Option<&'a Regex>,
    pub resets: Option<&'a Regex>,
}

pub fn check(ctx: &StyleContext, enabled: impl Fn(&str) -> bool, out: &mut Vec<Finding>) {
    if enabled("style/line-length") || enabled("style/trailing-whitespace") || enabled("style/tabs")
    {
        lines(ctx.text, ctx.max_line_length, ctx.indent, &enabled, out);
    }
    // The kinds the arms below look at: only these become syntax nodes.
    let case_others = enabled("style/case-others");
    let magic = enabled("style/magic-number");
    let generic_default = enabled("style/generic-default");
    let port_order = enabled("style/port-order");
    let process_all = enabled("style/process-all") && ctx.standard >= Standard::Vhdl2008;
    let shared = enabled("style/shared-variable");
    let wanted = |kind| {
        (case_others && kind == SyntaxKind::CASE_STMT)
            || (magic && kind == SyntaxKind::LITERAL)
            || (generic_default && kind == SyntaxKind::GENERIC_CLAUSE)
            || (port_order && kind == SyntaxKind::PORT_CLAUSE)
            || (shared && kind == SyntaxKind::VARIABLE_DECL)
            || matches!(
                kind,
                SyntaxKind::INTERFACE_OBJECT_DECL
                    | SyntaxKind::USE_CLAUSE
                    | SyntaxKind::PROCESS_STMT
                    | SyntaxKind::GENERIC_MAP_ASPECT
                    | SyntaxKind::PORT_MAP_ASPECT
                    | SyntaxKind::BINARY_EXPR
            )
    };
    for node in descendants_where(ctx.root, wanted) {
        match node.kind() {
            SyntaxKind::INTERFACE_OBJECT_DECL if enabled("style/no-buffer-ports") => {
                if let Some(token) = node.child_token(|k| k == SyntaxKind::BUFFER_KW) {
                    let mut finding = Finding::new(
                        "style/no-buffer-ports",
                        token.text_range(),
                        "port of mode `buffer`; use `out` (readable since VHDL-2008) or an internal signal",
                    );
                    // Before VHDL-2008, an `out` port cannot be read.
                    if ctx.standard >= Standard::Vhdl2008 {
                        finding = finding.with_fix(Fix {
                            title: "Change the mode to `out`".into(),
                            edits: vec![Edit {
                                range: token.text_range(),
                                text: "out".into(),
                            }],
                            safe: false,
                        });
                    }
                    out.push(finding);
                }
            }
            SyntaxKind::USE_CLAUSE if enabled("style/no-std-logic-arith") => {
                for item in node.children() {
                    let text = item.to_string().to_ascii_lowercase();
                    let package = text.split('.').nth(1).unwrap_or("").trim();
                    if matches!(
                        package,
                        "std_logic_arith" | "std_logic_unsigned" | "std_logic_signed"
                    ) {
                        // `std_logic_unsigned` became `numeric_std_unsigned`
                        // in VHDL-2008; the others have `numeric_std`.
                        let standard = if package == "std_logic_unsigned"
                            && ctx.standard >= Standard::Vhdl2008
                        {
                            "numeric_std_unsigned"
                        } else {
                            "numeric_std"
                        };
                        let library = item.to_string();
                        let library = library.split('.').next().unwrap_or("ieee").trim();
                        out.push(
                            Finding::new(
                                "style/no-std-logic-arith",
                                item.text_range(),
                                format!(
                                    "`{package}` is not a standard package; use `ieee.numeric_std`"
                                ),
                            )
                            .with_fix(Fix {
                                title: format!("Use `{library}.{standard}` instead"),
                                edits: vec![Edit {
                                    range: item.text_range(),
                                    text: format!("{library}.{standard}.all"),
                                }],
                                safe: false,
                            }),
                        );
                    }
                }
            }
            SyntaxKind::PROCESS_STMT if enabled("style/process-label") => {
                if !node.children().any(|n| n.kind() == SyntaxKind::LABEL)
                    && let Some(kw) = first_token(&node, SyntaxKind::PROCESS_KW)
                {
                    out.push(Finding::new(
                        "style/process-label",
                        kw.text_range(),
                        "process without a label",
                    ));
                }
            }
            SyntaxKind::GENERIC_MAP_ASPECT | SyntaxKind::PORT_MAP_ASPECT
                if enabled("style/positional-association")
                    && node
                        .parent()
                        .is_some_and(|p| p.kind() == SyntaxKind::COMPONENT_INSTANTIATION_STMT) =>
            {
                let positional = node
                    .descendants()
                    .filter(|n| n.kind() == SyntaxKind::ASSOCIATION_ELEMENT)
                    .filter(|n| n.parent().and_then(|p| p.parent()).as_ref() == Some(&node))
                    .any(|n| !n.has_child(|k| k == SyntaxKind::ARROW));
                if positional {
                    let what = if node.kind() == SyntaxKind::PORT_MAP_ASPECT {
                        "port"
                    } else {
                        "generic"
                    };
                    out.push(Finding::new(
                        "style/positional-association",
                        node.text_range(),
                        format!("{what} map associates by position; name the formals"),
                    ));
                }
            }
            SyntaxKind::BINARY_EXPR => {
                if enabled("style/clock-edge") {
                    clock_edge(ctx, &node, out);
                }
                if enabled("style/boolean-comparison") {
                    boolean_comparison(ctx, &node, out);
                }
            }
            SyntaxKind::CASE_STMT => case_others_check(ctx.text, &node, out),
            SyntaxKind::LITERAL => magic_number(ctx, &node, out),
            SyntaxKind::GENERIC_CLAUSE => generic_defaults(&node, out),
            SyntaxKind::PORT_CLAUSE => port_order_check(ctx, &node, out),
            SyntaxKind::VARIABLE_DECL => {
                if let Some(shared) = node.child_token(|k| k == SyntaxKind::SHARED_KW) {
                    let name = node
                        .children()
                        .find(|n| n.kind() == SyntaxKind::IDENT_LIST)
                        .map(|n| n.text().to_string())
                        .unwrap_or_default();
                    out.push(Finding::new(
                        "style/shared-variable",
                        TextRange::new(shared.text_range().start(), node.text_range().end()),
                        format!("shared variable `{}`", name.trim()),
                    ));
                }
            }
            _ => {}
        }
        if process_all && node.kind() == SyntaxKind::PROCESS_STMT {
            process_all_check(ctx, &node, out);
        }
    }
    if enabled("style/file-name") {
        file_name(ctx, out);
    }
    if enabled("style/one-unit-per-file") {
        one_unit_per_file(ctx, out);
    }
    if enabled("style/file-header")
        && let Some(header) = ctx.header
    {
        file_header(ctx, header, out);
    }
}

/// `style/boolean-comparison`: `x = true`, `x /= false` and the like.
fn boolean_comparison(ctx: &StyleContext, node: &SyntaxNode, out: &mut Vec<Finding>) {
    let Some(operator) = node.child_token(|k| !k.is_trivia()) else {
        return;
    };
    if !matches!(operator.kind(), SyntaxKind::EQ | SyntaxKind::NEQ) {
        return;
    }
    let operands: Vec<SyntaxNode> = node.children().collect();
    let [a, b] = operands.as_slice() else { return };
    let literal = |n: &SyntaxNode| -> Option<bool> {
        if n.kind() != SyntaxKind::NAME {
            return None;
        }
        match n.text().to_string().trim().to_ascii_lowercase().as_str() {
            "true" => Some(true),
            "false" => Some(false),
            _ => None,
        }
    };
    let (value, other) = match (literal(a), literal(b)) {
        (Some(v), None) => (v, b),
        (None, Some(v)) => (v, a),
        _ => return,
    };
    let negate = value == (operator.kind() == SyntaxKind::NEQ);
    let text = &ctx.text[other.text_range()];
    let replacement = if !negate {
        text.to_owned()
    } else if matches!(
        other.kind(),
        SyntaxKind::BINARY_EXPR | SyntaxKind::UNARY_EXPR
    ) {
        format!("not ({text})")
    } else {
        format!("not {text}")
    };
    out.push(
        Finding::new(
            "style/boolean-comparison",
            node.text_range(),
            format!(
                "comparison of a boolean with `{value}`; write `{}`",
                replacement
            ),
        )
        .with_fix(Fix {
            title: format!("Replace with `{replacement}`"),
            edits: vec![Edit {
                range: node.text_range(),
                text: replacement,
            }],
            safe: true,
        }),
    );
}

/// `style/magic-number`: an integer literal in a statement, other than
/// the allowed ones. Declarations (constants, types, generics, signals
/// and their constraints) and generic maps are where numbers get names.
fn magic_number(ctx: &StyleContext, node: &SyntaxNode, out: &mut Vec<Finding>) {
    use SyntaxKind::*;
    let Some(token) = node.first_token() else {
        return;
    };
    let value = match token.kind() {
        DECIMAL_LITERAL => {
            let text = token.text().replace('_', "");
            if text.contains(['.', 'e', 'E']) {
                return;
            }
            text.parse::<i64>().ok()
        }
        BASED_LITERAL => {
            let text = token.text().replace('_', "");
            let mut parts = text.split('#');
            let base = parts.next().and_then(|b| b.parse::<u32>().ok());
            let digits = parts.next();
            match (base, digits) {
                (Some(base), Some(digits)) if (2..=16).contains(&base) && !digits.contains('.') => {
                    i64::from_str_radix(digits, base).ok()
                }
                _ => return,
            }
        }
        _ => return,
    };
    if value.is_some_and(|v| ctx.magic_allow.contains(&v)) {
        return;
    }
    let mut in_statement = false;
    for ancestor in node.ancestors().skip(1) {
        match ancestor.kind() {
            PHYSICAL_LITERAL
            | CONSTANT_DECL
            | SIGNAL_DECL
            | VARIABLE_DECL
            | FILE_DECL
            | TYPE_DECL
            | SUBTYPE_DECL
            | ALIAS_DECL
            | ATTRIBUTE_SPEC
            | INTERFACE_OBJECT_DECL
            | GENERIC_MAP_ASPECT
            | SUBTYPE_INDICATION
            | COMPONENT_DECL
            | CONFIGURATION_DECL => {
                return;
            }
            PROCESS_STMT | SIGNAL_ASSIGN_STMT | SELECTED_ASSIGN_STMT | VARIABLE_ASSIGN_STMT
            | PROCEDURE_CALL_STMT | IF_STMT | CASE_STMT | LOOP_STMT | WAIT_STMT | RETURN_STMT
            | FOR_GENERATE_STMT | IF_GENERATE_STMT | CASE_GENERATE_STMT | PORT_MAP_ASPECT => {
                in_statement = true
            }
            DESIGN_UNIT => break,
            _ => {}
        }
    }
    if in_statement {
        out.push(Finding::new(
            "style/magic-number",
            node.text_range(),
            format!(
                "magic number `{}`; give it a name with a constant",
                token.text()
            ),
        ));
    }
}

/// `style/generic-default`: generics of an entity or component without a
/// default value.
fn generic_defaults(clause: &SyntaxNode, out: &mut Vec<Finding>) {
    if !clause.parent().is_some_and(|p| {
        matches!(
            p.kind(),
            SyntaxKind::ENTITY_DECL | SyntaxKind::COMPONENT_DECL
        )
    }) {
        return;
    }
    for decl in clause
        .descendants()
        .filter(|n| n.kind() == SyntaxKind::INTERFACE_OBJECT_DECL)
    {
        if decl.has_child(|k| k == SyntaxKind::COLON_EQ) {
            continue;
        }
        let Some(list) = decl.children().find(|n| n.kind() == SyntaxKind::IDENT_LIST) else {
            continue;
        };
        out.push(Finding::new(
            "style/generic-default",
            list.text_range(),
            format!(
                "generic `{}` has no default value",
                list.text().to_string().trim()
            ),
        ));
    }
}

/// `style/port-order`: clock and reset ports of an entity come first.
fn port_order_check(ctx: &StyleContext, clause: &SyntaxNode, out: &mut Vec<Finding>) {
    if !clause
        .parent()
        .is_some_and(|p| p.kind() == SyntaxKind::ENTITY_DECL)
    {
        return;
    }
    let (Some(clocks), Some(resets)) = (ctx.clocks, ctx.resets) else {
        return;
    };
    let mut other: Option<String> = None;
    for decl in clause
        .descendants()
        .filter(|n| n.kind() == SyntaxKind::INTERFACE_OBJECT_DECL)
    {
        let Some(list) = decl.children().find(|n| n.kind() == SyntaxKind::IDENT_LIST) else {
            continue;
        };
        for name in list
            .children_with_tokens()
            .filter_map(|e| e.into_token())
            .filter(|t| t.kind() == SyntaxKind::IDENT)
        {
            let what = if clocks.is_match(name.text()) {
                "clock"
            } else if resets.is_match(name.text()) {
                "reset"
            } else {
                if other.is_none() {
                    other = Some(name.text().to_owned());
                }
                continue;
            };
            if let Some(first) = &other {
                out.push(Finding::new(
                    "style/port-order",
                    name.text_range(),
                    format!(
                        "{what} port `{}` comes after port `{first}`; put clocks and resets first",
                        name.text()
                    ),
                ));
            }
        }
    }
}

/// `style/process-all`: a combinational process with a sensitivity list
/// of signals, in VHDL-2008.
fn process_all_check(ctx: &StyleContext, process: &SyntaxNode, out: &mut Vec<Finding>) {
    use SyntaxKind::*;
    let Some(list) = process.children().find(|n| n.kind() == SENSITIVITY_LIST) else {
        return;
    };
    if list.has_child(|k| k == ALL_KW) {
        return;
    }
    // Clocked processes and processes with `wait` are not combinational.
    let clocked = !descendants_where(process, |k| matches!(k, WAIT_STMT)).is_empty()
        || descendants_where(process, |k| matches!(k, CALL_EXPR | ATTRIBUTE_NAME))
            .iter()
            .any(is_clock_edge);
    if clocked {
        return;
    }
    // A list cut short by a syntax error (`process (`) has no inside.
    let written = &ctx.text[list.text_range()];
    if written.len() < 2 || !written.starts_with('(') || !written.ends_with(')') {
        return;
    }
    let inner = TextRange::new(
        list.text_range().start() + TextSize::from(1),
        list.text_range().end() - TextSize::from(1),
    );
    out.push(
        Finding::new(
            "style/process-all",
            list.text_range(),
            "combinational process: use `process (all)` instead of listing its signals",
        )
        .with_fix(Fix {
            title: "Use `process (all)`".into(),
            edits: vec![Edit {
                range: inner,
                text: "all".into(),
            }],
            // A list that leaves signals out changes simulation.
            safe: false,
        }),
    );
}

/// `rising_edge(x)`, `falling_edge(x)` or `x'event`.
fn is_clock_edge(node: &SyntaxNode) -> bool {
    match node.kind() {
        SyntaxKind::CALL_EXPR => node.children().next().is_some_and(|n| {
            let name = n.text().to_string().trim().to_ascii_lowercase();
            name == "rising_edge" || name == "falling_edge"
        }),
        SyntaxKind::ATTRIBUTE_NAME => is_event(node).is_some(),
        _ => false,
    }
}

/// `style/one-unit-per-file`: the primary units after the first.
fn one_unit_per_file(ctx: &StyleContext, out: &mut Vec<Finding>) {
    for (name, range) in primary_units(ctx.root).into_iter().skip(1) {
        out.push(Finding::new(
            "style/one-unit-per-file",
            range,
            format!("`{name}` is not the first primary unit of this file; give it its own file"),
        ));
    }
}

/// `style/file-header`: the comments before the first token must match.
fn file_header(ctx: &StyleContext, pattern: &Regex, out: &mut Vec<Finding>) {
    let end = ctx
        .root
        .descendants_with_tokens()
        .filter_map(|e| e.into_token())
        .find(|t| !t.kind().is_trivia())
        .map_or(ctx.text.len(), |t| usize::from(t.text_range().start()));
    let header = &ctx.text[..end];
    if pattern.is_match(header) {
        return;
    }
    let first_line = ctx.text.find('\n').unwrap_or(ctx.text.len());
    let first_line = ctx.text[..first_line].trim_end_matches('\r').len();
    out.push(Finding::new(
        "style/file-header",
        range(0, first_line),
        "the file header does not match the configured pattern",
    ));
}

/// `style/case-others`: a `case` statement without `when others`.
fn case_others_check(text: &str, node: &SyntaxNode, out: &mut Vec<Finding>) {
    let alternatives: Vec<SyntaxNode> = node
        .children()
        .filter(|n| n.kind() == SyntaxKind::CASE_ALTERNATIVE)
        .collect();
    let has_others = alternatives.iter().any(|a| {
        a.children()
            .filter(|n| n.kind() == SyntaxKind::CHOICES)
            .any(|c| {
                c.descendants_with_tokens()
                    .any(|e| e.kind() == SyntaxKind::OTHERS_KW)
            })
    });
    let Some(case_kw) = first_token(node, SyntaxKind::CASE_KW) else {
        return;
    };
    if has_others || alternatives.is_empty() {
        return;
    }
    let mut finding = Finding::new(
        "style/case-others",
        case_kw.text_range(),
        "`case` without `when others`",
    );
    // `when others => null;` before `end case`, indented as the other
    // alternatives. Legal even when the choices already cover every value.
    let line_start = |offset: usize| text[..offset].rfind('\n').map_or(0, |i| i + 1);
    if let Some(end) = first_token(node, SyntaxKind::END_KW) {
        let end = usize::from(end.text_range().start());
        let start = line_start(end);
        let when = usize::from(alternatives[0].text_range().start());
        let when_start = line_start(when);
        if text[start..end].trim().is_empty() && text[when_start..when].trim().is_empty() {
            let newline = if text[..start].ends_with("\r\n") {
                "\r\n"
            } else {
                "\n"
            };
            let at = TextSize::from(start as u32);
            finding = finding.with_fix(Fix {
                title: "Add `when others => null;`".into(),
                edits: vec![Edit {
                    range: TextRange::empty(at),
                    text: format!("{}when others => null;{newline}", &text[when_start..when]),
                }],
                safe: true,
            });
        }
    }
    out.push(finding);
}

fn first_token(node: &SyntaxNode, kind: SyntaxKind) -> Option<vfl_syntax::SyntaxToken> {
    node.child_token(|k| k == kind)
}

/// `style/line-length`, `style/trailing-whitespace` and `style/tabs`,
/// which only look at the text.
pub(crate) fn lines(
    text: &str,
    max_line_length: usize,
    tab_width: usize,
    enabled: &impl Fn(&str) -> bool,
    out: &mut Vec<Finding>,
) {
    let mut offset = 0usize;
    for line in text.split_inclusive('\n') {
        let content = line.trim_end_matches(['\n', '\r']);
        let start = offset;
        offset += line.len();
        if enabled("style/line-length") {
            // Columns on screen, as the formatter counts them: CJK and
            // other wide characters take two.
            let char_width = |c: char| {
                if c.is_ascii() {
                    1
                } else {
                    unicode_width::UnicodeWidthChar::width(c).unwrap_or(1)
                }
            };
            let width: usize = if content.is_ascii() {
                content.len()
            } else {
                content.chars().map(char_width).sum()
            };
            if width > max_line_length {
                let mut col = 0;
                let cut = content
                    .char_indices()
                    .find(|&(_, c)| {
                        col += char_width(c);
                        col > max_line_length
                    })
                    .map_or(content.len(), |(i, _)| i);
                out.push(Finding::new(
                    "style/line-length",
                    range(start + cut, start + content.len()),
                    format!("line is {width} characters long (max {max_line_length})"),
                ));
            }
        }
        if enabled("style/trailing-whitespace") {
            let trimmed = content.trim_end_matches([' ', '\t']);
            if trimmed.len() < content.len() {
                let r = range(start + trimmed.len(), start + content.len());
                out.push(
                    Finding::new("style/trailing-whitespace", r, "trailing whitespace").with_fix(
                        Fix {
                            title: "Remove trailing whitespace".into(),
                            edits: vec![Edit {
                                range: r,
                                text: String::new(),
                            }],
                            safe: true,
                        },
                    ),
                );
            }
        }
        if enabled("style/tabs") {
            let indent = content.len() - content.trim_start_matches([' ', '\t']).len();
            if content[..indent].contains('\t') {
                let r = range(start, start + indent);
                let tab = tab_width.max(1);
                let mut column = 0;
                for c in content[..indent].chars() {
                    column = if c == '\t' {
                        (column / tab + 1) * tab
                    } else {
                        column + 1
                    };
                }
                out.push(
                    Finding::new("style/tabs", r, "line is indented with tabs").with_fix(Fix {
                        title: "Indent with spaces".into(),
                        edits: vec![Edit {
                            range: r,
                            text: " ".repeat(column),
                        }],
                        safe: true,
                    }),
                );
            }
        }
    }
}

fn range(start: usize, end: usize) -> TextRange {
    TextRange::new(TextSize::from(start as u32), TextSize::from(end as u32))
}

/// `clk'event and clk = '1'` (either order), suggesting `rising_edge(clk)`.
fn clock_edge(ctx: &StyleContext, node: &SyntaxNode, out: &mut Vec<Finding>) {
    let operator = node.child_token(|k| !k.is_trivia());
    if operator.map(|t| t.kind()) != Some(SyntaxKind::AND_KW) {
        return;
    }
    let operands: Vec<SyntaxNode> = node.children().collect();
    let [a, b] = operands.as_slice() else { return };
    let (event, compare) = if is_event(a).is_some() {
        (a, b)
    } else if is_event(b).is_some() {
        (b, a)
    } else {
        return;
    };
    let Some(signal) = is_event(event) else {
        return;
    };
    let Some(value) = compared_value(compare, &signal) else {
        return;
    };
    let function = match value.as_str() {
        "'1'" => "rising_edge",
        "'0'" => "falling_edge",
        _ => return,
    };
    let replacement = format!("{function}({})", &ctx.text[signal_range(event)]);
    out.push(
        Finding::new(
            "style/clock-edge",
            node.text_range(),
            format!("use `{replacement}` instead of testing `'event`"),
        )
        .with_fix(Fix {
            title: format!("Replace with `{replacement}`"),
            edits: vec![Edit {
                range: node.text_range(),
                text: replacement,
            }],
            // `rising_edge` also excludes 'X' -> '1' transitions.
            safe: false,
        }),
    );
}

/// The signal of `s'event`, lower-cased.
fn is_event(node: &SyntaxNode) -> Option<String> {
    if node.kind() != SyntaxKind::ATTRIBUTE_NAME {
        return None;
    }
    let attribute = node
        .children_with_tokens()
        .filter_map(|e| e.into_token())
        .skip_while(|t| t.kind() != SyntaxKind::TICK)
        .nth(1)?;
    if !attribute.text().eq_ignore_ascii_case("event") {
        return None;
    }
    let prefix = node.children().next()?;
    (prefix.kind() == SyntaxKind::NAME).then(|| prefix.to_string().trim().to_ascii_lowercase())
}

fn signal_range(event: &SyntaxNode) -> TextRange {
    event
        .children()
        .next()
        .map_or(event.text_range(), |n| n.text_range())
}

/// For `s = 'x'` comparing `signal`, the literal.
fn compared_value(node: &SyntaxNode, signal: &str) -> Option<String> {
    let node = if node.kind() == SyntaxKind::PAREN_EXPR {
        node.children().next()?
    } else {
        node.clone()
    };
    if node.kind() != SyntaxKind::BINARY_EXPR {
        return None;
    }
    let operator = node.child_token(|k| !k.is_trivia())?;
    if operator.kind() != SyntaxKind::EQ {
        return None;
    }
    let operands: Vec<SyntaxNode> = node.children().collect();
    let [l, r] = operands.as_slice() else {
        return None;
    };
    if l.to_string().trim().to_ascii_lowercase() != signal {
        return None;
    }
    Some(r.to_string().trim().to_owned())
}

fn file_name(ctx: &StyleContext, out: &mut Vec<Finding>) {
    let Some(stem) = &ctx.file_stem else { return };
    let stem = stem.to_ascii_lowercase();
    let primaries = primary_units(ctx.root);
    let Some((first, range)) = primaries.first() else {
        return;
    };
    if primaries.iter().all(|(name, _)| *name != stem) {
        let extension = ctx.extension.as_deref().unwrap_or("vhd");
        out.push(Finding::new(
            "style/file-name",
            *range,
            format!(
                "file name does not match the unit it declares; expected `{first}.{extension}`"
            ),
        ));
    }
}

/// The names (lower case) and name ranges of a file's primary units.
fn primary_units(root: &SyntaxNode) -> Vec<(String, TextRange)> {
    root.children()
        .filter(|n| n.kind() == SyntaxKind::DESIGN_UNIT)
        .flat_map(|n| n.children())
        .filter(|n| {
            matches!(
                n.kind(),
                SyntaxKind::ENTITY_DECL
                    | SyntaxKind::PACKAGE_DECL
                    | SyntaxKind::CONTEXT_DECL
                    | SyntaxKind::CONFIGURATION_DECL
                    | SyntaxKind::PACKAGE_INSTANTIATION_DECL
            )
        })
        .filter_map(|n| {
            n.child_token(|k| k == SyntaxKind::IDENT)
                .map(|t| (t.text().to_ascii_lowercase(), t.text_range()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_length_counts_wide_characters_twice() {
        let mut out = Vec::new();
        let only = |rule: &str| rule == "style/line-length";
        // 6 characters, 9 columns: past a limit of 6 from the second wide one.
        lines("-- 日本語\n-- ab\n", 6, 4, &only, &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].message, "line is 9 characters long (max 6)");
        assert_eq!(usize::from(out[0].range.start()), "-- 日".len());
    }
}

//! Rules that only look at the text and the syntax tree of one file.

use vfl_sema::{Edit, Fix};
use vfl_syntax::green_walk::{ChildTokens, descendants_where};
use vfl_syntax::{Standard, SyntaxKind, SyntaxNode, TextRange, TextSize};

use crate::Finding;

/// Context for the style rules of one file.
pub struct StyleContext<'a> {
    pub text: &'a str,
    pub root: &'a SyntaxNode,
    pub file_stem: Option<String>,
    /// `style/line-length`'s limit.
    pub max_line_length: usize,
    /// Columns per indentation level: what a tab becomes.
    pub indent: usize,
    pub standard: Standard,
}

pub fn check(ctx: &StyleContext, enabled: impl Fn(&str) -> bool, out: &mut Vec<Finding>) {
    if enabled("style/line-length") || enabled("style/trailing-whitespace") || enabled("style/tabs")
    {
        lines(ctx.text, ctx.max_line_length, ctx.indent, &enabled, out);
    }
    // The kinds the arms below look at: only these become syntax nodes.
    let case_others = enabled("style/case-others");
    let wanted = |kind| {
        (case_others && kind == SyntaxKind::CASE_STMT)
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
            SyntaxKind::BINARY_EXPR if enabled("style/clock-edge") => clock_edge(ctx, &node, out),
            SyntaxKind::CASE_STMT => case_others_check(ctx.text, &node, out),
            _ => {}
        }
    }
    if enabled("style/file-name") {
        file_name(ctx, out);
    }
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
            let width = content.chars().count();
            if width > max_line_length {
                let cut = content
                    .char_indices()
                    .nth(max_line_length)
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
    let primaries: Vec<(String, TextRange)> = ctx
        .root
        .children()
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
        .collect();
    let Some((first, range)) = primaries.first() else {
        return;
    };
    if primaries.iter().all(|(name, _)| *name != stem) {
        out.push(Finding::new(
            "style/file-name",
            *range,
            format!("file name does not match the unit it declares; expected `{first}.vhd`"),
        ));
    }
}

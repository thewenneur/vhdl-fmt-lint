//! Rules on the structure of the code, from its syntax tree and comments:
//! nesting depth, declarations of several objects, `others` choices out
//! of place, library clauses nothing needs, deprecated packages, names
//! reserved elsewhere, operators whose precedence surprises, nested
//! comments, literal forms, vector directions, prohibited constructs,
//! unknown pragmas, missing labels, encrypted blocks.

use vfl_sema::{Edit, Fix};
use vfl_syntax::green_walk::{ChildTokens, descendants_where};
use vfl_syntax::{Standard, SyntaxKind, SyntaxNode, TextRange, TextSize};

use crate::Finding;
use crate::style::StyleContext;

pub const NESTING_DEPTH: &str = "style/nesting-depth";
pub const MULTIPLE_DECLARATIONS: &str = "style/multiple-declarations";
pub const OTHERS_POSITION: &str = "sema/others-position";
pub const UNNEEDED_LIBRARY: &str = "style/unneeded-library";
pub const DEPRECATED_PACKAGE: &str = "style/deprecated-package";
pub const RESERVED_WORD: &str = "style/reserved-word";
pub const OPERATOR_PRECEDENCE: &str = "style/operator-precedence";
pub const NESTED_COMMENT: &str = "style/nested-comment";
pub const LITERAL_FORM: &str = "style/literal-form";
pub const RANGE_DIRECTION: &str = "style/range-direction";
pub const PROHIBITED: &str = "style/prohibited";
pub const UNKNOWN_PRAGMA: &str = "style/unknown-pragma";
pub const MISSING_LABEL: &str = "style/missing-label";
pub const ENCRYPTED_FILE: &str = "project/encrypted-file";
pub const SECONDARY_UNIT_FILE: &str = "style/secondary-unit-file";

/// The options of these rules.
#[derive(Debug, Clone, Default)]
pub struct Options {
    /// `style/nesting-depth`'s `max`.
    pub max_depth: usize,
    /// `style/range-direction`'s `direction`: `to` when true.
    pub ascending: bool,
    /// `style/prohibited`'s lists, lower case.
    pub keywords: Vec<String>,
    pub operators: Vec<String>,
    pub attributes: Vec<String>,
    pub pragmas: Vec<String>,
    /// `style/missing-label`'s statements: `loop`, `if`, `case`.
    pub labels: Vec<String>,
}

pub const IDS: &[&str] = &[
    NESTING_DEPTH,
    MULTIPLE_DECLARATIONS,
    OTHERS_POSITION,
    UNNEEDED_LIBRARY,
    DEPRECATED_PACKAGE,
    RESERVED_WORD,
    OPERATOR_PRECEDENCE,
    NESTED_COMMENT,
    LITERAL_FORM,
    RANGE_DIRECTION,
    PROHIBITED,
    UNKNOWN_PRAGMA,
    MISSING_LABEL,
    ENCRYPTED_FILE,
];

pub fn check(
    ctx: &StyleContext,
    options: &Options,
    enabled: impl Fn(&str) -> bool,
    out: &mut Vec<Finding>,
) {
    if !IDS.iter().any(|id| enabled(id)) {
        return;
    }
    let depth = enabled(NESTING_DEPTH);
    let multiple = enabled(MULTIPLE_DECLARATIONS);
    let others = enabled(OTHERS_POSITION);
    let library = enabled(UNNEEDED_LIBRARY);
    let deprecated = enabled(DEPRECATED_PACKAGE);
    let precedence = enabled(OPERATOR_PRECEDENCE);
    let literal = enabled(LITERAL_FORM);
    let direction = enabled(RANGE_DIRECTION);
    let prohibited = enabled(PROHIBITED) && !options.attributes.is_empty();
    let labels = enabled(MISSING_LABEL);
    let wanted = |kind| {
        use SyntaxKind::*;
        (depth && matches!(kind, PROCESS_STMT | SUBPROGRAM_BODY))
            || (multiple && matches!(kind, SIGNAL_DECL | VARIABLE_DECL | CONSTANT_DECL))
            || (others && matches!(kind, CASE_STMT | AGGREGATE | SELECTED_WAVEFORMS))
            || (library && kind == DESIGN_UNIT)
            || (deprecated && kind == USE_CLAUSE)
            || (precedence && kind == BINARY_EXPR)
            || (literal && matches!(kind, FOR_SCHEME | FOR_GENERATE_STMT))
            || (direction && kind == SUBTYPE_INDICATION)
            || (prohibited && kind == ATTRIBUTE_NAME)
            || (labels && matches!(kind, LOOP_STMT | IF_STMT | CASE_STMT))
    };
    for node in descendants_where(ctx.root, wanted) {
        match node.kind() {
            SyntaxKind::PROCESS_STMT | SyntaxKind::SUBPROGRAM_BODY => {
                nesting(&node, options.max_depth.max(1), out)
            }
            SyntaxKind::SIGNAL_DECL | SyntaxKind::VARIABLE_DECL | SyntaxKind::CONSTANT_DECL => {
                multiple_declaration(&node, out)
            }
            SyntaxKind::CASE_STMT => {
                if others {
                    others_position(&node, out);
                }
                if labels {
                    missing_label(&node, &options.labels, out);
                }
            }
            SyntaxKind::AGGREGATE | SyntaxKind::SELECTED_WAVEFORMS => others_position(&node, out),
            SyntaxKind::DESIGN_UNIT => libraries(ctx, &node, out),
            SyntaxKind::USE_CLAUSE => deprecated_package(ctx, &node, out),
            SyntaxKind::BINARY_EXPR => operator_precedence(&node, out),
            SyntaxKind::FOR_SCHEME | SyntaxKind::FOR_GENERATE_STMT => {
                parenthesized_range(&node, out)
            }
            SyntaxKind::SUBTYPE_INDICATION => range_direction(&node, options.ascending, out),
            SyntaxKind::ATTRIBUTE_NAME => prohibited_attribute(&node, options, out),
            SyntaxKind::LOOP_STMT | SyntaxKind::IF_STMT => {
                missing_label(&node, &options.labels, out)
            }
            _ => {}
        }
    }
    // Token rules: one pass over the tokens.
    let reserved = enabled(RESERVED_WORD);
    let nested = enabled(NESTED_COMMENT);
    let pragmas = enabled(UNKNOWN_PRAGMA);
    let encrypted = enabled(ENCRYPTED_FILE);
    let prohibited_tokens = enabled(PROHIBITED)
        && !(options.keywords.is_empty()
            && options.operators.is_empty()
            && options.pragmas.is_empty());
    if reserved || nested || pragmas || encrypted || prohibited_tokens || literal {
        let mut encrypted_seen = false;
        for token in ctx
            .root
            .descendants_with_tokens()
            .filter_map(|e| e.into_token())
        {
            let kind = token.kind();
            match kind {
                SyntaxKind::BLOCK_COMMENT if nested => {
                    let text = token.text();
                    let inner = &text[2.min(text.len())..];
                    if let Some(at) = inner.find("/*") {
                        let start = token.text_range().start() + TextSize::from(2 + at as u32);
                        out.push(Finding::new(
                            NESTED_COMMENT,
                            TextRange::at(start, 2.into()),
                            "`/*` inside a block comment: block comments do not nest, so the first `*/` ends both",
                        ));
                    }
                }
                SyntaxKind::LINE_COMMENT | SyntaxKind::BLOCK_COMMENT => {
                    if encrypted
                        && !encrypted_seen
                        && token
                            .text()
                            .to_ascii_lowercase()
                            .contains("protect begin_protected")
                    {
                        encrypted_seen = true;
                        out.push(Finding::new(
                            ENCRYPTED_FILE,
                            token.text_range(),
                            "encrypted (IEEE 1735) code: its contents are not checked",
                        ));
                    }
                    if (pragmas || prohibited_tokens) && kind == SyntaxKind::LINE_COMMENT {
                        pragma(&token, pragmas, &options.pragmas, prohibited_tokens, out);
                    }
                }
                SyntaxKind::TOOL_DIRECTIVE if encrypted && !encrypted_seen => {
                    if token.text().to_ascii_lowercase().contains("protect") {
                        encrypted_seen = true;
                        out.push(Finding::new(
                            ENCRYPTED_FILE,
                            token.text_range(),
                            "encrypted (IEEE 1735) code: its contents are not checked",
                        ));
                    }
                }
                SyntaxKind::DECIMAL_LITERAL if literal => {
                    let text = token.text();
                    if !text.contains('.')
                        && let Some((_, exponent)) = text.split_once(['e', 'E'])
                        && exponent.starts_with('-')
                    {
                        out.push(Finding::new(
                            LITERAL_FORM,
                            token.text_range(),
                            "an integer literal cannot have a negative exponent; write a real literal (`1.0e-3`)",
                        ));
                    }
                }
                SyntaxKind::IDENT if reserved => reserved_word(ctx, &token, out),
                _ if prohibited_tokens && !kind.is_trivia() => {
                    let text = token.text().to_ascii_lowercase();
                    let keyword = options.keywords.contains(&text)
                        && token
                            .text()
                            .chars()
                            .all(|c| c.is_ascii_alphabetic() || c == '_')
                        && kind != SyntaxKind::IDENT;
                    let operator = options.operators.contains(&text) && kind != SyntaxKind::IDENT;
                    if keyword || operator {
                        out.push(Finding::new(
                            PROHIBITED,
                            token.text_range(),
                            format!(
                                "the {} `{}` is prohibited in this project",
                                if keyword { "keyword" } else { "operator" },
                                token.text()
                            ),
                        ));
                    }
                }
                _ => {}
            }
        }
    }
}

// ---- E25: nesting ---------------------------------------------------------------

fn nesting(body: &SyntaxNode, max: usize, out: &mut Vec<Finding>) {
    // The first statement nested deeper than `max`, if any.
    let nested = |n: &SyntaxNode| {
        matches!(
            n.kind(),
            SyntaxKind::IF_STMT | SyntaxKind::CASE_STMT | SyntaxKind::LOOP_STMT
        )
    };
    for n in body.descendants().filter(nested) {
        // Not inside a nested subprogram or process: it is checked on its own.
        let depth = n
            .ancestors()
            .take_while(|a| a != body)
            .filter(nested)
            .count()
            + 1;
        if depth > max
            && n.ancestors()
                .skip(1)
                .take_while(|a| a != body)
                .filter(nested)
                .count()
                == max
            && !n.ancestors().skip(1).take_while(|a| a != body).any(|a| {
                matches!(
                    a.kind(),
                    SyntaxKind::SUBPROGRAM_BODY | SyntaxKind::PROCESS_STMT
                )
            })
        {
            let keyword = n.first_token().map_or_else(
                || n.text_range(),
                |t| {
                    n.children_with_tokens()
                        .filter_map(|e| e.into_token())
                        .find(|t| !t.kind().is_trivia())
                        .map_or(t.text_range(), |t| t.text_range())
                },
            );
            out.push(Finding::new(
                NESTING_DEPTH,
                keyword,
                format!("statements nested {depth} deep (more than {max}): move the inner part to a subprogram or flatten the conditions"),
            ));
            return;
        }
    }
}

// ---- E26: several objects in one declaration ---------------------------------------

fn multiple_declaration(node: &SyntaxNode, out: &mut Vec<Finding>) {
    let Some(list) = node.children().find(|n| n.kind() == SyntaxKind::IDENT_LIST) else {
        return;
    };
    let names = list
        .children_with_tokens()
        .filter(|e| matches!(e.kind(), SyntaxKind::IDENT | SyntaxKind::EXT_IDENT))
        .count();
    if names > 1 {
        out.push(Finding::new(
            MULTIPLE_DECLARATIONS,
            list.text_range(),
            format!("{names} objects in one declaration: declare one per line"),
        ));
    }
}

// ---- `others` out of place ---------------------------------------------------------

fn others_position(node: &SyntaxNode, out: &mut Vec<Finding>) {
    let (kind, what) = match node.kind() {
        SyntaxKind::CASE_STMT => (SyntaxKind::CASE_ALTERNATIVE, "case alternative"),
        SyntaxKind::AGGREGATE => (SyntaxKind::ELEMENT_ASSOCIATION, "association"),
        _ => (SyntaxKind::WAVEFORM, "choice"),
    };
    let items: Vec<SyntaxNode> = node.children().filter(|n| n.kind() == kind).collect();
    for (i, item) in items.iter().enumerate() {
        let choices = if kind == SyntaxKind::WAVEFORM {
            // `with s select x <= a when others, b when c;`: the choices
            // follow each waveform.
            item.next_sibling()
                .filter(|n| n.kind() == SyntaxKind::CHOICES)
        } else {
            item.children().find(|n| n.kind() == SyntaxKind::CHOICES)
        };
        let Some(choices) = choices else { continue };
        let Some(others) = choices.child_token(|k| k == SyntaxKind::OTHERS_KW) else {
            continue;
        };
        let alone = choices
            .children_with_tokens()
            .filter(|e| !e.kind().is_trivia())
            .count()
            == 1;
        if !alone {
            out.push(Finding::new(
                OTHERS_POSITION,
                others.text_range(),
                "`others` cannot be combined with other choices",
            ));
        } else if i + 1 != items.len() {
            out.push(Finding::new(
                OTHERS_POSITION,
                others.text_range(),
                format!("`others` must be the last {what}"),
            ));
        }
    }
}

// ---- E29: library clauses ------------------------------------------------------------

fn libraries(ctx: &StyleContext, unit: &SyntaxNode, out: &mut Vec<Finding>) {
    let Some(context) = unit
        .children()
        .find(|n| n.kind() == SyntaxKind::CONTEXT_CLAUSE)
    else {
        return;
    };
    // The prefixes of selected names in the file: `ieee` of `ieee.x.y`.
    // The whole file, since an entity's clauses also serve its
    // architectures.
    let used = |name: &str| {
        ctx.root
            .descendants()
            .filter(|n| n.kind() == SyntaxKind::SELECTED_NAME)
            .filter(|n| {
                !n.parent()
                    .is_some_and(|p| p.kind() == SyntaxKind::SELECTED_NAME)
            })
            .any(|n| {
                n.descendants_with_tokens()
                    .filter_map(|e| e.into_token())
                    .find(|t| !t.kind().is_trivia())
                    .is_some_and(|t| t.text().eq_ignore_ascii_case(name))
            })
            || ctx
                .root
                .descendants()
                .filter(|n| n.kind() == SyntaxKind::CONTEXT_REFERENCE)
                .any(|n| {
                    n.text()
                        .to_string()
                        .to_ascii_lowercase()
                        .contains(&format!("{name}."))
                })
    };
    for clause in context
        .children()
        .filter(|n| n.kind() == SyntaxKind::LIBRARY_CLAUSE)
    {
        let names: Vec<_> = clause
            .descendants_with_tokens()
            .filter_map(|e| e.into_token())
            .filter(|t| matches!(t.kind(), SyntaxKind::IDENT | SyntaxKind::EXT_IDENT))
            .collect();
        for name in &names {
            let key = name.text().to_ascii_lowercase();
            let message = match key.as_str() {
                "work" | "std" => format!(
                    "library `{}` is always visible: this clause is not needed",
                    name.text()
                ),
                _ if !used(&key) => format!("nothing uses library `{}`", name.text()),
                _ => continue,
            };
            let range = if names.len() == 1 {
                whole_lines(ctx.text, clause.text_range())
            } else {
                match vfl_sema_list_removal(ctx.text, name.text_range()) {
                    Some(r) => r,
                    None => continue,
                }
            };
            out.push(
                Finding::new(UNNEEDED_LIBRARY, name.text_range(), message).with_fix(Fix {
                    title: "Remove it".into(),
                    edits: vec![Edit {
                        range,
                        text: String::new(),
                    }],
                    safe: true,
                }),
            );
        }
    }
}

/// The item and its comma, in a comma-separated list.
fn vfl_sema_list_removal(text: &str, item: TextRange) -> Option<TextRange> {
    let start = usize::from(item.start());
    let end = usize::from(item.end());
    let after = &text[end..];
    let gap = after.len() - after.trim_start().len();
    if after[gap..].starts_with(',') {
        let rest = &after[gap + 1..];
        let spaces = rest.len() - rest.trim_start_matches([' ', '\t']).len();
        return Some(TextRange::new(
            item.start(),
            ((end + gap + 1 + spaces) as u32).into(),
        ));
    }
    let before = text[..start].trim_end();
    before
        .strip_suffix(',')
        .map(|b| TextRange::new((b.len() as u32).into(), item.end()))
}

/// A range extended to whole lines when nothing else is on them.
fn whole_lines(text: &str, range: TextRange) -> TextRange {
    let start = usize::from(range.start());
    let end = usize::from(range.end());
    let line_start = text[..start].rfind('\n').map_or(0, |i| i + 1);
    let line_end = text[end..].find('\n').map_or(text.len(), |i| end + i + 1);
    if text[line_start..start].trim().is_empty() && text[end..line_end].trim().is_empty() {
        TextRange::new((line_start as u32).into(), (line_end as u32).into())
    } else {
        // Take the spaces after it, so `a; b;` becomes `b;`.
        let rest = &text[end..];
        let spaces = rest.len() - rest.trim_start_matches([' ', '\t']).len();
        TextRange::new(range.start(), ((end + spaces) as u32).into())
    }
}

// ---- E30: deprecated packages ----------------------------------------------------------

fn deprecated_package(ctx: &StyleContext, clause: &SyntaxNode, out: &mut Vec<Finding>) {
    for item in clause.children() {
        let text = item.to_string().to_ascii_lowercase();
        let mut parts = text.split('.').map(str::trim);
        let (Some(library), Some(package)) = (parts.next(), parts.next()) else {
            continue;
        };
        if library != "ieee" {
            continue;
        }
        let message = match package {
            "std_logic_textio" if ctx.standard >= Standard::Vhdl2008 => {
                "`std_logic_textio` is deprecated: since VHDL-2008, `ieee.std_logic_1164` has its contents"
            }
            "std_logic_misc" => {
                "`std_logic_misc` is not an IEEE package (it is Synopsys'); VHDL-2008 has its reduction operators (`and v`)"
            }
            "numeric_std_unsigned"
            | "fixed_pkg"
            | "float_pkg"
            | "fixed_float_types"
            | "fixed_generic_pkg"
            | "float_generic_pkg"
                if ctx.standard < Standard::Vhdl2008 =>
            {
                "this package is part of VHDL-2008, not of the standard configured"
            }
            _ => continue,
        };
        out.push(Finding::new(DEPRECATED_PACKAGE, item.text_range(), message));
    }
}

// ---- E31: reserved words -----------------------------------------------------------------

/// Words reserved by VHDL-2008, which earlier standards allow as names.
const VHDL_2008_WORDS: &[&str] = &[
    "assume",
    "assume_guarantee",
    "context",
    "cover",
    "default",
    "fairness",
    "force",
    "parameter",
    "property",
    "release",
    "restrict",
    "restrict_guarantee",
    "sequence",
    "strong",
    "vmode",
    "vprop",
    "vunit",
];
/// Words reserved by VHDL-2019.
const VHDL_2019_WORDS: &[&str] = &["private", "view"];
/// SystemVerilog keywords that are valid VHDL names: an entity or port
/// with such a name cannot be instantiated from Verilog.
const VERILOG_WORDS: &[&str] = &[
    "always",
    "always_comb",
    "always_ff",
    "always_latch",
    "assign",
    "bit",
    "byte",
    "class",
    "default",
    "defparam",
    "endmodule",
    "enum",
    "input",
    "int",
    "integer",
    "interface",
    "logic",
    "longint",
    "module",
    "output",
    "parameter",
    "real",
    "reg",
    "shortint",
    "signed",
    "string",
    "struct",
    "supply0",
    "supply1",
    "time",
    "tri",
    "unsigned",
    "wire",
];

fn reserved_word(ctx: &StyleContext, token: &vfl_syntax::SyntaxToken, out: &mut Vec<Finding>) {
    let key = token.text().to_ascii_lowercase();
    let later = if ctx.standard < Standard::Vhdl2008 && VHDL_2008_WORDS.contains(&key.as_str()) {
        Some("VHDL-2008")
    } else if ctx.standard < Standard::Vhdl2019 && VHDL_2019_WORDS.contains(&key.as_str()) {
        Some("VHDL-2019")
    } else {
        None
    };
    // Only where the name is declared: an entity, a port or generic, an
    // object.
    let Some(parent) = token.parent() else { return };
    let declares = matches!(
        parent.kind(),
        SyntaxKind::ENTITY_DECL | SyntaxKind::IDENT_LIST
    );
    if !declares {
        return;
    }
    if let Some(standard) = later {
        out.push(Finding::new(
            RESERVED_WORD,
            token.text_range(),
            format!(
                "`{}` is a reserved word in {standard}: the code will not compile with it",
                token.text()
            ),
        ));
        return;
    }
    let interface = parent.kind() == SyntaxKind::ENTITY_DECL
        || parent.ancestors().any(|a| {
            matches!(
                a.kind(),
                SyntaxKind::PORT_CLAUSE | SyntaxKind::GENERIC_CLAUSE
            )
        }) && parent
            .ancestors()
            .any(|a| a.kind() == SyntaxKind::ENTITY_DECL);
    if interface && VERILOG_WORDS.contains(&key.as_str()) {
        out.push(Finding::new(
            RESERVED_WORD,
            token.text_range(),
            format!(
                "`{}` is a SystemVerilog keyword: Verilog code cannot instantiate or connect it",
                token.text()
            ),
        ));
    }
}

// ---- E32: precedence -------------------------------------------------------------------

fn operator_precedence(node: &SyntaxNode, out: &mut Vec<Finding>) {
    use SyntaxKind::*;
    let Some(op) = node.child_token(|k| !k.is_trivia()) else {
        return;
    };
    let relational = matches!(op.kind(), EQ | NEQ | LT | LTE | GT | GTE);
    let shift = matches!(
        op.kind(),
        SLL_KW | SRL_KW | SLA_KW | SRA_KW | ROL_KW | ROR_KW
    );
    let mut operands = node.children();
    let (Some(left), right) = (operands.next(), operands.next()) else {
        return;
    };
    if relational
        && left.kind() == UNARY_EXPR
        && left
            .child_token(|k| !k.is_trivia())
            .is_some_and(|t| t.kind() == NOT_KW)
    {
        out.push(Finding::new(
            OPERATOR_PRECEDENCE,
            node.text_range(),
            format!(
                "`not` applies to its operand only, before `{}`: write `(not a) {} b` or `not (a {} b)`",
                op.text(),
                op.text(),
                op.text()
            ),
        ));
        return;
    }
    if shift {
        for operand in [Some(left), right].into_iter().flatten() {
            if operand.kind() == BINARY_EXPR
                && operand
                    .child_token(|k| !k.is_trivia())
                    .is_some_and(|t| matches!(t.kind(), PLUS | MINUS | AMP))
            {
                out.push(Finding::new(
                    OPERATOR_PRECEDENCE,
                    operand.text_range(),
                    format!(
                        "`{}` is computed before `{}`: add parentheses to show it",
                        operand.text().to_string().trim(),
                        op.text()
                    ),
                ));
            }
        }
    }
}

// ---- E34: literal forms ------------------------------------------------------------------

fn parenthesized_range(node: &SyntaxNode, out: &mut Vec<Finding>) {
    let Some(aggregate) = node.children().find(|n| n.kind() == SyntaxKind::AGGREGATE) else {
        return;
    };
    let single = aggregate
        .children()
        .filter(|n| n.kind() == SyntaxKind::ELEMENT_ASSOCIATION)
        .collect::<Vec<_>>();
    if single.len() == 1
        && single[0]
            .children()
            .next()
            .is_some_and(|n| n.kind() == SyntaxKind::RANGE_EXPR)
    {
        out.push(Finding::new(
            LITERAL_FORM,
            aggregate.text_range(),
            "a range in parentheses is an aggregate, not a range; remove the parentheses",
        ));
    }
}

// ---- E35: vector directions ---------------------------------------------------------------

fn range_direction(indication: &SyntaxNode, ascending: bool, out: &mut Vec<Finding>) {
    let Some(call) = indication
        .children()
        .find(|n| n.kind() == SyntaxKind::CALL_EXPR)
    else {
        return;
    };
    let Some(mark) = call.children().next() else {
        return;
    };
    let name = mark.text().to_string().to_ascii_lowercase();
    let name = name.rsplit('.').next().unwrap_or("").trim();
    if !matches!(
        name,
        "std_logic_vector"
            | "std_ulogic_vector"
            | "unsigned"
            | "signed"
            | "bit_vector"
            | "u_unsigned"
            | "u_signed"
    ) {
        return;
    }
    let Some(range) = call
        .descendants()
        .find(|n| n.kind() == SyntaxKind::RANGE_EXPR)
    else {
        return;
    };
    let (wrong, right) = if ascending {
        (SyntaxKind::DOWNTO_KW, "to")
    } else {
        (SyntaxKind::TO_KW, "downto")
    };
    if let Some(t) = range.child_token(|k| k == wrong) {
        out.push(Finding::new(
            RANGE_DIRECTION,
            t.text_range(),
            format!("vectors in this project use `{right}`"),
        ));
    }
}

// ---- E39: prohibited constructs --------------------------------------------------------------

fn prohibited_attribute(node: &SyntaxNode, options: &Options, out: &mut Vec<Finding>) {
    let Some(attribute) = node
        .children_with_tokens()
        .filter_map(|e| e.into_token())
        .skip_while(|t| t.kind() != SyntaxKind::TICK)
        .nth(1)
    else {
        return;
    };
    if options
        .attributes
        .contains(&attribute.text().to_ascii_lowercase())
    {
        out.push(Finding::new(
            PROHIBITED,
            attribute.text_range(),
            format!(
                "the attribute `'{}` is prohibited in this project",
                attribute.text()
            ),
        ));
    }
}

// ---- E40: pragmas --------------------------------------------------------------------------

const PRAGMA_PREFIXES: &[&str] = &[
    "pragma",
    "synthesis",
    "synopsys",
    "rtl_synthesis",
    "exemplar",
    "cadence",
];
const KNOWN_PRAGMAS: &[&str] = &[
    "translate_off",
    "translate_on",
    "synthesis_off",
    "synthesis_on",
    "off",
    "on",
    "read_comments_as_hdl",
    "full_case",
    "parallel_case",
    "coverage_off",
    "coverage_on",
    "keep",
    "preserve",
    "dc_script_begin",
    "dc_script_end",
    "resolution_method",
    "map_to_entity",
    "map_to_operator",
    "return_port_name",
    "template",
    "label_applies_to",
    "built_in",
    "infer_mux",
    "infer_multibit",
    "dont_infer_multibit",
    "sync_set_reset",
    "async_set_reset",
    "one_hot",
    "one_cold",
    "enum",
    "state_vector",
    "attribute",
];

fn pragma(
    token: &vfl_syntax::SyntaxToken,
    unknown: bool,
    prohibited: &[String],
    check_prohibited: bool,
    out: &mut Vec<Finding>,
) {
    let text = token.text();
    let body = text.trim_start_matches('-').trim_start();
    let mut words = body.split_whitespace();
    let Some(prefix) = words.next() else { return };
    let prefix_key = prefix.to_ascii_lowercase();
    if !PRAGMA_PREFIXES.contains(&prefix_key.as_str()) {
        return;
    }
    let Some(word) = words.next() else { return };
    let key = word.to_ascii_lowercase();
    let start = token.text_range().start()
        + TextSize::from((text.len() - body.len()) as u32 + body.find(word).unwrap_or(0) as u32);
    let range = TextRange::at(start, (word.len() as u32).into());
    if check_prohibited && prohibited.contains(&key) {
        out.push(Finding::new(
            PROHIBITED,
            range,
            format!("the pragma `{word}` is prohibited in this project"),
        ));
        return;
    }
    // `-- synthesis` and `-- pragma` comments are not always directives:
    // only words that look like one (`translate_of`).
    let looks_like = key.contains('_') || key.ends_with("off") || key.ends_with("on");
    if unknown && looks_like && !KNOWN_PRAGMAS.contains(&key.as_str()) {
        let suggestion = vfl_vendor::near(&key, KNOWN_PRAGMAS.iter().copied())
            .map(|s| format!("; did you mean `{s}`?"))
            .unwrap_or_default();
        out.push(Finding::new(
            UNKNOWN_PRAGMA,
            range,
            format!("unknown pragma `{prefix} {word}`: tools ignore it{suggestion}"),
        ));
    }
}

// ---- E42: labels -----------------------------------------------------------------------------

fn missing_label(node: &SyntaxNode, statements: &[String], out: &mut Vec<Finding>) {
    let what = match node.kind() {
        SyntaxKind::LOOP_STMT => "loop",
        SyntaxKind::IF_STMT => "if",
        _ => "case",
    };
    if !statements.iter().any(|s| s == what)
        || node.children().any(|n| n.kind() == SyntaxKind::LABEL)
    {
        return;
    }
    let keyword = node
        .children_with_tokens()
        .filter_map(|e| e.into_token())
        .find(|t| !t.kind().is_trivia())
        .map_or_else(|| node.text_range(), |t| t.text_range());
    out.push(Finding::new(
        MISSING_LABEL,
        keyword,
        format!("`{what}` statement without a label"),
    ));
}

//! What other files see of a Verilog file: its modules with their
//! parameters, ports and instances.
//!
//! [`modules`] reads them off a syntax tree, for binding VHDL instances to
//! Verilog modules and for documentation. Port declarations are found
//! whether they are in the port list (`input logic [7:0] a`) or in the
//! module body (`module m(a); input [7:0] a;`).

use crate::{SyntaxKind, SyntaxNode, SyntaxToken, TextRange};

use rustc_hash::FxHashMap;

/// The direction of a port.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Direction {
    Input,
    Output,
    Inout,
    /// `ref`.
    Ref,
}

impl Direction {
    pub fn as_str(self) -> &'static str {
        match self {
            Direction::Input => "input",
            Direction::Output => "output",
            Direction::Inout => "inout",
            Direction::Ref => "ref",
        }
    }

    fn from_token(kind: SyntaxKind) -> Option<Direction> {
        Some(match kind {
            SyntaxKind::INPUT_KW => Direction::Input,
            SyntaxKind::OUTPUT_KW => Direction::Output,
            SyntaxKind::INOUT_KW => Direction::Inout,
            SyntaxKind::REF_KW => Direction::Ref,
            _ => return None,
        })
    }
}

/// A module.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Module {
    pub name: String,
    pub name_range: TextRange,
    /// From `module` to `endmodule`.
    pub range: TextRange,
    /// From `module` to the `;` after the ports.
    pub header: TextRange,
    /// The comment lines right before the module.
    pub description: String,
    /// The parameters an instance can set, in order: those of the
    /// `#(...)` list, or the `parameter` declarations of the body when
    /// there is no list.
    pub parameters: Vec<Parameter>,
    pub ports: Vec<Port>,
    pub instances: Vec<Instance>,
}

/// A parameter.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Parameter {
    pub name: String,
    pub name_range: TextRange,
    /// The declaration: from `parameter` (or the type) to the default.
    pub range: TextRange,
    /// The type as written, white space collapsed; empty when none is
    /// written. `type` for a type parameter.
    pub ty: String,
    /// The default value as written.
    pub default: Option<String>,
    /// The default value, when it is a literal integer expression.
    pub value: Option<i64>,
    pub description: String,
}

/// A port.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Port {
    pub name: String,
    pub name_range: TextRange,
    /// The declaration the port's direction comes from.
    pub range: TextRange,
    /// `None` for an interface port, or a port whose direction is not
    /// declared.
    pub direction: Option<Direction>,
    /// The type as written, white space collapsed, with any unpacked
    /// dimensions after the name: `logic [7:0]`, `wire signed [3:0]`,
    /// `axi_if.master`. Empty when none is written.
    pub ty: String,
    /// The number of bits of a vector whose dimensions are literal
    /// integer expressions: 8 for `[7:0]`. `None` for a single bit, for a
    /// width that depends on parameters and for other types.
    pub width: Option<u32>,
    /// The default value of an input port (`input logic a = 1'b0`).
    pub default: Option<String>,
    /// An interface port (`axi_if.master bus`, `interface bus`).
    pub interface: bool,
    pub description: String,
}

impl Port {
    /// The declaration as it would be written in a port list:
    /// `input logic [7:0] a`.
    pub fn declaration(&self) -> String {
        let mut out = String::new();
        if let Some(d) = self.direction {
            out.push_str(d.as_str());
            out.push(' ');
        }
        if !self.ty.is_empty() {
            out.push_str(&self.ty);
            out.push(' ');
        }
        out.push_str(&self.name);
        if let Some(default) = &self.default {
            out.push_str(" = ");
            out.push_str(default);
        }
        out
    }
}

impl Parameter {
    /// The declaration as it would be written in a parameter list:
    /// `parameter int W = 8`.
    pub fn declaration(&self) -> String {
        let mut out = String::from("parameter ");
        if !self.ty.is_empty() {
            out.push_str(&self.ty);
            out.push(' ');
        }
        out.push_str(&self.name);
        if let Some(default) = &self.default {
            out.push_str(" = ");
            out.push_str(default);
        }
        out
    }
}

/// An instance of a module (or of an interface or a primitive: modules
/// are not told apart from them here).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Instance {
    /// The instantiated module's name.
    pub module: String,
    pub module_range: TextRange,
    pub name: String,
    pub name_range: TextRange,
    /// `(parameter, value)`; the parameter is empty for an ordered one.
    pub parameters: Vec<(String, String)>,
    /// `(port, expression)`; the port is empty for an ordered connection,
    /// `*` for `.*`; the expression is empty for an unconnected port
    /// (`.p()`).
    pub connections: Vec<(String, String)>,
}

/// The modules declared at the top level of a file, in order.
pub fn modules(root: &SyntaxNode) -> Vec<Module> {
    root.children()
        .filter(|n| n.kind() == SyntaxKind::MODULE_DECL)
        .filter_map(|n| module(&n))
        .collect()
}

fn module(node: &SyntaxNode) -> Option<Module> {
    let name = child(node, SyntaxKind::NAME).and_then(|n| name_token(&n))?;
    let param_list = child(node, SyntaxKind::PARAM_PORT_LIST);
    let port_list = child(node, SyntaxKind::PORT_LIST);

    let mut parameters = Vec::new();
    match &param_list {
        Some(list) => {
            // `#(parameter A = 1, B = 2)`: `B` is a parameter too; a
            // `localparam` keyword makes the ones after it local, up to
            // the next `parameter`.
            let mut local = false;
            let mut ty: Option<SyntaxNode> = None;
            for decl in list
                .children()
                .filter(|n| n.kind() == SyntaxKind::PARAM_DECL)
            {
                match first_keyword(&decl) {
                    Some(SyntaxKind::LOCALPARAM_KW) => local = true,
                    Some(SyntaxKind::PARAMETER_KW) => local = false,
                    _ => {}
                }
                if let Some(t) = type_of(&decl) {
                    ty = Some(t);
                } else if first_keyword(&decl).is_some() {
                    ty = None;
                }
                if !local {
                    parameters.extend(param_decl(&decl, ty.as_ref()));
                }
            }
        }
        None => {
            for decl in body_items(node).filter(|n| n.kind() == SyntaxKind::PARAM_DECL) {
                if first_keyword(&decl) == Some(SyntaxKind::PARAMETER_KW) {
                    let ty = type_of(&decl);
                    parameters.extend(param_decl(&decl, ty.as_ref()));
                }
            }
        }
    }

    let mut ports = Vec::new();
    if let Some(list) = &port_list {
        let items: Vec<SyntaxNode> = list
            .children()
            .filter(|n| n.kind() == SyntaxKind::PORT)
            .collect();
        let ansi = items
            .first()
            .is_some_and(|p| direction_of(p).is_some() || type_of(p).is_some());
        if ansi {
            ansi_ports(&items, &mut ports);
        } else {
            body_ports(node, &items, &mut ports);
        }
    }

    let mut instances = Vec::new();
    for inst in node
        .descendants()
        .filter(|n| n.kind() == SyntaxKind::INSTANTIATION)
    {
        instantiation(&inst, &mut instances);
    }

    let header_end = node
        .children_with_tokens()
        .find(|e| e.kind() == SyntaxKind::SEMI)
        .map_or(node.text_range().end(), |e| e.text_range().end());
    Some(Module {
        name: name.text().to_owned(),
        name_range: name.text_range(),
        range: node.text_range(),
        header: TextRange::new(node.text_range().start(), header_end),
        description: leading_comment(node),
        parameters,
        ports,
        instances,
    })
}

/// The items of a module body, also inside `generate` regions (but not in
/// generate blocks, whose declarations are local).
fn body_items(module: &SyntaxNode) -> impl Iterator<Item = SyntaxNode> {
    module.children().flat_map(|n| {
        if n.kind() == SyntaxKind::GENERATE_REGION {
            n.children().collect::<Vec<_>>()
        } else {
            vec![n]
        }
    })
}

fn param_decl(decl: &SyntaxNode, ty: Option<&SyntaxNode>) -> Vec<Parameter> {
    let is_type = decl
        .children_with_tokens()
        .any(|e| e.kind() == SyntaxKind::TYPE_KW);
    let ty = if is_type {
        "type".to_owned()
    } else {
        ty.map(collapsed).unwrap_or_default()
    };
    decl.children()
        .filter(|n| n.kind() == SyntaxKind::DECLARATOR)
        .filter_map(|d| {
            let name = child(&d, SyntaxKind::NAME).and_then(|n| name_token(&n))?;
            let default = initial_value(&d);
            Some(Parameter {
                name: name.text().to_owned(),
                name_range: name.text_range(),
                range: TextRange::new(decl.text_range().start(), d.text_range().end()),
                ty: ty.clone(),
                value: default.as_ref().and_then(eval),
                default: default.as_ref().map(collapsed),
                description: describe(&d, decl),
            })
        })
        .collect()
}

/// Ports declared in the port list. A port without a direction or a type
/// takes those of the port before it.
fn ansi_ports(items: &[SyntaxNode], ports: &mut Vec<Port>) {
    let mut direction = None;
    let mut ty: Option<SyntaxNode> = None;
    for item in items {
        let Some(name) = child(item, SyntaxKind::NAME).and_then(|n| name_token(&n)) else {
            continue;
        };
        let own_direction = direction_of(item);
        let own_type = type_of(item);
        if own_direction.is_some() || own_type.is_some() {
            direction = own_direction.or(if own_type.is_some() { direction } else { None });
            ty = own_type;
        }
        let interface = ty.as_ref().is_some_and(is_interface_type);
        let unpacked = unpacked_dims(item);
        let mut ty_text = ty.as_ref().map(collapsed).unwrap_or_default();
        if !unpacked.is_empty() {
            ty_text = format!("{ty_text} {unpacked}").trim().to_owned();
        }
        let width = if unpacked.is_empty() {
            ty.as_ref().and_then(packed_width)
        } else {
            None
        };
        let default = initial_value(item);
        ports.push(Port {
            name: name.text().to_owned(),
            name_range: name.text_range(),
            range: item.text_range(),
            direction: if interface { None } else { direction },
            ty: ty_text,
            width,
            default: default.as_ref().map(collapsed),
            interface,
            description: describe(item, item),
        });
    }
}

/// Ports whose directions are declared in the module body.
fn body_ports(module: &SyntaxNode, items: &[SyntaxNode], ports: &mut Vec<Port>) {
    // Name -> (direction, type, declarator, declaration).
    let mut declared: FxHashMap<
        String,
        (
            Option<Direction>,
            Option<SyntaxNode>,
            SyntaxNode,
            SyntaxNode,
        ),
    > = FxHashMap::default();
    // Types given by a separate net or variable declaration: `output c;
    // reg [3:0] c;`.
    let mut types: FxHashMap<String, SyntaxNode> = FxHashMap::default();
    for item in body_items(module) {
        let kind = item.kind();
        if !matches!(
            kind,
            SyntaxKind::PORT_DECL | SyntaxKind::NET_DECL | SyntaxKind::DATA_DECL
        ) {
            continue;
        }
        let direction = direction_of(&item);
        let ty = type_of(&item);
        for d in item
            .children()
            .filter(|n| n.kind() == SyntaxKind::DECLARATOR)
        {
            let Some(name) = child(&d, SyntaxKind::NAME).and_then(|n| name_token(&n)) else {
                continue;
            };
            let name = name.text().to_owned();
            if kind == SyntaxKind::PORT_DECL {
                declared
                    .entry(name)
                    .or_insert((direction, ty.clone(), d.clone(), item.clone()));
            } else if let Some(ty) = &ty {
                types.entry(name).or_insert(ty.clone());
            }
        }
    }
    for item in items {
        // `a`, or `.a(x)`: the external name is the port's.
        let Some(name) = child(item, SyntaxKind::NAME).and_then(|n| name_token(&n)) else {
            continue;
        };
        let key = name.text();
        let (direction, ty, declarator, declaration) = match declared.get(key) {
            Some((dir, ty, d, decl)) => (*dir, ty.clone(), Some(d), Some(decl)),
            None => (None, None, None, None),
        };
        // The type of the port declaration if it has one with a width,
        // else that of a separate declaration.
        let ty = match (ty, types.get(key)) {
            (Some(t), Some(other)) if packed_width(&t).is_none() && !has_dimension(&t) => {
                Some(other.clone())
            }
            (None, Some(other)) => Some(other.clone()),
            (t, _) => t,
        };
        let unpacked = declarator.map(unpacked_dims).unwrap_or_default();
        let mut ty_text = ty.as_ref().map(collapsed).unwrap_or_default();
        if !unpacked.is_empty() {
            ty_text = format!("{ty_text} {unpacked}").trim().to_owned();
        }
        let (name_range, range) = match (declarator, declaration) {
            (Some(d), Some(decl)) => (
                child(d, SyntaxKind::NAME)
                    .and_then(|n| name_token(&n))
                    .map_or(name.text_range(), |t| t.text_range()),
                decl.text_range(),
            ),
            _ => (name.text_range(), item.text_range()),
        };
        ports.push(Port {
            name: key.to_owned(),
            name_range,
            range,
            direction,
            width: if unpacked.is_empty() {
                ty.as_ref().and_then(packed_width)
            } else {
                None
            },
            ty: ty_text,
            default: None,
            interface: false,
            description: match (declarator, declaration) {
                (Some(d), Some(decl)) => describe(d, decl),
                _ => describe(item, item),
            },
        });
    }
}

fn instantiation(node: &SyntaxNode, out: &mut Vec<Instance>) {
    // Gates and primitives start with a keyword.
    let Some(module) = child(node, SyntaxKind::NAME_REF) else {
        return;
    };
    let Some(module_name) = module
        .children_with_tokens()
        .filter_map(|e| e.into_token())
        .filter(|t| matches!(t.kind(), SyntaxKind::IDENT | SyntaxKind::ESC_IDENT))
        .last()
    else {
        return;
    };
    let parameters = child(node, SyntaxKind::PARAM_VALUES)
        .map(|p| connections(&p))
        .unwrap_or_default();
    for inst in node.children().filter(|n| n.kind() == SyntaxKind::INSTANCE) {
        let Some(name) = child(&inst, SyntaxKind::NAME).and_then(|n| name_token(&n)) else {
            continue;
        };
        out.push(Instance {
            module: module_name.text().to_owned(),
            module_range: module_name.text_range(),
            name: name.text().to_owned(),
            name_range: name.text_range(),
            parameters: parameters.clone(),
            connections: connections(&inst),
        });
    }
}

fn connections(node: &SyntaxNode) -> Vec<(String, String)> {
    node.children()
        .filter(|n| n.kind() == SyntaxKind::CONNECTION)
        .map(|c| {
            let star = c
                .children_with_tokens()
                .any(|e| e.kind() == SyntaxKind::DOT_STAR || e.kind() == SyntaxKind::STAR);
            let name = child(&c, SyntaxKind::NAME).and_then(|n| name_token(&n));
            let value = c
                .children()
                .find(|n| n.kind() != SyntaxKind::NAME)
                .map(|n| collapsed(&n))
                .unwrap_or_default();
            let parens = c
                .children_with_tokens()
                .any(|e| e.kind() == SyntaxKind::LPAREN);
            match name {
                // `.clk`: connected to `clk`.
                Some(name) if !parens => (name.text().to_owned(), name.text().to_owned()),
                Some(name) => (name.text().to_owned(), value),
                None if star => ("*".to_owned(), String::new()),
                None => (String::new(), value),
            }
        })
        .collect()
}

// ---- Helpers ---------------------------------------------------------------

fn child(node: &SyntaxNode, kind: SyntaxKind) -> Option<SyntaxNode> {
    node.children().find(|n| n.kind() == kind)
}

fn name_token(name: &SyntaxNode) -> Option<SyntaxToken> {
    name.children_with_tokens()
        .filter_map(|e| e.into_token())
        .find(|t| matches!(t.kind(), SyntaxKind::IDENT | SyntaxKind::ESC_IDENT))
}

fn first_keyword(node: &SyntaxNode) -> Option<SyntaxKind> {
    node.children_with_tokens()
        .filter_map(|e| e.into_token())
        .map(|t| t.kind())
        .find(|k| !k.is_trivia())
        .filter(|k| k.is_keyword())
}

fn direction_of(node: &SyntaxNode) -> Option<Direction> {
    node.children_with_tokens()
        .filter_map(|e| e.into_token())
        .find_map(|t| Direction::from_token(t.kind()))
}

fn type_of(node: &SyntaxNode) -> Option<SyntaxNode> {
    child(node, SyntaxKind::DATA_TYPE)
}

fn has_dimension(ty: &SyntaxNode) -> bool {
    ty.children().any(|n| n.kind() == SyntaxKind::DIMENSION)
}

fn is_interface_type(ty: &SyntaxNode) -> bool {
    let interface_kw = ty
        .children_with_tokens()
        .any(|e| e.kind() == SyntaxKind::INTERFACE_KW);
    // `axi_if.master`: a modport of an interface.
    let modport = child(ty, SyntaxKind::NAME_REF).is_some_and(|n| {
        n.children_with_tokens()
            .any(|e| e.kind() == SyntaxKind::DOT)
    });
    interface_kw || modport
}

/// The unpacked dimensions after a declared name, as written.
fn unpacked_dims(node: &SyntaxNode) -> String {
    node.children()
        .filter(|n| n.kind() == SyntaxKind::DIMENSION)
        .map(|n| collapsed(&n))
        .collect::<Vec<_>>()
        .join("")
}

/// The value after `=` in a declarator or a port.
fn initial_value(node: &SyntaxNode) -> Option<SyntaxNode> {
    let mut after_eq = false;
    for e in node.children_with_tokens() {
        match e {
            rowan::NodeOrToken::Token(t) if t.kind() == SyntaxKind::EQ => after_eq = true,
            rowan::NodeOrToken::Node(n) if after_eq => return Some(n),
            _ => {}
        }
    }
    None
}

/// The width of a vector type whose packed dimensions are literal: `logic
/// [7:0]`, `wire [3:0][1:0]`, `[15:0]`.
fn packed_width(ty: &SyntaxNode) -> Option<u32> {
    // A user-defined type, a struct or an enum: unknown.
    if ty.children().any(|n| {
        matches!(
            n.kind(),
            SyntaxKind::NAME_REF
                | SyntaxKind::STRUCT_TYPE
                | SyntaxKind::ENUM_TYPE
                | SyntaxKind::TYPE_REFERENCE
        )
    }) {
        return None;
    }
    let non_vector = ty.children_with_tokens().any(|e| {
        matches!(
            e.kind(),
            SyntaxKind::INTEGER_KW
                | SyntaxKind::INT_KW
                | SyntaxKind::SHORTINT_KW
                | SyntaxKind::LONGINT_KW
                | SyntaxKind::BYTE_KW
                | SyntaxKind::REAL_KW
                | SyntaxKind::REALTIME_KW
                | SyntaxKind::SHORTREAL_KW
                | SyntaxKind::TIME_KW
                | SyntaxKind::STRING_KW
                | SyntaxKind::CHANDLE_KW
                | SyntaxKind::EVENT_KW
        )
    });
    if non_vector {
        return None;
    }
    let mut width: u32 = 1;
    let mut any = false;
    for dim in ty.children().filter(|n| n.kind() == SyntaxKind::DIMENSION) {
        let bounds: Vec<SyntaxNode> = dim.children().collect();
        let colon = dim
            .children_with_tokens()
            .any(|e| e.kind() == SyntaxKind::COLON);
        let [msb, lsb] = bounds.as_slice() else {
            return None;
        };
        if !colon {
            return None;
        }
        let (msb, lsb) = (eval(msb)?, eval(lsb)?);
        let size = u32::try_from(msb.abs_diff(lsb)).ok()?.checked_add(1)?;
        width = width.checked_mul(size)?;
        any = true;
    }
    any.then_some(width)
}

/// The value of a literal integer expression: numbers, parentheses and
/// arithmetic, no names.
fn eval(expr: &SyntaxNode) -> Option<i64> {
    eval_at(expr, 0)
}

fn eval_at(expr: &SyntaxNode, depth: u32) -> Option<i64> {
    if depth > 32 {
        return None;
    }
    let tokens = || {
        expr.children_with_tokens()
            .filter_map(|e| e.into_token())
            .filter(|t| !t.kind().is_trivia())
    };
    match expr.kind() {
        SyntaxKind::LITERAL => {
            let token = tokens().next()?;
            number(token.kind(), token.text())
        }
        SyntaxKind::PAREN_EXPR => eval_at(&expr.children().next()?, depth + 1),
        SyntaxKind::UNARY_EXPR => {
            let op = tokens().next()?.kind();
            let value = eval_at(&expr.children().next()?, depth + 1)?;
            match op {
                SyntaxKind::MINUS => value.checked_neg(),
                SyntaxKind::PLUS => Some(value),
                _ => None,
            }
        }
        SyntaxKind::BINARY_EXPR => {
            let mut operands = expr.children();
            let (l, r) = (operands.next()?, operands.next()?);
            let op = tokens().next()?.kind();
            let (l, r) = (eval_at(&l, depth + 1)?, eval_at(&r, depth + 1)?);
            match op {
                SyntaxKind::PLUS => l.checked_add(r),
                SyntaxKind::MINUS => l.checked_sub(r),
                SyntaxKind::STAR => l.checked_mul(r),
                SyntaxKind::SLASH => l.checked_div(r),
                SyntaxKind::PERCENT => l.checked_rem(r),
                SyntaxKind::STAR2 => u32::try_from(r).ok().and_then(|r| l.checked_pow(r)),
                SyntaxKind::SHL | SyntaxKind::ASHL => {
                    u32::try_from(r).ok().and_then(|r| l.checked_shl(r))
                }
                SyntaxKind::SHR | SyntaxKind::ASHR => {
                    u32::try_from(r).ok().and_then(|r| l.checked_shr(r))
                }
                _ => None,
            }
        }
        _ => None,
    }
}

/// The value of a number without `x`, `z` or `?` digits.
fn number(kind: SyntaxKind, text: &str) -> Option<i64> {
    let text: String = text.chars().filter(|&c| c != '_').collect();
    match kind {
        SyntaxKind::INT_NUMBER => text.parse().ok(),
        SyntaxKind::BASED_NUMBER => {
            let tick = text.find('\'')?;
            let rest = text[tick + 1..].trim_start();
            let rest = rest.strip_prefix(['s', 'S']).unwrap_or(rest);
            let mut chars = rest.chars();
            let radix = match chars.next()?.to_ascii_lowercase() {
                'b' => 2,
                'o' => 8,
                'd' => 10,
                'h' => 16,
                _ => return None,
            };
            let digits: String = chars
                .as_str()
                .chars()
                .filter(|c| !c.is_whitespace())
                .collect();
            i64::from_str_radix(&digits, radix).ok()
        }
        _ => None,
    }
}

/// The text of a node with runs of white space and comments collapsed to
/// one space.
fn collapsed(node: &SyntaxNode) -> String {
    let mut out = String::new();
    let mut space = false;
    for token in node
        .descendants_with_tokens()
        .filter_map(|e| e.into_token())
    {
        if token.kind().is_trivia() {
            space = !out.is_empty();
            continue;
        }
        if space {
            out.push(' ');
            space = false;
        }
        out.push_str(token.text());
    }
    out
}

/// The description of a declaration: the comment lines right before it
/// (before `before`), else a comment after it on the same line.
fn describe(node: &SyntaxNode, before: &SyntaxNode) -> String {
    let leading = leading_comment(before);
    if !leading.is_empty() {
        return leading;
    }
    trailing_comment(node)
}

/// The `//` comment lines right before a node, with no blank line between.
fn leading_comment(node: &SyntaxNode) -> String {
    let Some(first) = node.first_token() else {
        return String::new();
    };
    let mut lines = Vec::new();
    let mut token = first.prev_token();
    while let Some(t) = token {
        match t.kind() {
            SyntaxKind::WHITESPACE => {
                if t.text().matches('\n').count() > 1 {
                    break;
                }
            }
            // A comment after code on its line belongs to that code.
            SyntaxKind::LINE_COMMENT if !alone_on_line(&t) => break,
            SyntaxKind::LINE_COMMENT => lines.push(comment_text(t.text())),
            SyntaxKind::ATTRIBUTE => {}
            _ => break,
        }
        token = t.prev_token();
    }
    lines.reverse();
    lines.join("\n").trim().to_owned()
}

/// Whether nothing but white space comes before a token on its line.
fn alone_on_line(token: &SyntaxToken) -> bool {
    let mut prev = token.prev_token();
    while let Some(t) = prev {
        match t.kind() {
            SyntaxKind::WHITESPACE if t.text().contains('\n') => return true,
            SyntaxKind::WHITESPACE => {}
            _ => return false,
        }
        prev = t.prev_token();
    }
    true
}

/// A `//` comment after a node on the same line, past a `,` or `;`.
fn trailing_comment(node: &SyntaxNode) -> String {
    let mut token = node.last_token().and_then(|t| t.next_token());
    while let Some(t) = token {
        match t.kind() {
            SyntaxKind::WHITESPACE if !t.text().contains('\n') => {}
            SyntaxKind::COMMA | SyntaxKind::SEMI => {}
            SyntaxKind::LINE_COMMENT => return comment_text(t.text()),
            _ => break,
        }
        token = t.next_token();
    }
    String::new()
}

fn comment_text(text: &str) -> String {
    text.trim_start_matches('/')
        .trim_start_matches('!')
        .trim()
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Standard, parse};

    fn summary(text: &str) -> Vec<Module> {
        let parse = parse(text, Standard::Sv2017);
        assert!(parse.errors().is_empty(), "{:?}", parse.errors());
        modules(&parse.syntax())
    }

    #[test]
    fn ansi_ports_inherit_direction_and_type() {
        let text = "// A register.\n\
             module foo #(parameter int W = 8, N, localparam L = 2) (\n\
               input  logic         clk, // The clock.\n\
               input  logic [W-1:0] d, e,\n\
               output logic [7:0]   q,\n\
               inout  wire          io,\n\
               my_if.master         bus,\n\
               input                x = 1'b0\n\
             );\n\
             endmodule\n";
        let m = &summary(text)[0];
        assert_eq!(m.name, "foo");
        assert_eq!(
            m.header.len(),
            (text.find(");").unwrap() + 2 - 15).try_into().unwrap()
        );
        assert_eq!(m.description, "A register.");
        let params: Vec<(&str, &str, Option<i64>)> = m
            .parameters
            .iter()
            .map(|p| (p.name.as_str(), p.ty.as_str(), p.value))
            .collect();
        assert_eq!(params, [("W", "int", Some(8)), ("N", "int", None)]);
        let ports: Vec<(&str, Option<Direction>, &str, Option<u32>)> = m
            .ports
            .iter()
            .map(|p| (p.name.as_str(), p.direction, p.ty.as_str(), p.width))
            .collect();
        assert_eq!(
            ports,
            [
                ("clk", Some(Direction::Input), "logic", None),
                ("d", Some(Direction::Input), "logic [W-1:0]", None),
                ("e", Some(Direction::Input), "logic [W-1:0]", None),
                ("q", Some(Direction::Output), "logic [7:0]", Some(8)),
                ("io", Some(Direction::Inout), "wire", None),
                ("bus", None, "my_if.master", None),
                ("x", Some(Direction::Input), "", None),
            ]
        );
        assert_eq!(m.ports[0].description, "The clock.");
        assert_eq!(m.ports[1].description, "");
        assert!(m.ports[5].interface);
        assert_eq!(m.ports[6].default.as_deref(), Some("1'b0"));
        assert_eq!(m.ports[3].declaration(), "output logic [7:0] q");
    }

    #[test]
    fn body_ports_take_their_declarations() {
        let m = &summary(
            "module old(a, b, c, d);\n\
               parameter P = 3, Q = 4'hf;\n\
               localparam L = 1;\n\
               input a;\n\
               input [3:0] b; // Data.\n\
               output c;\n\
               reg [1:0] c;\n\
               output [2*4-1:0] d;\n\
             endmodule\n",
        )[0];
        let params: Vec<(&str, Option<i64>)> = m
            .parameters
            .iter()
            .map(|p| (p.name.as_str(), p.value))
            .collect();
        assert_eq!(params, [("P", Some(3)), ("Q", Some(15))]);
        let ports: Vec<(&str, Option<Direction>, Option<u32>)> = m
            .ports
            .iter()
            .map(|p| (p.name.as_str(), p.direction, p.width))
            .collect();
        assert_eq!(
            ports,
            [
                ("a", Some(Direction::Input), None),
                ("b", Some(Direction::Input), Some(4)),
                ("c", Some(Direction::Output), Some(2)),
                ("d", Some(Direction::Output), Some(8)),
            ]
        );
        assert_eq!(m.ports[1].description, "Data.");
    }

    #[test]
    fn instances_are_listed_with_their_connections() {
        let m = &summary(
            "module top(input clk);\n\
               bar #(.P(1)) u_bar (.a(clk), .b(), .c);\n\
               bar u2 (clk), u3 (.*);\n\
               and g1 (x, y, z);\n\
               generate if (1) begin : g\n\
                 baz u4 ();\n\
               end endgenerate\n\
             endmodule\n",
        )[0];
        let instances: Vec<(&str, &str)> = m
            .instances
            .iter()
            .map(|i| (i.module.as_str(), i.name.as_str()))
            .collect();
        assert_eq!(
            instances,
            [
                ("bar", "u_bar"),
                ("bar", "u2"),
                ("bar", "u3"),
                ("baz", "u4")
            ]
        );
        assert_eq!(m.instances[0].parameters, [("P".into(), "1".into())]);
        assert_eq!(
            m.instances[0].connections,
            [
                ("a".into(), "clk".into()),
                ("b".into(), String::new()),
                ("c".into(), "c".into())
            ]
        );
        assert_eq!(m.instances[1].connections, [(String::new(), "clk".into())]);
        assert_eq!(m.instances[2].connections, [("*".into(), String::new())]);
    }
}

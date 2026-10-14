//! Widths of vectors and of the values assigned to them.
//!
//! A width is a linear expression of parameters (`W - 1 + 1` is `W`), so
//! that `logic [W-1:0] a` and `logic [W:0] b` are known to differ by one
//! bit whatever `W` is, while a width that depends on parameters in other
//! ways is unknown. An assignment is reported when the two sides' widths
//! differ by a constant: a value wider than its target is truncated, and
//! a narrower one is extended, which is reported only when no arithmetic
//! could make the extension intended (`sum = a + b` keeps the carry).

use std::collections::BTreeMap;
use std::fmt;

use vfl_sema::codes;
use vfl_verilog_syntax::{SyntaxKind as K, SyntaxNode};

use super::Unit;
use super::scope::{self, Kind, ident, tokens};
use crate::Finding;

/// `constant + Σ factor × name`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Linear {
    constant: i64,
    terms: BTreeMap<String, i64>,
}

impl Linear {
    fn constant(value: i64) -> Linear {
        Linear {
            constant: value,
            terms: BTreeMap::new(),
        }
    }

    fn symbol(name: String) -> Linear {
        Linear {
            constant: 0,
            terms: BTreeMap::from([(name, 1)]),
        }
    }

    pub fn as_constant(&self) -> Option<i64> {
        self.terms.is_empty().then_some(self.constant)
    }

    fn add(&self, other: &Linear, sign: i64) -> Linear {
        let mut out = self.clone();
        out.constant = out
            .constant
            .saturating_add(other.constant.saturating_mul(sign));
        for (name, factor) in &other.terms {
            let entry = out.terms.entry(name.clone()).or_insert(0);
            *entry = entry.saturating_add(factor.saturating_mul(sign));
        }
        out.terms.retain(|_, f| *f != 0);
        out
    }

    fn scale(&self, by: i64) -> Linear {
        let mut out = self.clone();
        out.constant = out.constant.saturating_mul(by);
        for f in out.terms.values_mut() {
            *f = f.saturating_mul(by);
        }
        out.terms.retain(|_, f| *f != 0);
        out
    }
}

impl From<u32> for Linear {
    fn from(value: u32) -> Self {
        Linear::constant(value.into())
    }
}

impl fmt::Display for Linear {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut first = true;
        for (name, factor) in &self.terms {
            let sign = if *factor < 0 {
                "-"
            } else if first {
                ""
            } else {
                "+"
            };
            let magnitude = factor.unsigned_abs();
            if magnitude == 1 {
                write!(f, "{sign}{name}")?;
            } else {
                write!(f, "{sign}{magnitude}*{name}")?;
            }
            first = false;
        }
        if first {
            write!(f, "{}", self.constant)
        } else if self.constant > 0 {
            write!(f, "+{}", self.constant)
        } else if self.constant < 0 {
            write!(f, "{}", self.constant)
        } else {
            Ok(())
        }
    }
}

/// The value of an integer literal: `8`, `4'd3`, `'h1f`, `2'b10`.
pub fn literal_value(node: &SyntaxNode) -> Option<u64> {
    if node.kind() != K::LITERAL {
        return None;
    }
    let token = tokens(node).next()?;
    let text: String = token.text().chars().filter(|c| *c != '_').collect();
    match token.kind() {
        K::INT_NUMBER => text.parse().ok(),
        K::BASED_NUMBER => {
            let (_, rest) = text.split_once('\'')?;
            let rest = rest.trim_start_matches(['s', 'S']);
            let mut chars = rest.chars();
            let radix = match chars.next()?.to_ascii_lowercase() {
                'b' => 2,
                'o' => 8,
                'd' => 10,
                'h' => 16,
                _ => return None,
            };
            u64::from_str_radix(chars.as_str().trim(), radix).ok()
        }
        _ => None,
    }
}

/// The size of a sized literal: 4 for `4'b1010`.
pub fn literal_size(node: &SyntaxNode) -> Option<u64> {
    let token = tokens(node).next()?;
    if token.kind() != K::BASED_NUMBER {
        return None;
    }
    let (size, _) = token.text().split_once('\'')?;
    size.trim().replace('_', "").parse().ok()
}

/// The value of a constant expression, as a linear expression of the
/// unit's parameters. Local parameters are replaced by their values.
pub fn constant(unit: &Unit, expr: &SyntaxNode, depth: u32) -> Option<Linear> {
    if depth > 16 {
        return None;
    }
    match expr.kind() {
        K::LITERAL => literal_value(expr)
            .and_then(|v| i64::try_from(v).ok())
            .map(Linear::constant),
        K::PAREN_EXPR => constant(unit, &expr.first_child()?, depth + 1),
        K::NAME_REF => {
            let name = scope::ref_name(expr).filter(|_| scope::is_simple_ref(expr))?;
            let name = ident(&name);
            let decl = unit
                .scope
                .named(&name)
                .find(|d| matches!(d.kind, Kind::Param | Kind::LocalParam | Kind::Genvar))?;
            match decl.kind {
                Kind::LocalParam => {
                    let value = decl
                        .node
                        .children()
                        .filter(|n| n.kind() != K::NAME)
                        .last()?;
                    constant(unit, &value, depth + 1).or(Some(Linear::symbol(name)))
                }
                Kind::Genvar => None,
                _ => Some(Linear::symbol(name)),
            }
        }
        K::UNARY_EXPR => {
            let operand = constant(unit, &expr.first_child()?, depth + 1)?;
            match tokens(expr).next()?.kind() {
                K::MINUS => Some(operand.scale(-1)),
                K::PLUS => Some(operand),
                _ => None,
            }
        }
        K::BINARY_EXPR => {
            let mut children = expr.children();
            let a = constant(unit, &children.next()?, depth + 1)?;
            let b = constant(unit, &children.next()?, depth + 1)?;
            match tokens(expr).next()?.kind() {
                K::PLUS => Some(a.add(&b, 1)),
                K::MINUS => Some(a.add(&b, -1)),
                K::STAR => match (a.as_constant(), b.as_constant()) {
                    (Some(x), _) => Some(b.scale(x)),
                    (_, Some(y)) => Some(a.scale(y)),
                    _ => None,
                },
                operator => {
                    let (x, y) = (a.as_constant()?, b.as_constant()?);
                    let value = match operator {
                        K::SLASH if y != 0 => x / y,
                        K::PERCENT if y != 0 => x % y,
                        K::SHL | K::ASHL if (0..63).contains(&y) => x.checked_shl(y as u32)?,
                        K::SHR | K::ASHR if (0..63).contains(&y) => x >> y,
                        K::STAR2 if (0..63).contains(&y) => x.checked_pow(y as u32)?,
                        _ => return None,
                    };
                    Some(Linear::constant(value))
                }
            }
        }
        K::CALL_EXPR => {
            let callee = expr.first_child()?;
            if callee.text() != "$clog2" {
                return None;
            }
            let argument = expr.children().nth(1)?.first_child()?;
            match constant(unit, &argument, depth + 1)?.as_constant() {
                Some(v) if v > 0 => {
                    Some(Linear::constant(64 - (v as u64 - 1).leading_zeros() as i64))
                }
                Some(_) => Some(Linear::constant(0)),
                None => {
                    let text: String = argument.text().to_string().split_whitespace().collect();
                    Some(Linear::symbol(format!("$clog2({text})")))
                }
            }
        }
        _ => None,
    }
}

/// The number of elements `[msb:lsb]` spans.
fn dimension(unit: &Unit, dimension: &SyntaxNode) -> Option<Linear> {
    let parts: Vec<SyntaxNode> = dimension.children().collect();
    match parts.as_slice() {
        [msb, lsb] if scope::has_token(dimension, K::COLON) => {
            let (msb, lsb) = (constant(unit, msb, 0)?, constant(unit, lsb, 0)?);
            let span = msb.add(&lsb, -1);
            match span.as_constant() {
                Some(d) => Some(Linear::constant(d.abs() + 1)),
                // `[W-1:0]` counts down, `[0:W-1]` up.
                None if lsb.as_constant() == Some(0) => Some(span.add(&Linear::constant(1), 1)),
                None if msb.as_constant() == Some(0) => {
                    Some(span.scale(-1).add(&Linear::constant(1), 1))
                }
                None => None,
            }
        }
        // `[N]`: N elements.
        [size] => constant(unit, size, 0),
        _ => None,
    }
}

/// The number of bits of a data type's packed part: `logic [7:0]` is 8,
/// `int` 32, a type the unit declares that of its definition.
fn type_width(unit: &Unit, data_type: Option<&SyntaxNode>, depth: u32) -> Option<Linear> {
    if depth > 8 {
        return None;
    }
    let Some(data_type) = data_type else {
        // A port or net with no type: one bit.
        return Some(Linear::constant(1));
    };
    let mut base = None;
    for t in tokens(data_type) {
        base = Some(match t.kind() {
            K::LOGIC_KW | K::REG_KW | K::BIT_KW => 1,
            K::BYTE_KW => 8,
            K::SHORTINT_KW => 16,
            K::INT_KW | K::INTEGER_KW => 32,
            K::LONGINT_KW | K::TIME_KW => 64,
            k if scope::is_net_keyword(k) => 1,
            K::SIGNED_KW | K::UNSIGNED_KW => continue,
            _ => return None,
        });
        if matches!(
            t.kind(),
            K::INT_KW | K::INTEGER_KW | K::BYTE_KW | K::SHORTINT_KW | K::LONGINT_KW | K::TIME_KW
        ) {
            // Integer types take no packed dimensions.
            break;
        }
    }
    let mut width = Linear::constant(base.unwrap_or(1) as i64);
    for child in data_type.children() {
        match child.kind() {
            K::DIMENSION => {
                let size = dimension(unit, &child)?;
                width = match (width.as_constant(), size.as_constant()) {
                    (Some(w), _) => size.scale(w),
                    (_, Some(s)) => width.scale(s),
                    _ => return None,
                };
            }
            K::NAME_REF if base.is_none() => {
                let name = scope::ref_name(&child).filter(|_| scope::is_simple_ref(&child))?;
                let typedef = unit
                    .scope
                    .named(&ident(&name))
                    .find(|d| d.kind == Kind::Type)?;
                let inner = type_width(unit, typedef.data_type.as_ref(), depth + 1)?;
                width = inner;
                base = Some(0);
            }
            K::ENUM_TYPE => {
                let inner = child.children().find(|n| n.kind() == K::DATA_TYPE);
                width = match inner {
                    Some(t) => type_width(unit, Some(&t), depth + 1)?,
                    None => Linear::constant(32),
                };
                base = Some(0);
            }
            K::STRUCT_TYPE => {
                if !scope::has_token(&child, K::PACKED_KW) {
                    return None;
                }
                let mut total = Linear::constant(0);
                for member in child.children().filter(|n| n.kind() == K::STRUCT_MEMBER) {
                    let member_type = member.children().find(|n| n.kind() == K::DATA_TYPE);
                    let one = type_width(unit, member_type.as_ref(), depth + 1)?;
                    let count = member
                        .children()
                        .filter(|n| n.kind() == K::DECLARATOR)
                        .count() as i64;
                    total = total.add(&one.scale(count), 1);
                }
                width = total;
                base = Some(0);
            }
            _ => return None,
        }
    }
    Some(width)
}

/// The width of a declared signal, if it is a packed value.
fn signal_width(unit: &Unit, name: &str, at: &SyntaxNode) -> Option<Linear> {
    // `output [7:0] q; reg [7:0] q;`: the declarations agree, or the one
    // with dimensions counts.
    let decls: Vec<&scope::Decl> = unit.scope.visible(name, at);
    if decls.iter().any(|d| !d.is_signal()) {
        return None;
    }
    if decls.is_empty() || decls.iter().any(|d| d.unpacked) {
        return None;
    }
    let mut widths = decls
        .iter()
        .map(|d| type_width(unit, d.data_type.as_ref(), 0));
    let first = widths.next()??;
    let mut best = first;
    for w in widths {
        let w = w?;
        if best.as_constant() == Some(1) {
            best = w;
        }
    }
    Some(best)
}

/// The signal `name` names at `at`.
fn signal_decl<'a>(unit: &'a Unit, name: &str, at: &SyntaxNode) -> Option<&'a scope::Decl> {
    unit.scope
        .visible(name, at)
        .into_iter()
        .find(|d| d.is_signal())
}

/// The width of a memory's elements.
fn element_width(unit: &Unit, name: &str, at: &SyntaxNode) -> Option<Linear> {
    let decl = signal_decl(unit, name, at)?;
    let unpacked = decl
        .node
        .children()
        .filter(|n| n.kind() == K::DIMENSION)
        .count();
    if unpacked != 1 {
        return None;
    }
    type_width(unit, decl.data_type.as_ref(), 0)
}

/// What an expression's width is made of.
#[derive(Default)]
struct Shape {
    /// It has arithmetic or a shift, whose result may be meant wider.
    arithmetic: bool,
}

/// The self-determined width of an expression.
fn width(unit: &Unit, expr: &SyntaxNode, shape: &mut Shape, depth: u32) -> Option<Linear> {
    if depth > 32 {
        return None;
    }
    match expr.kind() {
        K::LITERAL => literal_size(expr).map(|s| Linear::constant(s as i64)),
        K::PAREN_EXPR => width(unit, &expr.first_child()?, shape, depth + 1),
        K::NAME_REF => {
            let name = scope::ref_name(expr).filter(|_| scope::is_simple_ref(expr))?;
            signal_width(unit, &ident(&name), expr)
        }
        K::INDEX_EXPR => {
            let base = expr.first_child()?;
            let name = (base.kind() == K::NAME_REF && scope::is_simple_ref(&base))
                .then(|| scope::ref_name(&base))
                .flatten()
                .map(|t| ident(&t))?;
            let decl = signal_decl(unit, &name, expr)?;
            let parts: Vec<SyntaxNode> = expr.children().skip(1).collect();
            if decl.unpacked {
                // `mem[i]`: an element.
                return match parts.as_slice() {
                    [_] if !scope::has_token(expr, K::COLON) => element_width(unit, &name, expr),
                    _ => None,
                };
            }
            let packed = decl.data_type.as_ref().map_or(0, |t| {
                t.children().filter(|n| n.kind() == K::DIMENSION).count()
            });
            if packed > 1 {
                return None;
            }
            let colon = tokens(expr)
                .map(|t| t.kind())
                .find(|k| matches!(k, K::COLON | K::PLUS_COLON | K::MINUS_COLON));
            match (colon, parts.as_slice()) {
                (None, [_]) => Some(Linear::constant(1)),
                (Some(K::COLON), [msb, lsb]) => {
                    let span = constant(unit, msb, 0)?.add(&constant(unit, lsb, 0)?, -1);
                    let span = span.as_constant()?;
                    Some(Linear::constant(span.abs() + 1))
                }
                (Some(K::PLUS_COLON | K::MINUS_COLON), [_, size]) => constant(unit, size, 0),
                _ => None,
            }
        }
        K::CONCAT_EXPR => {
            let mut total = Linear::constant(0);
            for part in expr.children() {
                total = total.add(&width(unit, &part, shape, depth + 1)?, 1);
            }
            Some(total)
        }
        K::REPLICATION_EXPR => {
            let mut children = expr.children();
            let count = constant(unit, &children.next()?, 0)?;
            let inner = width(unit, &children.next()?, shape, depth + 1)?;
            match (count.as_constant(), inner.as_constant()) {
                (Some(c), _) => Some(inner.scale(c)),
                (_, Some(i)) => Some(count.scale(i)),
                _ => None,
            }
        }
        K::UNARY_EXPR => {
            let operator = tokens(expr).next()?.kind();
            match operator {
                K::TILDE | K::PLUS | K::MINUS => {
                    if operator != K::TILDE {
                        shape.arithmetic = true;
                    }
                    width(unit, &expr.first_child()?, shape, depth + 1)
                }
                // Reductions and `!`.
                _ => Some(Linear::constant(1)),
            }
        }
        K::BINARY_EXPR => {
            let operator = tokens(expr).next()?.kind();
            let mut children = expr.children();
            let (a, b) = (children.next()?, children.next()?);
            match operator {
                K::EQ2
                | K::NEQ
                | K::EQ3
                | K::NEQ2
                | K::EQ2_Q
                | K::NEQ_Q
                | K::LT
                | K::GT
                | K::LTE
                | K::GTE
                | K::AMP2
                | K::PIPE2
                | K::ARROW
                | K::EQUIV => Some(Linear::constant(1)),
                K::SHL | K::SHR | K::ASHL | K::ASHR | K::STAR2 => {
                    shape.arithmetic = true;
                    width(unit, &a, shape, depth + 1)
                }
                _ => {
                    if !matches!(operator, K::AMP | K::PIPE | K::CARET | K::TILDE_CARET) {
                        shape.arithmetic = true;
                    }
                    let wa = width(unit, &a, shape, depth + 1)?;
                    let wb = width(unit, &b, shape, depth + 1)?;
                    larger(wa, wb)
                }
            }
        }
        K::CONDITIONAL_EXPR => {
            let mut children = expr.children().skip(1);
            let wa = width(unit, &children.next()?, shape, depth + 1)?;
            let wb = width(unit, &children.next()?, shape, depth + 1)?;
            larger(wa, wb)
        }
        K::CALL_EXPR => {
            let callee = expr.first_child()?.text().to_string();
            match callee.as_str() {
                "$signed" | "$unsigned" => {
                    // Sign extension is what `$signed` is for.
                    shape.arithmetic = true;
                    let argument = expr.children().nth(1)?.first_child()?;
                    width(unit, &argument, shape, depth + 1)
                }
                _ => None,
            }
        }
        K::CAST_EXPR => {
            // `8'(x)`: a size cast.
            let size = expr.first_child()?;
            if size.kind() == K::NAME_REF
                && unit
                    .scope
                    .named(&size.text().to_string())
                    .any(|d| d.kind == Kind::Type)
            {
                return None;
            }
            constant(unit, &size, 0)
        }
        _ => None,
    }
}

/// The wider of two widths, when that is known.
fn larger(a: Linear, b: Linear) -> Option<Linear> {
    if a == b {
        return Some(a);
    }
    let d = a.add(&b, -1).as_constant()?;
    Some(if d >= 0 { a } else { b })
}

/// The width of an expression, when it is a constant.
pub fn constant_width(unit: &Unit, expr: &SyntaxNode) -> Option<u32> {
    width(unit, expr, &mut Shape::default(), 0)?
        .as_constant()
        .and_then(|w| u32::try_from(w).ok())
}

/// The width of a connection's expression and whether it is made by
/// arithmetic.
pub fn value_width(unit: &Unit, expr: &SyntaxNode) -> Option<(Linear, bool)> {
    if has_macro(expr) {
        return None;
    }
    let mut shape = Shape::default();
    let w = width(unit, expr, &mut shape, 0)?;
    Some((w, shape.arithmetic))
}

/// The message for a target `what` of width `target` given a value of
/// width `value`, if they differ in a way worth reporting.
/// Arithmetic, shifts and `$signed` make a width that may be meant to
/// grow or be cut (a carry, modular arithmetic, sign extension), and a
/// single bit made wider is a flag extended with zeros on purpose.
pub fn mismatch(target: &Linear, value: &Linear, arithmetic: bool) -> Option<(String, String)> {
    let difference = target.add(value, -1).as_constant()?;
    if difference == 0 || arithmetic || (difference > 0 && value.as_constant() == Some(1)) {
        return None;
    }
    Some((target.to_string(), value.to_string()))
}

/// Whether an expression is made of literals only.
fn literals_only(expr: &SyntaxNode) -> bool {
    match expr.kind() {
        K::LITERAL => true,
        K::CONCAT_EXPR | K::REPLICATION_EXPR | K::PAREN_EXPR => {
            expr.children().all(|c| literals_only(&c))
        }
        _ => false,
    }
}

fn plural(width: &str) -> &'static str {
    if width == "1" { "bit" } else { "bits" }
}

pub fn check(unit: &Unit, enabled: &impl Fn(&str) -> bool, out: &mut Vec<Finding>) {
    if !enabled(codes::WIDTH_MISMATCH) {
        return;
    }
    for node in unit.node.descendants() {
        let (target, value) = match node.kind() {
            K::BLOCKING_ASSIGN | K::NONBLOCKING_ASSIGN => {
                let mut children = node.children();
                match (children.next(), children.next()) {
                    (Some(t), Some(v)) => (t, v),
                    _ => continue,
                }
            }
            K::ASSIGN_EXPR
                if node
                    .parent()
                    .is_some_and(|p| p.kind() == K::CONTINUOUS_ASSIGN) =>
            {
                let mut children = node.children();
                match (children.next(), children.next()) {
                    (Some(t), Some(v)) => (t, v),
                    _ => continue,
                }
            }
            _ => continue,
        };
        // A macro may expand to anything; `{q, d}` into `q` shifts.
        if has_macro(&node) || shifts_into(&target, &value) || integer(unit, &value) {
            continue;
        }
        let mut shape = Shape::default();
        let Some(target_width) = width(unit, &target, &mut Shape::default(), 0) else {
            continue;
        };
        if literals_only(&value) {
            // A constant is reported only when it does not fit.
            let fits = match (target_width.as_constant(), literal_value(&value)) {
                (Some(bits), Some(v)) => bits >= 64 || v < 1u64 << bits.max(0),
                _ => true,
            };
            if fits {
                continue;
            }
        }
        let Some(value_width) = width(unit, &value, &mut shape, 0) else {
            continue;
        };
        if let Some((t, v)) = mismatch(&target_width, &value_width, shape.arithmetic) {
            let name = target.text().to_string();
            let name: String = name.split_whitespace().collect::<Vec<_>>().join(" ");
            out.push(Finding::new(
                codes::WIDTH_MISMATCH,
                node.text_range(),
                format!("`{name}` has {t} {} but the value has {v}", plural(&t)),
            ));
        }
    }
}

/// Whether an expression uses a macro, whose expansion is not known.
fn has_macro(node: &SyntaxNode) -> bool {
    super::all_tokens(node).any(|t| matches!(t.kind(), K::MACRO_REF | K::MACRO_CALL))
}

/// `q = {q, d}` or `q[3:1] <= q`: a shift register made wider and cut
/// back on purpose.
fn shifts_into(target: &SyntaxNode, value: &SyntaxNode) -> bool {
    let base = |n: &SyntaxNode| -> Option<String> {
        let n = if n.kind() == K::INDEX_EXPR {
            n.first_child()?
        } else {
            n.clone()
        };
        (n.kind() == K::NAME_REF)
            .then(|| scope::ref_name(&n))
            .flatten()
            .map(|t| ident(&t))
    };
    let Some(name) = base(target) else {
        return false;
    };
    let parts: Vec<SyntaxNode> = if value.kind() == K::CONCAT_EXPR {
        value.children().collect()
    } else {
        vec![value.clone()]
    };
    parts
        .iter()
        .any(|part| base(part).is_some_and(|p| p == name))
}

/// `q <= i;` with `integer i`: a loop index or counter whose values fit.
fn integer(unit: &Unit, value: &SyntaxNode) -> bool {
    value.kind() == K::NAME_REF
        && scope::ref_name(value).is_some_and(|name| {
            signal_decl(unit, &ident(&name), value)
                .and_then(|d| d.data_type.as_ref())
                .is_some_and(|t| {
                    scope::has_token(t, K::INTEGER_KW) || scope::has_token(t, K::INT_KW)
                })
        })
}

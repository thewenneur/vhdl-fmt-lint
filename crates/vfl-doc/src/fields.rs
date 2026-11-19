//! Bit-field layouts: the elements of a record type with their sizes, and
//! the aliases of slices of a signal as a register map.
//!
//! Sizes come from the subtypes as written: `std_logic`, vectors with a
//! range, integers with a range, enumerations, records and arrays of
//! those, and named subtypes of them. Constants and generic defaults in
//! the ranges are evaluated; a size that cannot be worked out is shown as
//! `?`.

use std::collections::HashMap;

use vfl_syntax::identifier_key;

use crate::model::{Alias, Design, Interface, Object, TypeInfo};
use crate::page::{Inline, code, text};
use crate::svg::{Canvas, cut, width};

/// Names a size can depend on: constants (with generic defaults) and
/// types, by key, then those of the parent scope.
pub struct Scope<'a> {
    parent: Option<&'a Scope<'a>>,
    constants: HashMap<String, String>,
    types: HashMap<String, &'a TypeInfo>,
}

impl<'a> Scope<'a> {
    /// The declarations of every package, built once and shared as the
    /// parent of the other scopes.
    pub fn packages(design: &'a Design) -> Self {
        let mut scope = Scope {
            parent: None,
            constants: HashMap::new(),
            types: HashMap::new(),
        };
        for p in &design.packages {
            scope.declare(&p.constants, &p.types);
        }
        scope
    }

    /// The scope of an architecture of `entity` (or of a package when
    /// `entity` is `None`): its declarations and the generics, then the
    /// packages.
    pub fn new(
        packages: &'a Scope<'a>,
        design: &'a Design,
        entity: Option<usize>,
        constants: &[Object],
        types: &'a [TypeInfo],
    ) -> Self {
        let mut scope = Scope {
            parent: Some(packages),
            constants: HashMap::new(),
            types: HashMap::new(),
        };
        if let Some(e) = entity {
            for g in &design.entities[e].generics {
                if let Some(v) = &g.default {
                    scope.constants.insert(identifier_key(&g.name), v.clone());
                }
            }
        }
        scope.declare(constants, types);
        scope
    }

    fn declare(&mut self, constants: &[Object], types: &'a [TypeInfo]) {
        for c in constants {
            if let Some(v) = &c.value {
                self.constants.insert(identifier_key(&c.name), v.clone());
            }
        }
        for t in types {
            self.types.insert(identifier_key(&t.name), t);
        }
    }

    fn constant(&self, key: &str) -> Option<&str> {
        match self.constants.get(key) {
            Some(v) => Some(v),
            None => self.parent?.constant(key),
        }
    }

    fn ty(&self, key: &str) -> Option<&'a TypeInfo> {
        match self.types.get(key) {
            Some(t) => Some(t),
            None => self.parent?.ty(key),
        }
    }

    /// The value of an integer expression of literals, constants and
    /// `+ - * / ** mod rem` (a small parser: nothing is executed).
    pub fn integer(&self, text: &str) -> Option<i64> {
        let tokens = tokenize(text)?;
        let mut parser = Expr {
            tokens: &tokens,
            at: 0,
            scope: self,
            depth: 0,
        };
        let value = parser.sum()?;
        (parser.at == tokens.len()).then_some(value)
    }

    /// The number of bits of a subtype as written.
    pub fn bits(&self, ty: &str) -> Option<u64> {
        self.bits_at(ty, 0)
    }

    fn bits_at(&self, ty: &str, depth: usize) -> Option<u64> {
        if depth > 16 {
            return None;
        }
        let ty = ty.trim();
        let lower = ty.to_ascii_lowercase();
        // `integer range 0 to 7`, `natural range 0 to N - 1`.
        if let Some((mark, range)) = split_word(&lower, " range ") {
            let mark = mark.trim();
            if matches!(mark, "integer" | "natural" | "positive") {
                let (a, b) = range_bounds(range)?;
                let (a, b) = (self.integer(a)?, self.integer(b)?);
                return Some(int_bits(a.min(b), a.max(b)));
            }
            return self.bits_at(mark, depth + 1);
        }
        // `std_logic_vector(7 downto 0)`, `unsigned(N - 1 downto 0)`.
        if let Some(open) = lower.find('(')
            && lower.ends_with(')')
        {
            let mark = lower[..open].trim();
            let inner = &ty[open + 1..ty.len() - 1];
            let element = match mark {
                "std_logic_vector"
                | "std_ulogic_vector"
                | "bit_vector"
                | "signed"
                | "unsigned"
                | "unresolved_signed"
                | "unresolved_unsigned"
                | "u_signed"
                | "u_unsigned" => 1,
                _ => {
                    // A constrained array subtype: the element of its type.
                    let t = self.ty(&identifier_key(mark))?;
                    let def = t.definition.to_ascii_lowercase();
                    let of = def.rfind(" of ")?;
                    self.bits_at(&t.definition[of + 4..], depth + 1)?
                }
            };
            return Some(self.count(inner)? * element);
        }
        match lower.as_str() {
            "std_logic" | "std_ulogic" | "bit" | "boolean" => return Some(1),
            "integer" | "natural" | "positive" => return Some(32),
            "character" => return Some(8),
            _ => {}
        }
        let t = self.ty(&identifier_key(&lower))?;
        if !t.fields.is_empty() {
            return t
                .fields
                .iter()
                .map(|(_, ty, _)| self.bits_at(ty, depth + 1))
                .sum();
        }
        let def = t.definition.trim();
        if def.starts_with('(') {
            let literals = def.matches(',').count() as u64 + 1;
            return Some(ceil_log2(literals));
        }
        let lower_def = def.to_ascii_lowercase();
        if lower_def.starts_with("array") {
            // `array (0 to 3) of byte_t`.
            let open = def.find('(')?;
            let close = matching(def, open)?;
            let of = lower_def[close..].find(" of ")? + close;
            let count = self.count(&def[open + 1..close])?;
            return Some(count * self.bits_at(&def[of + 4..], depth + 1)?);
        }
        if lower_def.starts_with("range ") {
            let (a, b) = range_bounds(&def[6..])?;
            let (a, b) = (self.integer(a)?, self.integer(b)?);
            return Some(int_bits(a.min(b), a.max(b)));
        }
        // A subtype: its definition is a subtype indication.
        if t.kind == "subtype" {
            return self.bits_at(def, depth + 1);
        }
        None
    }

    /// The number of elements of a range: `7 downto 0` is 8.
    fn count(&self, range: &str) -> Option<u64> {
        let (a, b) = range_bounds(range)?;
        Some(self.integer(a)?.abs_diff(self.integer(b)?) + 1)
    }

    /// The bits a slice covers: `(msb, lsb)`.
    fn slice(&self, slice: &str) -> Option<(u64, u64)> {
        match range_bounds(slice) {
            Some((a, b)) => {
                let (a, b) = (self.integer(a)?, self.integer(b)?);
                Some((a.max(b).try_into().ok()?, a.min(b).try_into().ok()?))
            }
            None => {
                let i: u64 = self.integer(slice)?.try_into().ok()?;
                Some((i, i))
            }
        }
    }
}

/// The Size cell of a port: its bits as a number when the subtype fixes
/// them, else the width expression over the generics, followed by the
/// value their defaults give when known. A subtype with no width (an
/// interface, a record of unknown fields) is shown as written.
pub fn port_size(packages: &Scope, generics: &[Interface], ty: &str, verilog: bool) -> Vec<Inline> {
    if verilog {
        return verilog_size(packages, ty);
    }
    if let Some(bits) = packages.bits(ty) {
        return vec![text(bits.to_string())];
    }
    let mut scope = Scope {
        parent: Some(packages),
        constants: HashMap::new(),
        types: HashMap::new(),
    };
    for g in generics {
        if let Some(v) = &g.default {
            scope.constants.insert(identifier_key(&g.name), v.clone());
        }
    }
    let mut cell = vec![code(
        vector_width(ty).unwrap_or_else(|| ty.trim().to_owned()),
    )];
    if let Some(bits) = scope.bits(ty) {
        cell.push(text(format!(" (default {bits})")));
    }
    cell
}

/// `N - 1 + 1` for `std_logic_vector(N - 1 downto 0)`, simplified to `N`.
fn vector_width(ty: &str) -> Option<String> {
    let open = ty.find('(')?;
    let inner = ty.trim_end().strip_suffix(')')?.get(open + 1..)?;
    let (high, low) = match split_word(inner, " downto ") {
        Some((h, l)) => (h, l),
        None => {
            let (l, h) = split_word(inner, " to ")?;
            (h, l)
        }
    };
    Some(width_of(high.trim(), low.trim()))
}

fn width_of(high: &str, low: &str) -> String {
    if low == "0" {
        match high.strip_suffix('1').map(str::trim_end) {
            Some(rest) if rest.ends_with('-') => rest[..rest.len() - 1].trim_end().to_owned(),
            _ => format!("{high} + 1"),
        }
    } else {
        format!("{high} - {low} + 1")
    }
}

/// Verilog: the product of the `[msb:lsb]` dimensions, 1 for a scalar net.
fn verilog_size(packages: &Scope, ty: &str) -> Vec<Inline> {
    let ty = ty.trim();
    let base = ty.split('[').next().unwrap_or("").trim();
    let element = match base.rsplit(' ').next().unwrap_or("") {
        _ if base.contains('.') => return vec![code(ty.to_owned())],
        "byte" => 8,
        "shortint" => 16,
        "int" | "integer" => 32,
        "longint" => 64,
        _ => 1,
    };
    let mut bits = Some(element);
    let mut exprs = Vec::new();
    for dim in ty.split('[').skip(1) {
        let Some((range, _)) = dim.split_once(']') else {
            return vec![code(ty.to_owned())];
        };
        let Some((a, b)) = range.split_once(':') else {
            return vec![code(ty.to_owned())];
        };
        let (a, b) = (a.trim(), b.trim());
        bits =
            bits.and_then(|n| Some(n * (packages.integer(a)?.abs_diff(packages.integer(b)?) + 1)));
        exprs.push(width_of(a, b));
    }
    match bits {
        Some(n) => vec![text(n.to_string())],
        None if exprs.len() == 1 && element == 1 => vec![code(exprs.remove(0))],
        None => vec![code(ty.to_owned())],
    }
}

fn ceil_log2(n: u64) -> u64 {
    if n <= 1 {
        1
    } else {
        64 - u64::from((n - 1).leading_zeros())
    }
}

/// Bits of an integer range.
fn int_bits(low: i64, high: i64) -> u64 {
    if low >= 0 {
        ceil_log2(high as u64 + 1)
    } else {
        let magnitude = high.max(-low - 1) as u64;
        ceil_log2(magnitude + 1) + 1
    }
}

/// `text` split at the first `word` outside parentheses.
fn split_word<'t>(text: &'t str, word: &str) -> Option<(&'t str, &'t str)> {
    let lower = text.to_ascii_lowercase();
    let mut depth = 0i32;
    for (i, c) in lower.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            _ if depth == 0 && lower[i..].starts_with(word) => {
                return Some((&text[..i], &text[i + word.len()..]));
            }
            _ => {}
        }
    }
    None
}

/// The bounds of `a downto b` or `a to b`.
fn range_bounds(range: &str) -> Option<(&str, &str)> {
    split_word(range, " downto ").or_else(|| split_word(range, " to "))
}

fn matching(text: &str, open: usize) -> Option<usize> {
    let mut depth = 0;
    for (i, c) in text[open..].char_indices() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(open + i);
                }
            }
            _ => {}
        }
    }
    None
}

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Number(i64),
    Name(String),
    Op(&'static str),
}

fn tokenize(text: &str) -> Option<Vec<Token>> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
        } else if c.is_ascii_digit() {
            let start = i;
            while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '_') {
                i += 1;
            }
            let digits: String = chars[start..i].iter().filter(|&&c| c != '_').collect();
            out.push(Token::Number(digits.parse().ok()?));
        } else if c.is_alphabetic() {
            let start = i;
            while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            let word: String = chars[start..i].iter().collect();
            match word.to_ascii_lowercase().as_str() {
                "mod" => out.push(Token::Op("mod")),
                "rem" => out.push(Token::Op("rem")),
                _ => out.push(Token::Name(word)),
            }
        } else {
            let op = match (c, chars.get(i + 1)) {
                ('*', Some('*')) => {
                    i += 1;
                    "**"
                }
                ('+', _) => "+",
                ('-', _) => "-",
                ('*', _) => "*",
                ('/', _) => "/",
                ('(', _) => "(",
                (')', _) => ")",
                _ => return None,
            };
            i += 1;
            out.push(Token::Op(op));
        }
    }
    Some(out)
}

struct Expr<'t, 's> {
    tokens: &'t [Token],
    at: usize,
    scope: &'s Scope<'s>,
    depth: usize,
}

impl Expr<'_, '_> {
    fn op(&mut self, op: &str) -> bool {
        if matches!(self.tokens.get(self.at), Some(Token::Op(o)) if *o == op) {
            self.at += 1;
            true
        } else {
            false
        }
    }

    fn sum(&mut self) -> Option<i64> {
        let mut value = if self.op("-") {
            -self.product()?
        } else {
            self.op("+");
            self.product()?
        };
        loop {
            if self.op("+") {
                value = value.checked_add(self.product()?)?;
            } else if self.op("-") {
                value = value.checked_sub(self.product()?)?;
            } else {
                return Some(value);
            }
        }
    }

    fn product(&mut self) -> Option<i64> {
        let mut value = self.power()?;
        loop {
            if self.op("*") {
                value = value.checked_mul(self.power()?)?;
            } else if self.op("/") {
                value = value.checked_div(self.power()?)?;
            } else if self.op("mod") {
                value = value.checked_rem_euclid(self.power()?)?;
            } else if self.op("rem") {
                value = value.checked_rem(self.power()?)?;
            } else {
                return Some(value);
            }
        }
    }

    fn power(&mut self) -> Option<i64> {
        let base = self.atom()?;
        if self.op("**") {
            let exp: u32 = self.atom()?.try_into().ok()?;
            return base.checked_pow(exp);
        }
        Some(base)
    }

    fn atom(&mut self) -> Option<i64> {
        match self.tokens.get(self.at)?.clone() {
            Token::Number(n) => {
                self.at += 1;
                Some(n)
            }
            Token::Name(name) => {
                self.at += 1;
                if self.depth > 16 {
                    return None;
                }
                let text = self.scope.constant(&identifier_key(&name))?;
                let tokens = tokenize(text)?;
                let mut inner = Expr {
                    tokens: &tokens,
                    at: 0,
                    scope: self.scope,
                    depth: self.depth + 1,
                };
                let value = inner.sum()?;
                (inner.at == tokens.len()).then_some(value)
            }
            Token::Op("(") => {
                self.at += 1;
                let value = self.sum()?;
                self.op(")").then_some(value)
            }
            Token::Op(_) => None,
        }
    }
}

/// A field of a layout.
struct Field {
    name: String,
    /// `msb:lsb` or `n bits`.
    detail: String,
    bits: Option<u64>,
    /// Bits no alias names.
    reserved: bool,
}

const MAX_WIDTH: f64 = 760.0;

fn draw(title: &str, heading: &str, fields: &[Field]) -> String {
    const H: f64 = 44.0;
    let known: u64 = fields.iter().filter_map(|f| f.bits).sum();
    let scale = if known == 0 {
        24.0
    } else {
        (MAX_WIDTH / known as f64).clamp(10.0, 48.0)
    };
    let mut c = Canvas::new();
    c.text(0.0, 14.0, &cut(heading, 80), "start", "vfl-title");
    let top = 24.0;
    let mut x = 0.0;
    for f in fields {
        let natural = f.bits.map_or(48.0, |b| b as f64 * scale);
        let label_w = width(&cut(&f.name, 24)).max(width(&f.detail)) + 10.0;
        let w = natural.max(if f.reserved { 16.0 } else { label_w });
        c.rect(x, top, w, H, if f.reserved { "vfl-ext" } else { "vfl-box" });
        if !f.reserved {
            c.text(
                x + w / 2.0,
                top + 18.0,
                &cut(&f.name, 24),
                "middle",
                "vfl-title",
            );
        }
        if w >= width(&f.detail) + 4.0 {
            c.text(
                x + w / 2.0,
                top + 35.0,
                &f.detail,
                "middle",
                "vfl-muted vfl-small",
            );
        }
        x += w;
    }
    c.finish(title, 8.0)
}

/// The layout of a record type; `None` for other types.
pub fn record(scope: &Scope, t: &TypeInfo) -> Option<String> {
    if t.fields.is_empty() {
        return None;
    }
    let fields: Vec<Field> = t
        .fields
        .iter()
        .map(|(name, ty, _)| {
            let bits = scope.bits(ty);
            Field {
                name: name.clone(),
                detail: match bits {
                    Some(1) => "1 bit".into(),
                    Some(b) => format!("{b} bits"),
                    None => "?".into(),
                },
                bits,
                reserved: false,
            }
        })
        .collect();
    let total: Option<u64> = fields.iter().map(|f| f.bits).sum();
    let heading = match total {
        Some(total) => format!("{} : {total} bits", t.name),
        None => t.name.clone(),
    };
    Some(draw(&format!("Fields of {}", t.name), &heading, &fields))
}

/// The register map of each signal with aliases of its slices: the
/// signal, MSB first, with the aliased bits named and the others shaded.
pub fn aliases(scope: &Scope, signals: &[Object], aliases: &[Alias]) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for signal in signals {
        let key = identifier_key(&signal.name);
        let mine: Vec<&Alias> = aliases
            .iter()
            .filter(|a| identifier_key(&a.target) == key && a.slice.is_some())
            .collect();
        if mine.is_empty() {
            continue;
        }
        let Some(total) = scope.bits(&signal.ty) else {
            continue;
        };
        if total == 0 || total > 1024 {
            continue;
        }
        let mut slices: Vec<(u64, u64, &Alias)> = mine
            .iter()
            .filter_map(|a| {
                let (msb, lsb) = scope.slice(a.slice.as_deref()?)?;
                (msb < total).then_some((msb, lsb, *a))
            })
            .collect();
        if slices.is_empty() {
            continue;
        }
        slices.sort_by_key(|s| std::cmp::Reverse(s.0));
        let mut fields = Vec::new();
        let mut next = total; // bits above `next` are laid out
        for (msb, lsb, alias) in slices {
            if msb + 1 > next {
                continue; // overlaps the previous alias
            }
            if msb + 1 < next {
                fields.push(Field {
                    name: String::new(),
                    detail: range_text(next - 1, msb + 1),
                    bits: Some(next - 1 - msb),
                    reserved: true,
                });
            }
            fields.push(Field {
                name: alias.name.clone(),
                detail: range_text(msb, lsb),
                bits: Some(msb - lsb + 1),
                reserved: false,
            });
            next = lsb;
        }
        if next > 0 {
            fields.push(Field {
                name: String::new(),
                detail: range_text(next - 1, 0),
                bits: Some(next),
                reserved: true,
            });
        }
        out.push((
            signal.name.clone(),
            draw(
                &format!("Fields of {}", signal.name),
                &format!("{} : {total} bits", signal.name),
                &fields,
            ),
        ));
    }
    out
}

fn range_text(msb: u64, lsb: u64) -> String {
    if msb == lsb {
        format!("{msb}")
    } else {
        format!("{msb}:{lsb}")
    }
}

#[cfg(test)]
mod size_tests {
    use super::*;

    #[test]
    fn vector_widths_are_simplified() {
        assert_eq!(
            vector_width("std_logic_vector(WIDTH - 1 downto 0)").as_deref(),
            Some("WIDTH")
        );
        assert_eq!(vector_width("unsigned(N-1 downto 0)").as_deref(), Some("N"));
        assert_eq!(
            vector_width("std_logic_vector(0 to N)").as_deref(),
            Some("N + 1")
        );
        assert_eq!(
            vector_width("signed(H downto L)").as_deref(),
            Some("H - L + 1")
        );
        assert_eq!(width_of("W-1", "0"), "W");
    }

    #[test]
    fn port_sizes() {
        let empty = Scope {
            parent: None,
            constants: HashMap::new(),
            types: HashMap::new(),
        };
        let generic = Interface {
            name: "WIDTH".into(),
            mode: String::new(),
            ty: "natural".into(),
            default: Some("8".into()),
            description: String::new(),
        };
        let size = |ty: &str, verilog| {
            port_size(&empty, std::slice::from_ref(&generic), ty, verilog)
                .iter()
                .map(|i| format!("{i:?}"))
                .collect::<String>()
        };
        assert!(size("std_logic", false).contains("\"1\""));
        assert!(size("std_logic_vector(7 downto 0)", false).contains("\"8\""));
        assert!(size("std_logic_vector(WIDTH - 1 downto 0)", false).contains("(default 8)"));
        assert!(size("", true).contains("\"1\""));
        assert!(size("logic [3:0][7:0]", true).contains("\"32\""));
        assert!(size("logic [W-1:0]", true).contains("\"W\""));
    }
}

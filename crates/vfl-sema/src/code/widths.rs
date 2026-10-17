//! Widths of vectors: `sema/width-mismatch`, and the widths and bounds
//! `sema/comparison-width` and `sema/index-range` compare.
//!
//! A width is a linear expression of generics and of loop and generate
//! parameters: `std_logic_vector(W - 1 downto 0)` has `W` elements, so it
//! is known to differ from `std_logic_vector(W downto 0)`, and to agree
//! with `unsigned(W - 1 downto 0)`, whatever `W` is. Constants are followed
//! to their value, or to their initial value when it depends on generics.
//! Where two widths differ by an amount that depends on generics, the
//! `generics` option compares them with the generics at their default
//! values.
//!
//! At an instantiation, the generics of the instantiated entity or
//! component are replaced by the actuals of the generic map (which may
//! depend on the generics of the unit that instantiates it), or by their
//! default values, so that each port has the width of that instance.

use rustc_hash::FxHashMap;
use smallvec::{SmallVec, smallvec};
use vfl_syntax::green_walk::ChildTokens;
use vfl_syntax::{SyntaxKind, SyntaxNode};

use super::{Checker, Green, Values, initial_expression, single_association, unparen};
use crate::decl::{Decl, DeclId, DeclKind, Reference, Target, TypeDef, UnitId};
use crate::diagnostic::codes;
use crate::resolver::mark_token;
use crate::rtl::{literal_width, static_int};

/// How deep expressions and names are followed: concatenations of many
/// parts nest deeply.
const MAX_DEPTH: u32 = 64;

/// `constant + Σ factor × symbol`, the symbols being generics and loop and
/// generate parameters.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Linear {
    constant: i64,
    /// Sorted by symbol, without zero factors. Most widths have one
    /// generic at most: kept inline, with no allocation.
    terms: SmallVec<[(DeclId, i64); 2]>,
}

impl Linear {
    pub(super) fn constant(value: i64) -> Linear {
        Linear {
            constant: value,
            terms: SmallVec::new(),
        }
    }

    fn symbol(id: DeclId) -> Linear {
        Linear {
            constant: 0,
            terms: smallvec![(id, 1)],
        }
    }

    pub(super) fn as_constant(&self) -> Option<i64> {
        self.terms.is_empty().then_some(self.constant)
    }

    fn is_symbolic(&self) -> bool {
        !self.terms.is_empty()
    }

    /// `self + sign × other`.
    fn add(&self, other: &Linear, sign: i64) -> Linear {
        if other.terms.is_empty() {
            return Linear {
                constant: self
                    .constant
                    .saturating_add(other.constant.saturating_mul(sign)),
                terms: self.terms.clone(),
            };
        }
        let mut terms = SmallVec::new();
        let (mut a, mut b) = (self.terms.iter().peekable(), other.terms.iter().peekable());
        loop {
            let next = match (a.peek(), b.peek()) {
                (Some(&&(x, f)), Some(&&(y, g))) if x == y => {
                    a.next();
                    b.next();
                    (x, f.saturating_add(g.saturating_mul(sign)))
                }
                (Some(&&(x, f)), Some(&&(y, _))) if x < y => {
                    a.next();
                    (x, f)
                }
                (Some(&&(x, f)), None) => {
                    a.next();
                    (x, f)
                }
                (_, Some(&&(y, g))) => {
                    b.next();
                    (y, g.saturating_mul(sign))
                }
                (None, None) => break,
            };
            if next.1 != 0 {
                terms.push(next);
            }
        }
        Linear {
            constant: self
                .constant
                .saturating_add(other.constant.saturating_mul(sign)),
            terms,
        }
    }

    fn sub(&self, other: &Linear) -> Linear {
        self.add(other, -1)
    }

    fn plus(&self, value: i64) -> Linear {
        self.add(&Linear::constant(value), 1)
    }

    fn scale(&self, by: i64) -> Linear {
        if by == 0 {
            return Linear::constant(0);
        }
        Linear {
            constant: self.constant.saturating_mul(by),
            terms: self
                .terms
                .iter()
                .map(|&(id, f)| (id, f.saturating_mul(by)))
                .collect(),
        }
    }

    /// The expression with each symbol replaced by what `with` gives for
    /// it; `None` if it gives `None` for one.
    fn substitute(&self, with: &mut dyn FnMut(DeclId) -> Option<Linear>) -> Option<Linear> {
        let mut out = Linear::constant(self.constant);
        for &(id, factor) in &self.terms {
            out = out.add(&with(id)?.scale(factor), 1);
        }
        Some(out)
    }

    /// The expression as VHDL, with the symbols' names: `W - 1`, `2 * N`.
    fn show(&self, values: &Values) -> String {
        let mut out = String::new();
        for &(id, factor) in &self.terms {
            let name = values
                .analysis
                .decl(id)
                .map_or("?", |d| d.spelling.as_str());
            let magnitude = factor.unsigned_abs();
            if out.is_empty() {
                if factor < 0 {
                    out.push('-');
                }
            } else {
                out.push_str(if factor < 0 { " - " } else { " + " });
            }
            if magnitude != 1 {
                out.push_str(&format!("{magnitude} * "));
            }
            out.push_str(name);
        }
        if out.is_empty() {
            return self.constant.to_string();
        }
        if self.constant != 0 {
            let sign = if self.constant < 0 { '-' } else { '+' };
            out.push_str(&format!(" {sign} {}", self.constant.unsigned_abs()));
        }
        out
    }

    /// A width in a message: `8`, or `` `W + 1` ``.
    fn describe(&self, values: &Values) -> String {
        match self.as_constant() {
            Some(n) => n.to_string(),
            None => format!("`{}`", self.show(values)),
        }
    }
}

/// The bounds of a one-dimensional range: `left to right` or `left downto
/// right`.
#[derive(Clone, Debug)]
pub(super) struct LinBounds {
    left: Linear,
    right: Linear,
    ascending: bool,
}

impl LinBounds {
    fn lo(&self) -> &Linear {
        if self.ascending {
            &self.left
        } else {
            &self.right
        }
    }

    fn hi(&self) -> &Linear {
        if self.ascending {
            &self.right
        } else {
            &self.left
        }
    }

    fn is_symbolic(&self) -> bool {
        self.left.is_symbolic() || self.right.is_symbolic()
    }

    /// The number of elements: `hi - lo + 1`, at least 0 when it is known.
    fn width(&self) -> Linear {
        let width = self.hi().sub(self.lo()).plus(1);
        match width.as_constant() {
            Some(n) if n < 0 => Linear::constant(0),
            _ => width,
        }
    }

    fn reversed(self) -> LinBounds {
        LinBounds {
            left: self.right,
            right: self.left,
            ascending: !self.ascending,
        }
    }

    fn describe(&self, values: &Values) -> String {
        let direction = if self.ascending { "to" } else { "downto" };
        format!(
            "{} {direction} {}",
            self.left.show(values),
            self.right.show(values)
        )
    }
}

/// What an expression is, as far as widths go.
#[derive(Clone, Debug)]
pub(super) enum Shape {
    /// One value: a bit, an integer, an enumeration literal.
    Scalar,
    /// A one-dimensional array of scalars with this many elements.
    Vector(Linear),
}

/// What the type of an object is, as far as widths go.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Class {
    Scalar,
    /// A one-dimensional array of scalars.
    Vector,
    /// A one-dimensional array of vectors.
    Memory,
    Other,
}

/// The operator token of a unary or binary expression.
fn operator(expr: &SyntaxNode) -> Option<vfl_syntax::SyntaxToken> {
    expr.child_token(|k| !k.is_trivia())
}

/// The arguments of a call: each with its formal's name when named.
fn arguments(list: &SyntaxNode) -> Vec<(Option<String>, SyntaxNode)> {
    list.children()
        .filter(|n| n.kind() == SyntaxKind::ASSOCIATION_ELEMENT)
        .filter_map(|element| {
            let mut parts = element.children();
            if element.has_child(|k| k == SyntaxKind::ARROW) {
                let formal = parts.next()?;
                let name = formal.text().to_string().trim().to_ascii_lowercase();
                Some((Some(name), parts.next()?))
            } else {
                Some((None, parts.next()?))
            }
        })
        .collect()
}

/// The argument at `position`, or the one named one of `names`.
fn argument<'n>(
    args: &'n [(Option<String>, SyntaxNode)],
    position: usize,
    names: &[&str],
) -> Option<&'n SyntaxNode> {
    if let Some((_, value)) = args
        .iter()
        .find(|(f, _)| f.as_deref().is_some_and(|f| names.contains(&f)))
    {
        return Some(value);
    }
    args.iter()
        .take_while(|(f, _)| f.is_none())
        .nth(position)
        .map(|(_, v)| v)
}

impl<'a> Values<'a> {
    fn is_foreign(&self, decl: &Decl) -> bool {
        self.analysis
            .files()
            .get(decl.file as usize)
            .is_some_and(|f| f.foreign.is_some())
    }

    /// The reference at `range` of `unit`.
    fn reference(&self, unit: UnitId, range: vfl_syntax::TextRange) -> Option<&'a Reference> {
        let refs = self.analysis.unit_references(unit);
        let i = refs.partition_point(|r| r.range.start() < range.start());
        refs[i..]
            .iter()
            .take_while(|r| r.range.start() == range.start())
            .find(|r| r.range == range)
    }

    /// The single declaration a name (`s`, `pkg.s`) of `unit` refers to.
    fn named(&self, unit: UnitId, name: &SyntaxNode) -> Option<(DeclId, &'a Decl)> {
        if !matches!(name.kind(), SyntaxKind::NAME | SyntaxKind::SELECTED_NAME) {
            return None;
        }
        let id = self.target(unit, mark_token(name)?.text_range())?;
        Some((id, self.analysis.decl(id)?))
    }

    /// The name of the function of the bundled libraries a name of `unit`
    /// calls, in lower case, when every declaration it may refer to is
    /// one.
    fn builtin_function(&self, unit: UnitId, name: &SyntaxNode) -> Option<&'a str> {
        if !matches!(name.kind(), SyntaxKind::NAME | SyntaxKind::SELECTED_NAME) {
            return None;
        }
        let r = self.reference(unit, mark_token(name)?.text_range())?;
        let mut found = None;
        for target in &r.targets {
            let Target::Decl(id) = target else {
                return None;
            };
            let decl = self.analysis.decl(*id)?;
            if decl.kind != DeclKind::Function
                || !self.analysis.files().get(decl.file as usize)?.builtin
            {
                return None;
            }
            found = Some(decl.name.as_str());
        }
        found
    }

    /// Whether the operator of a binary expression is predefined or one
    /// of the bundled libraries (`numeric_std`'s `+`), whose result width
    /// is known.
    fn known_operator(&self, unit: UnitId, token: &vfl_syntax::SyntaxToken) -> bool {
        let Some(r) = self.reference(unit, token.text_range()) else {
            return true;
        };
        r.targets.iter().all(|t| match t {
            Target::Decl(id) => self.analysis.decl(*id).is_some_and(|d| {
                self.analysis
                    .files()
                    .get(d.file as usize)
                    .is_some_and(|f| f.builtin)
            }),
            Target::Library(_) => false,
        })
    }

    /// The definition of the type a type or subtype mark denotes.
    fn mark_def(&self, mut mark: DeclId) -> Option<&'a TypeDef> {
        for _ in 0..MAX_DEPTH {
            let decl = self.analysis.decl(mark)?;
            match decl.kind {
                DeclKind::Type => return decl.type_def.as_deref(),
                DeclKind::Subtype => mark = decl.subtype?,
                _ => return None,
            }
        }
        None
    }

    fn class(&self, mark: Option<DeclId>) -> Class {
        let scalar = |d: &TypeDef| {
            matches!(
                d,
                TypeDef::Enum(_)
                    | TypeDef::Integer
                    | TypeDef::Real
                    | TypeDef::Physical
                    | TypeDef::Scalar
            )
        };
        match mark.and_then(|m| self.mark_def(m)) {
            Some(d) if scalar(d) => Class::Scalar,
            Some(TypeDef::Array {
                indexes,
                element: Some(element),
            }) if indexes.len() == 1 => match self.mark_def(*element) {
                Some(d) if scalar(d) => Class::Vector,
                Some(TypeDef::Array {
                    indexes,
                    element: Some(inner),
                }) if indexes.len() == 1 && self.mark_def(*inner).is_some_and(scalar) => {
                    Class::Memory
                }
                _ => Class::Other,
            },
            _ => Class::Other,
        }
    }

    /// Whether a declaration is an object (or an alias of one) of a type
    /// whose width is checked: a vector of scalars.
    pub(super) fn is_vector(&self, decl: &Decl) -> bool {
        (decl.kind.is_object() || decl.kind == DeclKind::Alias)
            && (self.class(decl.subtype) == Class::Vector
                || (self.is_foreign(decl) && decl.width.is_some()))
    }

    // ---- Linear values -------------------------------------------------------

    /// The value of an integer expression of `unit`, as a linear expression
    /// of generics and loop and generate parameters.
    pub(super) fn linear(&self, expr: &SyntaxNode, unit: UnitId, depth: u32) -> Option<Linear> {
        use SyntaxKind::*;
        if depth > MAX_DEPTH {
            return None;
        }
        let depth = depth + 1;
        match expr.kind() {
            LITERAL => static_int(expr, &|_| None).map(Linear::constant),
            PAREN_EXPR => self.linear(&expr.children().next()?, unit, depth),
            NAME | SELECTED_NAME => {
                let id = self.target(unit, mark_token(expr)?.text_range())?;
                self.name_linear(id, depth)
            }
            ATTRIBUTE_NAME => self.attribute_linear(expr, unit, depth),
            UNARY_EXPR => {
                let operand = self.linear(&expr.children().next()?, unit, depth)?;
                match operator(expr)?.kind() {
                    MINUS => Some(operand.scale(-1)),
                    PLUS => Some(operand),
                    _ => None,
                }
            }
            BINARY_EXPR => {
                let mut operands = expr.children();
                let left = self.linear(&operands.next()?, unit, depth)?;
                let right = self.linear(&operands.next()?, unit, depth)?;
                match operator(expr)?.kind() {
                    PLUS => Some(left.add(&right, 1)),
                    MINUS => Some(left.sub(&right)),
                    STAR => match (left.as_constant(), right.as_constant()) {
                        (Some(l), _) => Some(right.scale(l)),
                        (_, Some(r)) => Some(left.scale(r)),
                        _ => None,
                    },
                    // Of constants only.
                    kind => {
                        let (l, r) = (left.as_constant()?, right.as_constant()?);
                        match kind {
                            SLASH => l.checked_div(r),
                            MOD_KW => l
                                .checked_rem_euclid(r)
                                .map(|m| if r < 0 && m != 0 { m + r } else { m }),
                            REM_KW => l.checked_rem(r),
                            STAR2 => l.checked_pow(u32::try_from(r).ok()?),
                            _ => None,
                        }
                        .map(Linear::constant)
                    }
                }
            }
            _ => None,
        }
    }

    /// The value of a name: a constant's, or the name itself for a generic
    /// or a loop or generate parameter.
    fn name_linear(&self, id: DeclId, depth: u32) -> Option<Linear> {
        if let Some(value) = self.linears.borrow().get(&id) {
            return value.clone();
        }
        // A constant defined in terms of itself stops here.
        self.linears.borrow_mut().insert(id, None);
        let value = self.compute_name_linear(id, depth);
        self.linears.borrow_mut().insert(id, value.clone());
        value
    }

    fn compute_name_linear(&self, id: DeclId, depth: u32) -> Option<Linear> {
        let decl = self.analysis.decl(id)?;
        match decl.kind {
            DeclKind::Constant => {
                if let Some(value) = decl.value {
                    return Some(Linear::constant(value));
                }
                if self.is_foreign(decl) {
                    return None;
                }
                let expr = initial_expression(&self.declaration(decl)?)?;
                self.linear(&expr, id.unit, depth)
            }
            DeclKind::Generic { .. } | DeclKind::LoopParameter | DeclKind::GenerateParameter => {
                Some(Linear::symbol(id))
            }
            _ => None,
        }
    }

    /// `s'length`, `s'high`, `s'low`, `s'left`, `s'right` of an object.
    fn attribute_linear(&self, expr: &SyntaxNode, unit: UnitId, depth: u32) -> Option<Linear> {
        let prefix = expr.children().next()?;
        if expr.children().nth(1).is_some() {
            // `s'length(2)`.
            return None;
        }
        let attribute = expr.last_token()?.text().to_ascii_lowercase();
        let (id, decl) = self.named(unit, &prefix)?;
        if !(decl.kind.is_object() || decl.kind == DeclKind::Alias) {
            return None;
        }
        if attribute == "length" {
            return self.object_width(id, depth);
        }
        let bounds = self.bounds(id, depth)?;
        Some(match attribute.as_str() {
            "left" => bounds.left,
            "right" => bounds.right,
            "high" => bounds.hi().clone(),
            "low" => bounds.lo().clone(),
            _ => return None,
        })
    }

    /// The value of each symbol of `value` at the generics' default values.
    fn at_defaults(&self, value: &Linear, depth: u32) -> Option<i64> {
        value
            .substitute(&mut |id| self.default_value(id, depth).map(Linear::constant))?
            .as_constant()
    }

    /// The default value of a generic, when it is static.
    fn default_value(&self, id: DeclId, depth: u32) -> Option<i64> {
        if depth > MAX_DEPTH {
            return None;
        }
        if let Some(&value) = self.defaults.borrow().get(&id) {
            return value;
        }
        // A default defined in terms of itself stops here.
        self.defaults.borrow_mut().insert(id, None);
        let value = (|| {
            let decl = self.analysis.decl(id)?;
            if decl.kind != (DeclKind::Generic { has_default: true }) || self.is_foreign(decl) {
                return None;
            }
            let expr = initial_expression(&self.declaration(decl)?)?;
            let default = self.linear(&expr, id.unit, depth + 1)?;
            self.at_defaults(&default, depth + 1)
        })();
        self.defaults.borrow_mut().insert(id, value);
        value
    }

    // ---- Bounds and widths -----------------------------------------------------

    /// The index bounds of a one-dimensional array object, from its subtype
    /// indication or the subtypes and the type it names.
    pub(super) fn bounds(&self, id: DeclId, depth: u32) -> Option<LinBounds> {
        if let Some(bounds) = self.lin_bounds.borrow().get(&id) {
            return bounds.clone();
        }
        self.lin_bounds.borrow_mut().insert(id, None);
        let bounds = self.compute_bounds(id, depth);
        self.lin_bounds.borrow_mut().insert(id, bounds.clone());
        bounds
    }

    fn compute_bounds(&self, id: DeclId, depth: u32) -> Option<LinBounds> {
        let decl = self.analysis.decl(id)?;
        if self.is_foreign(decl) {
            return None;
        }
        let node = self.declaration(decl)?;
        let indication = node
            .children()
            .find(|n| n.kind() == SyntaxKind::SUBTYPE_INDICATION)?;
        self.indication_bounds_lin(&indication, id.unit, depth)
    }

    fn indication_bounds_lin(
        &self,
        indication: &SyntaxNode,
        unit: UnitId,
        depth: u32,
    ) -> Option<LinBounds> {
        use SyntaxKind::*;
        if depth > MAX_DEPTH {
            return None;
        }
        let mut mark = indication
            .children()
            .find(|n| matches!(n.kind(), CALL_EXPR | NAME | SELECTED_NAME))?;
        // `t(0 to 3)(7 downto 0)`: the index constraint comes first.
        while mark.kind() == CALL_EXPR {
            match mark.children().next() {
                Some(prefix) if prefix.kind() == CALL_EXPR => mark = prefix,
                _ => break,
            }
        }
        if mark.kind() == CALL_EXPR {
            let list = mark.children().find(|n| n.kind() == ASSOCIATION_LIST)?;
            return self.range_lin(&single_association(&list)?, unit, depth + 1);
        }
        let mark = self.target(unit, mark_token(&mark)?.text_range())?;
        self.mark_bounds_lin(mark, depth + 1)
    }

    /// The index bounds a constrained array subtype or type gives.
    fn mark_bounds_lin(&self, mark: DeclId, depth: u32) -> Option<LinBounds> {
        let decl = self.analysis.decl(mark)?;
        let node = self.declaration(decl)?;
        match decl.kind {
            DeclKind::Subtype => {
                let indication = node
                    .children()
                    .find(|n| n.kind() == SyntaxKind::SUBTYPE_INDICATION)?;
                self.indication_bounds_lin(&indication, mark.unit, depth)
            }
            DeclKind::Type => {
                let def = node
                    .children()
                    .find(|n| n.kind() == SyntaxKind::ARRAY_TYPE_DEF)?;
                let mut indexes = def
                    .children()
                    .filter(|n| n.kind() != SyntaxKind::SUBTYPE_INDICATION);
                let index = indexes.next()?;
                if indexes.next().is_some() {
                    return None;
                }
                self.range_lin(&index, mark.unit, depth)
            }
            _ => None,
        }
    }

    /// The bounds of a range of `unit`: `0 to W - 1`, `s'range`.
    pub(super) fn range_lin(
        &self,
        range: &SyntaxNode,
        unit: UnitId,
        depth: u32,
    ) -> Option<LinBounds> {
        match range.kind() {
            SyntaxKind::RANGE_EXPR => {
                let mut bounds = range.children();
                let left = self.linear(&bounds.next()?, unit, depth)?;
                let right = self.linear(&bounds.next()?, unit, depth)?;
                Some(LinBounds {
                    left,
                    right,
                    ascending: range.has_child(|k| k == SyntaxKind::TO_KW),
                })
            }
            SyntaxKind::ATTRIBUTE_NAME => {
                let prefix = range.children().next()?;
                if range.children().nth(1).is_some() {
                    return None;
                }
                let attribute = range.last_token()?.text().to_ascii_lowercase();
                let (id, decl) = self.named(unit, &prefix)?;
                if !(decl.kind.is_object() || decl.kind == DeclKind::Alias) {
                    return None;
                }
                let bounds = self.bounds(id, depth + 1)?;
                match attribute.as_str() {
                    "range" => Some(bounds),
                    "reverse_range" => Some(bounds.reversed()),
                    _ => None,
                }
            }
            _ => None,
        }
    }

    /// The number of elements of a vector object, as declared.
    fn declared_width(&self, id: DeclId, depth: u32) -> Option<Linear> {
        let decl = self.analysis.decl(id)?;
        // Name resolution worked out static widths already.
        if let Some(width) = decl.width {
            return Some(Linear::constant(width.into()));
        }
        if self.is_foreign(decl) {
            return None;
        }
        self.bounds(id, depth).map(|b| b.width())
    }

    /// The number of elements of a vector object: as declared, or for a
    /// constant of an unconstrained type, that of its value.
    fn object_width(&self, id: DeclId, depth: u32) -> Option<Linear> {
        if let Some(width) = self.analysis.decl(id)?.width {
            return Some(Linear::constant(width.into()));
        }
        if let Some(width) = self.widths.borrow().get(&id) {
            return width.clone();
        }
        // A constant whose value names itself stops here.
        self.widths.borrow_mut().insert(id, None);
        let width = self.compute_object_width(id, depth);
        self.widths.borrow_mut().insert(id, width.clone());
        width
    }

    fn compute_object_width(&self, id: DeclId, depth: u32) -> Option<Linear> {
        if let Some(width) = self.declared_width(id, depth) {
            return Some(width);
        }
        let decl = self.analysis.decl(id)?;
        if decl.kind != DeclKind::Constant || self.is_foreign(decl) {
            return None;
        }
        let expr = initial_expression(&self.declaration(decl)?)?;
        match self.shape(&expr, id.unit, depth + 1)? {
            Shape::Vector(width) => Some(width),
            Shape::Scalar => None,
        }
    }

    /// The width of the elements of a memory object: `mem_t` is `array (0
    /// to 3) of std_logic_vector(7 downto 0)`, or `t(0 to 3)(7 downto 0)`.
    fn element_width(&self, id: DeclId, depth: u32) -> Option<Linear> {
        let decl = self.analysis.decl(id)?;
        if self.is_foreign(decl) {
            return None;
        }
        let node = self.declaration(decl)?;
        let indication = node
            .children()
            .find(|n| n.kind() == SyntaxKind::SUBTYPE_INDICATION)?;
        self.indication_element_width(&indication, id.unit, depth)
    }

    fn indication_element_width(
        &self,
        indication: &SyntaxNode,
        unit: UnitId,
        depth: u32,
    ) -> Option<Linear> {
        use SyntaxKind::*;
        if depth > MAX_DEPTH {
            return None;
        }
        let mut mark = indication
            .children()
            .find(|n| matches!(n.kind(), CALL_EXPR | NAME | SELECTED_NAME))?;
        if mark.kind() == CALL_EXPR && mark.children().next()?.kind() == CALL_EXPR {
            let list = mark.children().find(|n| n.kind() == ASSOCIATION_LIST)?;
            return Some(
                self.range_lin(&single_association(&list)?, unit, depth + 1)?
                    .width(),
            );
        }
        while mark.kind() == CALL_EXPR {
            mark = mark.children().next()?;
        }
        let mark = self.target(unit, mark_token(&mark)?.text_range())?;
        let decl = self.analysis.decl(mark)?;
        let node = self.declaration(decl)?;
        match decl.kind {
            DeclKind::Subtype => {
                let indication = node.children().find(|n| n.kind() == SUBTYPE_INDICATION)?;
                self.indication_element_width(&indication, mark.unit, depth + 1)
            }
            DeclKind::Type => {
                let element = node
                    .children()
                    .find(|n| n.kind() == ARRAY_TYPE_DEF)?
                    .children()
                    .find(|n| n.kind() == SUBTYPE_INDICATION)?;
                Some(
                    self.indication_bounds_lin(&element, mark.unit, depth + 1)?
                        .width(),
                )
            }
            _ => None,
        }
    }

    // ---- Shapes of expressions ---------------------------------------------------

    /// Whether an expression of `unit` is a scalar or a vector, and how wide.
    pub(super) fn shape(&self, expr: &SyntaxNode, unit: UnitId, depth: u32) -> Option<Shape> {
        use SyntaxKind::*;
        if depth > MAX_DEPTH {
            return None;
        }
        let depth = depth + 1;
        match expr.kind() {
            LITERAL => {
                let token = expr.first_token()?;
                match token.kind() {
                    STRING_LITERAL | BIT_STRING_LITERAL => Some(Shape::Vector(Linear::constant(
                        literal_width(&token)?.into(),
                    ))),
                    CHAR_LITERAL | DECIMAL_LITERAL | BASED_LITERAL => Some(Shape::Scalar),
                    _ => None,
                }
            }
            PAREN_EXPR => self.shape(&expr.children().next()?, unit, depth),
            NAME | SELECTED_NAME => {
                let (id, decl) = self.named(unit, expr)?;
                if decl.kind == DeclKind::EnumLiteral {
                    return Some(Shape::Scalar);
                }
                if !(decl.kind.is_object() || decl.kind == DeclKind::Alias) {
                    return None;
                }
                if self.is_foreign(decl) {
                    return decl
                        .width
                        .map(|w| Shape::Vector(Linear::constant(w.into())));
                }
                match self.class(decl.subtype) {
                    Class::Scalar => Some(Shape::Scalar),
                    Class::Vector => Some(Shape::Vector(self.object_width(id, depth)?)),
                    _ => None,
                }
            }
            CALL_EXPR => self.call_shape(expr, unit, depth),
            QUALIFIED_EXPR => self.shape(&expr.children().last()?, unit, depth),
            ATTRIBUTE_NAME => None,
            UNARY_EXPR => match operator(expr)?.kind() {
                NOT_KW | MINUS | PLUS | ABS_KW => self.shape(&expr.children().next()?, unit, depth),
                // Reductions (`and v`), `??`.
                AND_KW | OR_KW | XOR_KW | NAND_KW | NOR_KW | XNOR_KW | QQ => Some(Shape::Scalar),
                _ => None,
            },
            BINARY_EXPR => self.binary_shape(expr, unit, depth),
            _ => None,
        }
    }

    fn call_shape(&self, expr: &SyntaxNode, unit: UnitId, depth: u32) -> Option<Shape> {
        use SyntaxKind::*;
        let prefix = expr.children().next()?;
        let list = expr.children().find(|n| n.kind() == ASSOCIATION_LIST)?;
        if prefix.kind() == CALL_EXPR {
            // `mem(i)(3 downto 0)`, `mem(i)(3)`.
            let Shape::Vector(_) = self.call_shape(&prefix, unit, depth + 1)? else {
                return None;
            };
            let index = single_association(&list)?;
            return match index.kind() {
                RANGE_EXPR | ATTRIBUTE_NAME => {
                    Some(Shape::Vector(self.range_lin(&index, unit, depth)?.width()))
                }
                _ => Some(Shape::Scalar),
            };
        }
        if let Some(function) = self.builtin_function(unit, &prefix) {
            return self.function_shape(function, &list, unit, depth);
        }
        let (id, decl) = self.named(unit, &prefix)?;
        if decl.kind.is_object() || decl.kind == DeclKind::Alias {
            let class = self.class(decl.subtype);
            let index = single_association(&list)?;
            let slice = matches!(index.kind(), RANGE_EXPR)
                || (index.kind() == ATTRIBUTE_NAME
                    && index
                        .last_token()
                        .is_some_and(|t| t.text().to_ascii_lowercase().ends_with("range")));
            match (class, slice) {
                (Class::Vector, true) => {
                    Some(Shape::Vector(self.range_lin(&index, unit, depth)?.width()))
                }
                (Class::Vector, false) => Some(Shape::Scalar),
                (Class::Memory, false) => Some(Shape::Vector(self.element_width(id, depth)?)),
                _ => None,
            }
        } else if matches!(decl.kind, DeclKind::Type | DeclKind::Subtype) {
            // A type conversion.
            if self.class(Some(id)) == Class::Scalar {
                return Some(Shape::Scalar);
            }
            let args = arguments(&list);
            let [(None, value)] = args.as_slice() else {
                return None;
            };
            match self.shape(value, unit, depth)? {
                Shape::Vector(width) => Some(Shape::Vector(width)),
                Shape::Scalar => None,
            }
        } else {
            None
        }
    }

    /// The result of a function of the bundled libraries.
    fn function_shape(
        &self,
        function: &str,
        list: &SyntaxNode,
        unit: UnitId,
        depth: u32,
    ) -> Option<Shape> {
        let args = arguments(list);
        let first = || argument(&args, 0, &["arg", "s", "l", "a"]);
        match function {
            "resize" | "to_unsigned" | "to_signed" | "to_slv" | "to_sulv" | "to_stdlogicvector"
            | "to_stdulogicvector" | "to_bitvector" | "to_bv" => {
                match argument(&args, 1, &["size", "new_size", "size_res"]) {
                    // A size, or (VHDL-2008) a vector of the size wanted.
                    Some(size) => match self.shape(size, unit, depth) {
                        Some(Shape::Vector(width)) => Some(Shape::Vector(width)),
                        _ => Some(Shape::Vector(self.linear(size, unit, depth)?)),
                    },
                    None if matches!(
                        function,
                        "to_stdlogicvector" | "to_stdulogicvector" | "to_bitvector"
                    ) =>
                    {
                        self.vector_shape(first()?, unit, depth)
                    }
                    None => None,
                }
            }
            "shift_left" | "shift_right" | "rotate_left" | "rotate_right" | "to_01" | "to_x01"
            | "to_x01z" | "to_ux01" => self.vector_shape(first()?, unit, depth),
            "to_integer" | "rising_edge" | "falling_edge" | "std_match" | "and_reduce"
            | "or_reduce" | "xor_reduce" | "to_bit" | "to_stdulogic" | "is_x" => {
                Some(Shape::Scalar)
            }
            _ => None,
        }
    }

    fn vector_shape(&self, expr: &SyntaxNode, unit: UnitId, depth: u32) -> Option<Shape> {
        match self.shape(expr, unit, depth)? {
            Shape::Vector(width) => Some(Shape::Vector(width)),
            Shape::Scalar => None,
        }
    }

    fn binary_shape(&self, expr: &SyntaxNode, unit: UnitId, depth: u32) -> Option<Shape> {
        use SyntaxKind::*;
        let operator = operator(expr)?;
        let mut operands = expr.children();
        let left = operands.next()?;
        let right = operands.next()?;
        let kind = operator.kind();
        if matches!(
            kind,
            EQ | NEQ | LT | LTE | GT | GTE | QEQ | QNEQ | QLT | QLTE | QGT | QGTE
        ) {
            return Some(Shape::Scalar);
        }
        let l = self.shape(&left, unit, depth)?;
        if matches!(kind, SLL_KW | SRL_KW | SLA_KW | SRA_KW | ROL_KW | ROR_KW) {
            return Some(l);
        }
        let r = self.shape(&right, unit, depth)?;
        if kind == AMP {
            let width = |s: Shape| match s {
                Shape::Scalar => Linear::constant(1),
                Shape::Vector(w) => w,
            };
            return Some(Shape::Vector(width(l).add(&width(r), 1)));
        }
        if !self.known_operator(unit, &operator) {
            return None;
        }
        match (kind, l, r) {
            (_, Shape::Scalar, Shape::Scalar) => Some(Shape::Scalar),
            // VHDL-2008 lets a bit and a vector be combined.
            (
                AND_KW | OR_KW | XOR_KW | NAND_KW | NOR_KW | XNOR_KW,
                Shape::Vector(a),
                Shape::Vector(b),
            ) => (a == b).then_some(Shape::Vector(a)),
            (
                AND_KW | OR_KW | XOR_KW | NAND_KW | NOR_KW | XNOR_KW | PLUS | MINUS | SLASH,
                Shape::Vector(w),
                Shape::Scalar,
            )
            | (
                AND_KW | OR_KW | XOR_KW | NAND_KW | NOR_KW | XNOR_KW | PLUS | MINUS,
                Shape::Scalar,
                Shape::Vector(w),
            ) => Some(Shape::Vector(w)),
            // `numeric_std`: the wider operand.
            (PLUS | MINUS, Shape::Vector(a), Shape::Vector(b)) => {
                let d = a.sub(&b).as_constant()?;
                Some(Shape::Vector(if d >= 0 { a } else { b }))
            }
            (STAR, Shape::Vector(a), Shape::Vector(b)) => Some(Shape::Vector(a.add(&b, 1))),
            (SLASH, Shape::Vector(a), Shape::Vector(_)) => Some(Shape::Vector(a)),
            (MOD_KW | REM_KW, _, Shape::Vector(b)) => Some(Shape::Vector(b)),
            (MOD_KW | REM_KW, Shape::Vector(a), Shape::Scalar) => Some(Shape::Vector(a)),
            _ => None,
        }
    }
}

/// How a width and the width of a value differ.
struct Difference {
    target: String,
    value: String,
    /// Found with the generics at their default values.
    defaults: bool,
}

impl Checker<'_, '_> {
    /// The difference between two widths: whatever the generics are, or
    /// with `generics`, at their default values.
    fn difference(&self, target: &Linear, value: &Linear, generics: bool) -> Option<Difference> {
        let values = self.values;
        match target.sub(value).as_constant() {
            Some(0) => None,
            Some(_) => Some(Difference {
                target: target.describe(values),
                value: value.describe(values),
                defaults: false,
            }),
            None if generics => {
                let (t, v) = (
                    values.at_defaults(target, 0)?,
                    values.at_defaults(value, 0)?,
                );
                (t != v).then(|| Difference {
                    target: t.to_string(),
                    value: v.to_string(),
                    defaults: true,
                })
            }
            None => None,
        }
    }

    fn vector_width(&self, expr: &SyntaxNode) -> Option<Linear> {
        match self.values.shape(expr, self.unit, 0)? {
            Shape::Vector(width) => Some(width),
            Shape::Scalar => None,
        }
    }

    /// Reports a value whose width is not its target's.
    fn width_mismatch(&mut self, value: &SyntaxNode, name: &str, width: &Linear, note: &str) {
        let Some(found) = self.vector_width(value) else {
            return;
        };
        let Some(d) = self.difference(width, &found, self.checks.width_generics) else {
            return;
        };
        if d.defaults && Self::in_conditional_generate(value) {
            return;
        }
        let suffix = if d.defaults {
            ", with generics at their default values"
        } else {
            ""
        };
        self.report(
            codes::WIDTH_MISMATCH,
            value.text_range(),
            format!(
                "`{name}` has {} elements{note} but the value has {}{suffix}",
                d.target, d.value
            ),
        );
    }

    // ---- Green filters -------------------------------------------------------

    /// Whether an assignment's target is a vector, a slice or a word of a
    /// memory.
    pub(super) fn assigns_vector(&self, statement: Green<'_>) -> bool {
        use SyntaxKind::*;
        let mut nodes = statement.nodes();
        let target = if statement.kind() == SELECTED_ASSIGN_STMT {
            // The target follows the selector.
            nodes.find(|n| n.kind() != LABEL).and(nodes.next())
        } else {
            nodes.find(|n| n.kind() != LABEL)
        };
        let Some(target) = target else {
            return false;
        };
        let name = match target.kind() {
            CALL_EXPR => target.nodes().next(),
            _ => Some(target),
        };
        name.and_then(|n| self.green_named(n))
            .is_some_and(|(_, d)| {
                (d.kind.is_object() || d.kind == DeclKind::Alias)
                    && matches!(self.values.class(d.subtype), Class::Vector | Class::Memory)
            })
    }

    /// Whether an object declaration gives a vector an initial value.
    pub(super) fn initializes_vector(&self, decl: Green<'_>) -> bool {
        if !decl.tokens().any(|(k, _)| k == SyntaxKind::COLON_EQ) {
            return false;
        }
        decl.nodes()
            .find(|n| n.kind() == SyntaxKind::IDENT_LIST)
            .and_then(|l| l.tokens().find(|(k, _)| *k == SyntaxKind::IDENT))
            .and_then(|(_, range)| self.decl_at(range))
            .and_then(|id| self.decl(id))
            .is_some_and(|d| self.values.class(d.subtype) == Class::Vector)
    }

    // ---- sema/width-mismatch ---------------------------------------------------

    /// The values of an assignment against its target.
    pub(super) fn width_assignment(&mut self, node: &SyntaxNode) {
        use SyntaxKind::*;
        let mut seen_select = node.kind() != SELECTED_ASSIGN_STMT;
        let mut target = None;
        let mut values = Vec::new();
        for element in node.children_with_tokens() {
            let n = match element {
                rowan::NodeOrToken::Token(t) => {
                    seen_select |= t.kind() == SELECT_KW;
                    continue;
                }
                rowan::NodeOrToken::Node(n) => n,
            };
            if n.kind() == LABEL || !seen_select {
                continue;
            }
            if target.is_none() {
                target = Some(n);
                continue;
            }
            if n.kind() != DELAY_MECHANISM {
                crate::rtl::assigned_values(&n, &mut values);
            }
        }
        let Some(target) = target else {
            return;
        };
        let name = match target.kind() {
            CALL_EXPR => target.children().next(),
            _ => Some(target.clone()),
        };
        let Some((_, decl)) = name.and_then(|n| self.named(&n)) else {
            return;
        };
        // A word of a memory is named with its index: `mem(i)`.
        let name = if target.kind() == CALL_EXPR && self.values.class(decl.subtype) == Class::Memory
        {
            target.text().to_string()
        } else {
            decl.spelling.clone()
        };
        let Some(width) = self.vector_width(&target) else {
            return;
        };
        for value in values {
            self.width_mismatch(&value, &name, &width, "");
        }
    }

    /// An object declaration's initial value against its width.
    pub(super) fn width_declaration(&mut self, node: &SyntaxNode) {
        let Some(first) = node
            .children()
            .find(|n| n.kind() == SyntaxKind::IDENT_LIST)
            .and_then(|l| l.first_token())
        else {
            return;
        };
        let Some(id) = self.decl_at(first.text_range()) else {
            return;
        };
        let Some(decl) = self.decl(id) else {
            return;
        };
        let Some(width) = self.values.declared_width(id, 0) else {
            return;
        };
        if let Some(value) = initial_expression(node) {
            let name = decl.spelling.clone();
            self.width_mismatch(&value, &name, &width, "");
        }
    }

    /// The associations of an instance's port and generic maps against the
    /// widths the instance gives its ports and generics.
    pub(super) fn width_instance(&mut self, node: &SyntaxNode) {
        use SyntaxKind::*;
        // The entity or component: the first name naming one.
        let Some((unit_id, unit)) = node
            .descendants_with_tokens()
            .filter_map(|e| e.into_token())
            .take_while(|t| !matches!(t.kind(), GENERIC_KW | PORT_KW))
            .filter(|t| matches!(t.kind(), SyntaxKind::IDENT | SyntaxKind::EXT_IDENT))
            .filter_map(|t| {
                let id = super::single_decl(self.reference_at(t.text_range())?)?;
                Some((id, self.decl(id)?))
            })
            .find(|(_, d)| matches!(d.kind, DeclKind::Entity | DeclKind::Component))
        else {
            return;
        };
        // Not a vendor primitive's stub, whose port widths depend on
        // generics it may not declare.
        if self.analysis.files()[unit.file as usize].builtin {
            return;
        }
        let Some(members) = unit.members.clone() else {
            return;
        };
        let interface = |ports: bool| -> Vec<DeclId> {
            members
                .iter()
                .filter(|&id| {
                    self.decl(id).is_some_and(|d| match d.kind {
                        DeclKind::Port { .. } => ports,
                        DeclKind::Generic { .. }
                        | DeclKind::GenericType
                        | DeclKind::GenericSubprogram
                        | DeclKind::GenericPackage => !ports,
                        _ => false,
                    })
                })
                .collect()
        };
        let aspect = |kind: SyntaxKind| {
            node.children()
                .find(|n| n.kind() == kind)
                .and_then(|a| a.children().find(|n| n.kind() == ASSOCIATION_LIST))
        };
        let generics = interface(false);
        let generic_map = aspect(GENERIC_MAP_ASPECT);
        let port_map = aspect(PORT_MAP_ASPECT);
        // The actual of each generic: `None` when it is not a linear
        // expression, absent when the default applies.
        let mut actuals: FxHashMap<DeclId, Option<Linear>> = FxHashMap::default();
        let mut associations = Vec::new();
        if let Some(list) = &generic_map {
            for (formal, actual) in self.associations(list, &generics) {
                if let Some(formal) = formal {
                    let value = match &actual {
                        Some(a) => self.values.linear(a, self.unit, 0),
                        None => continue,
                    };
                    actuals.insert(formal, value);
                    associations.push((formal, actual));
                }
            }
        }
        if let Some(list) = &port_map {
            let ports = interface(true);
            associations.extend(
                self.associations(list, &ports)
                    .into_iter()
                    .filter_map(|(f, a)| Some((f?, a))),
            );
        }
        let in_unit = unit_id.unit;
        for (formal, actual) in associations {
            let Some(actual) = actual else {
                continue;
            };
            let Some(decl) = self.decl(formal) else {
                continue;
            };
            if !self.values.is_vector(decl) {
                continue;
            }
            let Some(width) = self.values.declared_width(formal, 0) else {
                continue;
            };
            let Some(here) = self.instance_value(&width, in_unit, &mut actuals, 0) else {
                continue;
            };
            let note = if width.is_symbolic() {
                format!(" (`{}` in `{}`)", width.show(self.values), unit.spelling)
            } else {
                String::new()
            };
            let name = decl.spelling.clone();
            self.width_mismatch(&actual, &name, &here, &note);
        }
    }

    /// The formals and actuals of a generic or port map, the formal being
    /// a whole interface object; `None` for an actual left `open` or not
    /// analyzed (`inertial`).
    fn associations(
        &self,
        list: &SyntaxNode,
        interface: &[DeclId],
    ) -> Vec<(Option<DeclId>, Option<SyntaxNode>)> {
        use SyntaxKind::*;
        let mut out = Vec::new();
        for (position, element) in list
            .children()
            .filter(|n| n.kind() == ASSOCIATION_ELEMENT)
            .enumerate()
        {
            let named = element.has_child(|k| k == ARROW);
            let mut parts = element.children();
            let (formal, actual) = if named {
                // A whole formal; `f(3 downto 0) => x` and conversions are
                // left alone.
                let formal = match parts.next() {
                    Some(f) if f.kind() == NAME => self.named(&f).map(|(id, _)| id),
                    _ => None,
                };
                (formal, parts.next())
            } else {
                (interface.get(position).copied(), parts.next())
            };
            let actual =
                actual.filter(|_| !element.has_child(|k| matches!(k, OPEN_KW | INERTIAL_KW)));
            out.push((formal, actual));
        }
        out
    }

    /// A width of the instantiated unit `unit` with its generics replaced
    /// by their actuals, or their default values: a width of the
    /// instantiating unit.
    fn instance_value(
        &self,
        width: &Linear,
        unit: UnitId,
        actuals: &mut FxHashMap<DeclId, Option<Linear>>,
        depth: u32,
    ) -> Option<Linear> {
        if depth > MAX_DEPTH {
            return None;
        }
        width.substitute(&mut |id| {
            if id.unit != unit {
                return None;
            }
            if let Some(actual) = actuals.get(&id) {
                return actual.clone();
            }
            // Defaults are worked out once, and one that names itself
            // stops here.
            actuals.insert(id, None);
            let value = (|| {
                let decl = self.decl(id)?;
                if decl.kind != (DeclKind::Generic { has_default: true }) {
                    return None;
                }
                let expr = initial_expression(&self.values.declaration(decl)?)?;
                let default = self.values.linear(&expr, unit, 0)?;
                self.instance_value(&default, unit, actuals, depth + 1)
            })();
            actuals.insert(id, value.clone());
            value
        })
    }

    // ---- sema/comparison-width -------------------------------------------------

    /// Whether an expression names an object, rather than being made of
    /// literals only (`bit_vector'(x"0F")`).
    fn names_object(&self, expr: &SyntaxNode) -> bool {
        std::iter::once(expr.clone())
            .chain(expr.descendants())
            .filter(|n| matches!(n.kind(), SyntaxKind::NAME | SyntaxKind::SELECTED_NAME))
            .any(|n| {
                self.named(&n)
                    .is_some_and(|(_, d)| d.kind.is_object() || d.kind == DeclKind::Alias)
            })
    }

    /// The widths of the two sides of a comparison, when they differ.
    pub(super) fn compared_widths(
        &self,
        left: &SyntaxNode,
        right: &SyntaxNode,
    ) -> Option<(String, String, bool)> {
        let left = unparen(left);
        let right = unparen(right);
        let (l, r) = (self.vector_width(&left)?, self.vector_width(&right)?);
        if !(self.names_object(&left) || self.names_object(&right)) {
            return None;
        }
        let d = self.difference(&l, &r, self.checks.comparison_generics)?;
        Some((d.target, d.value, d.defaults))
    }

    // ---- sema/index-range ------------------------------------------------------

    /// An index or slice whose bounds depend on generics, against bounds
    /// that do too: reported when it is out of range whatever the generics
    /// are. Returns whether it was settled (reported or in range).
    pub(super) fn symbolic_index(&mut self, id: DeclId, name: &str, index: &SyntaxNode) -> bool {
        let values = self.values;
        let Some(bounds) = values.bounds(id, 0) else {
            return false;
        };
        let positive = |l: Linear| l.as_constant().is_some_and(|n| n > 0);
        let at_least_zero = |l: Linear| l.as_constant().is_some_and(|n| n >= 0);
        let message = if index.kind() == SyntaxKind::RANGE_EXPR {
            let Some(slice) = values.range_lin(index, self.unit, 0) else {
                return false;
            };
            if !(slice.is_symbolic() || bounds.is_symbolic()) {
                return false;
            }
            if positive(slice.lo().sub(slice.hi())) {
                format!(
                    "the slice `{}` of `{name}` is a null range: it has no elements",
                    slice.describe(values)
                )
            } else if positive(slice.hi().sub(bounds.hi())) || positive(bounds.lo().sub(slice.lo()))
            {
                format!(
                    "the slice `{}` is outside the range of `{name}` ({})",
                    slice.describe(values),
                    bounds.describe(values)
                )
            } else {
                return at_least_zero(bounds.hi().sub(slice.hi()))
                    && at_least_zero(slice.lo().sub(bounds.lo()));
            }
        } else {
            let Some(value) = values.linear(index, self.unit, 0) else {
                return false;
            };
            if !(value.is_symbolic() || bounds.is_symbolic()) {
                return false;
            }
            if positive(value.sub(bounds.hi())) || positive(bounds.lo().sub(&value)) {
                format!(
                    "index {} is outside the range of `{name}` ({})",
                    value.describe(values),
                    bounds.describe(values)
                )
            } else {
                return at_least_zero(bounds.hi().sub(&value))
                    && at_least_zero(value.sub(bounds.lo()));
            }
        };
        self.report(codes::INDEX_RANGE, index.text_range(), message);
        true
    }
}

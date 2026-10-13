//! Hardware checks on resolved units: width mismatches in assignments and
//! associations, latches inferred by combinational processes, signals
//! that cross clock domains, input ports left open, resets, and state
//! machines.

mod fsm;
mod reset;

use std::collections::{HashMap, HashSet};

use vhone_syntax::{SyntaxKind, SyntaxNode, SyntaxToken, TextRange};

use crate::analysis::{Ctx, UnitResult};
use crate::decl::{Decl, DeclId, DeclKind, Target, TypeDef, UnitId};
use crate::diagnostic::{Diagnostic, codes};
use crate::resolver::{is_clock_edge, mark_token};
use crate::typeck::FxMap;

// ---- Static values ---------------------------------------------------------

/// How deep expressions are followed.
const MAX_DEPTH: u32 = 64;

/// The value of a static integer expression: literals, constants whose
/// value is known (through `name`), `+ - * / mod rem **`, parentheses.
pub(crate) fn static_int(
    expr: &SyntaxNode,
    name: &dyn Fn(&SyntaxNode) -> Option<i64>,
) -> Option<i64> {
    static_int_at(expr, name, 0)
}

fn static_int_at(
    expr: &SyntaxNode,
    name: &dyn Fn(&SyntaxNode) -> Option<i64>,
    depth: u32,
) -> Option<i64> {
    use SyntaxKind::*;
    if depth > MAX_DEPTH {
        return None;
    }
    let depth = depth + 1;
    match expr.kind() {
        LITERAL => {
            let token = expr.first_token()?;
            if token.kind() != DECIMAL_LITERAL {
                return None;
            }
            let text = token.text().replace('_', "");
            if let Some((mantissa, exponent)) = text.split_once(['e', 'E']) {
                let exponent: u32 = exponent.trim_start_matches('+').parse().ok()?;
                return mantissa
                    .parse::<i64>()
                    .ok()?
                    .checked_mul(10i64.checked_pow(exponent)?);
            }
            text.parse().ok()
        }
        PAREN_EXPR => static_int_at(&expr.children().next()?, name, depth),
        NAME | SELECTED_NAME => name(expr),
        UNARY_EXPR => {
            let operand = static_int_at(&expr.children().next()?, name, depth)?;
            match operator(expr)? {
                MINUS => operand.checked_neg(),
                PLUS => Some(operand),
                ABS_KW => operand.checked_abs(),
                _ => None,
            }
        }
        BINARY_EXPR => {
            let mut operands = expr.children();
            let left = static_int_at(&operands.next()?, name, depth)?;
            let right = static_int_at(&operands.next()?, name, depth)?;
            match operator(expr)? {
                PLUS => left.checked_add(right),
                MINUS => left.checked_sub(right),
                STAR => left.checked_mul(right),
                SLASH => left.checked_div(right),
                MOD_KW => left
                    .checked_rem_euclid(right)
                    .map(|r| if right < 0 && r != 0 { r + right } else { r }),
                REM_KW => left.checked_rem(right),
                STAR2 => left.checked_pow(u32::try_from(right).ok()?),
                _ => None,
            }
        }
        _ => None,
    }
}

/// The operator token of a unary or binary expression.
fn operator(expr: &SyntaxNode) -> Option<SyntaxKind> {
    expr.children_with_tokens()
        .filter_map(|e| e.into_token())
        .find(|t| !t.kind().is_trivia())
        .map(|t| t.kind())
}

/// The number of values in `left to right` or `left downto right`.
pub(crate) fn range_width(
    range: &SyntaxNode,
    name: &dyn Fn(&SyntaxNode) -> Option<i64>,
) -> Option<u32> {
    if range.kind() != SyntaxKind::RANGE_EXPR {
        return None;
    }
    let mut bounds = range.children();
    let left = static_int(&bounds.next()?, name)?;
    let right = static_int(&bounds.next()?, name)?;
    let ascending = range
        .children_with_tokens()
        .any(|e| e.kind() == SyntaxKind::TO_KW);
    let count = if ascending {
        right - left
    } else {
        left - right
    } + 1;
    u32::try_from(count.max(0)).ok()
}

/// The width a one-dimensional index constraint gives: `(7 downto 0)`.
pub(crate) fn constraint_width(
    list: &SyntaxNode,
    name: &dyn Fn(&SyntaxNode) -> Option<i64>,
) -> Option<u32> {
    let mut elements = list
        .children()
        .filter(|n| n.kind() == SyntaxKind::ASSOCIATION_ELEMENT);
    let element = elements.next()?;
    if elements.next().is_some() {
        return None;
    }
    let mut inner = element.children();
    let range = inner.next()?;
    if inner.next().is_some() {
        return None;
    }
    range_width(&range, name)
}

/// The number of elements a string or bit string literal has.
fn literal_width(token: &SyntaxToken) -> Option<u32> {
    let text = token.text();
    match token.kind() {
        SyntaxKind::STRING_LITERAL => {
            let inner = text.get(1..text.len().checked_sub(1)?)?;
            Some(inner.replace("\"\"", "\"").chars().count() as u32)
        }
        SyntaxKind::BIT_STRING_LITERAL => {
            let quote = text.find('"')?;
            let (prefix, rest) = text.split_at(quote);
            let digits: String = prefix.chars().take_while(char::is_ascii_digit).collect();
            if !digits.is_empty() {
                return digits.parse().ok();
            }
            let base = prefix.chars().last()?.to_ascii_lowercase();
            let content = rest.trim_matches('"').replace('_', "");
            let n = content.chars().count() as u32;
            match base {
                'b' => Some(n),
                'o' => Some(3 * n),
                'x' => Some(4 * n),
                _ => None,
            }
        }
        _ => None,
    }
}

// ---- The checks --------------------------------------------------------

/// Runs the checks on one unit; `refs_at` and `decls_at` are the type
/// checker's tables of references and declarations by range.
pub(crate) fn check_unit(
    ctx: &Ctx,
    unit: UnitId,
    design_unit: &SyntaxNode,
    result: &UnitResult,
    refs_at: &FxMap<TextRange, usize>,
    decls_at: &FxMap<TextRange, DeclId>,
) -> Vec<Diagnostic> {
    let file = ctx.units[unit as usize].file;
    let mut checker = Checker {
        ctx,
        unit,
        file,
        result,
        refs_at,
        decls_at,
        by_start: std::cell::OnceCell::new(),
        clocked: Vec::new(),
        reset_uses: Vec::new(),
        reset_ranges: Vec::new(),
        cases: Vec::new(),
        diags: Vec::new(),
    };
    checker.walk(design_unit);
    checker.clock_domains();
    checker.reset_consistency();
    checker.state_machines(design_unit);
    checker.diags
}

struct Checker<'a> {
    ctx: &'a Ctx<'a>,
    unit: UnitId,
    file: u32,
    result: &'a UnitResult,
    refs_at: &'a FxMap<TextRange, usize>,
    /// This unit's declarations by name range.
    decls_at: &'a FxMap<TextRange, DeclId>,
    /// Reference indexes in source order, built when needed.
    by_start: std::cell::OnceCell<Vec<usize>>,
    /// Clocked processes without `wait`, with their clock.
    clocked: Vec<(DeclId, SyntaxNode)>,
    /// The resets of clocked processes.
    reset_uses: Vec<reset::ResetUse>,
    /// The statements that run on a reset.
    reset_ranges: Vec<TextRange>,
    /// `case` statements, for the state machine checks.
    cases: Vec<SyntaxNode>,
    diags: Vec<Diagnostic>,
}

impl<'a> Checker<'a> {
    fn decl(&self, id: DeclId) -> Option<&'a Decl> {
        if id.unit == self.unit {
            self.result.decls.get(id.index as usize)
        } else {
            self.ctx.decl(id)
        }
    }

    /// The single declaration a token names.
    fn target(&self, token: &SyntaxToken) -> Option<DeclId> {
        let r = &self.result.refs[*self.refs_at.get(&token.text_range())?];
        let mut ids = r.targets.iter().filter_map(|t| match t {
            Target::Decl(id) => Some(*id),
            Target::Library(_) => None,
        });
        let id = ids.next()?;
        ids.next().is_none().then_some(id)
    }

    /// The declaration a name (`s`, `pkg.s`) refers to.
    fn named(&self, name: &SyntaxNode) -> Option<&'a Decl> {
        if !matches!(name.kind(), SyntaxKind::NAME | SyntaxKind::SELECTED_NAME) {
            return None;
        }
        self.decl(self.target(&mark_token(name)?)?)
    }

    fn named_value(&self, name: &SyntaxNode) -> Option<i64> {
        self.named(name)?.value
    }

    fn type_def(&self, mut mark: DeclId) -> Option<&'a TypeDef> {
        for _ in 0..32 {
            let decl = self.decl(mark)?;
            match decl.kind {
                DeclKind::Type => return decl.type_def.as_deref(),
                DeclKind::Subtype => mark = decl.subtype?,
                _ => return None,
            }
        }
        None
    }

    /// Whether an object holds one scalar value rather than an array.
    fn is_scalar(&self, decl: &Decl) -> bool {
        decl.subtype
            .and_then(|s| self.type_def(s))
            .is_some_and(|d| {
                matches!(
                    d,
                    TypeDef::Enum(_)
                        | TypeDef::Integer
                        | TypeDef::Real
                        | TypeDef::Physical
                        | TypeDef::Scalar
                )
            })
    }

    /// The definition of the type of an object.
    fn object_type(&self, decl: &Decl) -> Option<&'a TypeDef> {
        self.type_def(decl.subtype?)
    }

    /// Whether an object is an array of scalars, such as a vector of bits:
    /// what widths are checked for.
    fn is_vector(&self, decl: &Decl) -> bool {
        match self.object_type(decl) {
            Some(TypeDef::Array {
                indexes,
                element: Some(element),
            }) if indexes.len() == 1 => self.type_def(*element).is_some_and(|d| {
                matches!(d, TypeDef::Enum(_) | TypeDef::Integer | TypeDef::Scalar)
            }),
            _ => false,
        }
    }

    /// Whether an object is an array of arrays: a memory.
    fn is_memory(&self, decl: &Decl) -> bool {
        match self.object_type(decl) {
            Some(TypeDef::Array {
                element: Some(element),
                ..
            }) => matches!(self.type_def(*element), Some(TypeDef::Array { .. })),
            _ => false,
        }
    }

    /// The number of elements an expression has, when it is static.
    fn width(&self, expr: &SyntaxNode) -> Option<u32> {
        self.width_at(expr, 0)
    }

    fn width_at(&self, expr: &SyntaxNode, depth: u32) -> Option<u32> {
        use SyntaxKind::*;
        if depth > MAX_DEPTH {
            return None;
        }
        let depth = depth + 1;
        let value = |n: &SyntaxNode| self.named_value(n);
        match expr.kind() {
            LITERAL => literal_width(&expr.first_token()?),
            PAREN_EXPR => self.width_at(&expr.children().next()?, depth),
            NAME | SELECTED_NAME => {
                let decl = self.named(expr)?;
                if decl.kind.is_object() || decl.kind == DeclKind::Alias {
                    decl.width
                } else {
                    None
                }
            }
            CALL_EXPR => {
                let prefix = expr.children().next()?;
                let list = expr.children().find(|n| n.kind() == ASSOCIATION_LIST)?;
                let decl = self.named(&prefix)?;
                if decl.kind.is_object() || decl.kind == DeclKind::Alias {
                    // A slice.
                    constraint_width(&list, &value)
                } else if matches!(decl.kind, DeclKind::Type | DeclKind::Subtype) {
                    // A type conversion.
                    let mut elements = list.children();
                    let element = elements.next()?;
                    if elements.next().is_some() {
                        return None;
                    }
                    self.width_at(&element.children().next()?, depth)
                } else {
                    None
                }
            }
            QUALIFIED_EXPR => self.width_at(&expr.children().last()?, depth),
            UNARY_EXPR => match operator(expr)? {
                NOT_KW | MINUS | PLUS | ABS_KW => self.width_at(&expr.children().next()?, depth),
                _ => None,
            },
            BINARY_EXPR => {
                let mut operands = expr.children();
                let left = operands.next()?;
                let right = operands.next()?;
                match operator(expr)? {
                    AMP => Some(self.part_width(&left, depth)? + self.part_width(&right, depth)?),
                    AND_KW | OR_KW | XOR_KW | NAND_KW | NOR_KW | XNOR_KW => {
                        let (l, r) = (self.width_at(&left, depth)?, self.width_at(&right, depth)?);
                        (l == r).then_some(l)
                    }
                    PLUS | MINUS => {
                        // `numeric_std`: the wider operand; an integer
                        // operand takes the other's width.
                        let l = self.width_at(&left, depth);
                        let r = self.width_at(&right, depth);
                        match (l, r) {
                            (Some(l), Some(r)) => Some(l.max(r)),
                            (Some(w), None) if static_int(&right, &value).is_some() => Some(w),
                            (None, Some(w)) if static_int(&left, &value).is_some() => Some(w),
                            _ => None,
                        }
                    }
                    _ => None,
                }
            }
            _ => None,
        }
    }

    /// The width of an operand of `&`: a scalar counts as one element.
    fn part_width(&self, expr: &SyntaxNode, depth: u32) -> Option<u32> {
        if expr.kind() == SyntaxKind::LITERAL
            && expr.first_token()?.kind() == SyntaxKind::CHAR_LITERAL
        {
            return Some(1);
        }
        if let Some(decl) = self.named(expr)
            && (decl.kind.is_object() || decl.kind == DeclKind::Alias)
            && self.is_scalar(decl)
        {
            return Some(1);
        }
        self.width_at(expr, depth)
    }

    /// The width of an assignment target: a name or a slice.
    fn target_width(&self, target: &SyntaxNode) -> Option<(u32, String)> {
        let name = match target.kind() {
            SyntaxKind::CALL_EXPR => target.children().next()?,
            _ => target.clone(),
        };
        let decl = self.named(&name)?;
        if !(decl.kind.is_object() || decl.kind == DeclKind::Alias) || !self.is_vector(decl) {
            return None;
        }
        Some((self.width(target)?, decl.spelling.clone()))
    }

    fn mismatch(&self, target: (u32, String), value: &SyntaxNode) -> Option<Diagnostic> {
        let width = self.width(value)?;
        if width == target.0 {
            return None;
        }
        Some(Diagnostic {
            code: codes::WIDTH_MISMATCH,
            file: self.file,
            range: value.text_range(),
            message: format!(
                "`{}` has {} elements but the value has {width}",
                target.1, target.0
            ),
            fix: None,
        })
    }

    fn check_value(&mut self, target: &Option<(u32, String)>, value: &SyntaxNode) {
        if let Some(target) = target
            && let Some(diag) = self.mismatch(target.clone(), value)
        {
            self.diags.push(diag);
        }
    }

    /// The values of a waveform, conditional waveforms or selected
    /// waveforms: every expression that is assigned.
    fn assigned_values(node: &SyntaxNode, out: &mut Vec<SyntaxNode>) {
        use SyntaxKind::*;
        match node.kind() {
            WAVEFORM => {
                for element in node.children().filter(|n| n.kind() == WAVEFORM_ELEMENT) {
                    // `value after delay`: the first expression.
                    if let Some(value) = element.children().next()
                        && !element.children_with_tokens().any(|e| e.kind() == NULL_KW)
                    {
                        out.push(value);
                    }
                }
            }
            CONDITIONAL_WAVEFORMS | SELECTED_WAVEFORMS => {
                for child in node.children().filter(|n| n.kind() == WAVEFORM) {
                    Self::assigned_values(&child, out);
                }
            }
            CONDITIONAL_EXPR => {
                // `a when c else b`: the expressions before `when` and
                // after the last `else`.
                let mut after_when = false;
                for element in node.children_with_tokens() {
                    match element {
                        rowan::NodeOrToken::Token(t) => match t.kind() {
                            WHEN_KW => after_when = true,
                            ELSE_KW => after_when = false,
                            _ => {}
                        },
                        rowan::NodeOrToken::Node(n) if !after_when => out.push(n),
                        rowan::NodeOrToken::Node(_) => {}
                    }
                }
            }
            _ => out.push(node.clone()),
        }
    }

    fn assignment(&mut self, node: &SyntaxNode) {
        use SyntaxKind::*;
        let selected = node.kind() == SELECTED_ASSIGN_STMT;
        let mut seen_select = !selected;
        let mut target = None;
        let mut target_seen = false;
        let mut values = Vec::new();
        for element in node.children_with_tokens() {
            let n = match element {
                rowan::NodeOrToken::Token(t) => {
                    if t.kind() == SELECT_KW {
                        seen_select = true;
                    }
                    continue;
                }
                rowan::NodeOrToken::Node(n) => n,
            };
            if n.kind() == LABEL || !seen_select {
                continue;
            }
            if !target_seen {
                target_seen = true;
                target = self.target_width(&n);
                continue;
            }
            if n.kind() != DELAY_MECHANISM {
                Self::assigned_values(&n, &mut values);
            }
        }
        for value in values {
            self.check_value(&target, &value);
        }
    }

    /// An object declaration's initial value against its width.
    fn object_decl(&mut self, node: &SyntaxNode) {
        let Some(list) = node.children().find(|n| n.kind() == SyntaxKind::IDENT_LIST) else {
            return;
        };
        let Some(first) = list.first_token() else {
            return;
        };
        let Some(decl) = self
            .decls_at
            .get(&first.text_range())
            .and_then(|&id| self.decl(id))
        else {
            return;
        };
        let Some(width) = decl.width.filter(|_| self.is_vector(decl)) else {
            return;
        };
        let after_default = node
            .children_with_tokens()
            .skip_while(|e| e.kind() != SyntaxKind::COLON_EQ)
            .find_map(|e| e.into_node());
        if let Some(value) = after_default {
            self.check_value(&Some((width, decl.spelling.clone())), &value);
        }
    }

    /// The associations of a port or generic map against the instantiated
    /// unit's interface.
    fn associations(&mut self, list: &SyntaxNode, interface: &[DeclId], ports: bool) {
        for (position, element) in list
            .children()
            .filter(|n| n.kind() == SyntaxKind::ASSOCIATION_ELEMENT)
            .enumerate()
        {
            let named = element
                .children_with_tokens()
                .any(|e| e.kind() == SyntaxKind::ARROW);
            let mut parts = element.children();
            let (formal, actual) = if named {
                let formal = parts.next();
                (formal, parts.next())
            } else {
                (None, parts.next())
            };
            let Some(actual) = actual else {
                // `p => open`.
                if ports
                    && element
                        .children_with_tokens()
                        .any(|e| e.kind() == SyntaxKind::OPEN_KW)
                {
                    self.open_input(&element, formal.as_ref(), interface.get(position));
                }
                continue;
            };
            if actual
                .descendants_with_tokens()
                .any(|e| matches!(e.kind(), SyntaxKind::OPEN_KW | SyntaxKind::INERTIAL_KW))
            {
                continue;
            }
            let formal_width = match formal {
                // A whole formal; `f(3 downto 0) => x` and conversions are
                // left alone.
                Some(f) if matches!(f.kind(), SyntaxKind::NAME) => self.named(&f),
                Some(_) => None,
                None => interface.get(position).and_then(|&id| self.decl(id)),
            }
            .filter(|d| self.is_vector(d))
            .and_then(|d| Some((d.width?, d.spelling.clone())));
            self.check_value(&formal_width, &actual);
        }
    }

    /// An input port without a default associated with `open`.
    fn open_input(
        &mut self,
        element: &SyntaxNode,
        formal: Option<&SyntaxNode>,
        positional: Option<&DeclId>,
    ) {
        let decl = match formal {
            Some(f) if f.kind() == SyntaxKind::NAME => self.named(f),
            Some(_) => None,
            None => positional.and_then(|&id| self.decl(id)),
        };
        if let Some(decl) = decl
            && let DeclKind::Port {
                mode: crate::decl::Mode::In,
                has_default: false,
            } = decl.kind
        {
            self.diags.push(Diagnostic {
                code: codes::OPEN_INPUT,
                file: self.file,
                range: element.text_range(),
                message: format!(
                    "input port `{}` is left open and has no default value",
                    decl.spelling
                ),
                fix: None,
            });
        }
    }

    fn instantiation(&mut self, node: &SyntaxNode) {
        // The entity or component: the first name naming one.
        let unit = node
            .descendants_with_tokens()
            .filter_map(|e| e.into_token())
            .take_while(|t| !matches!(t.kind(), SyntaxKind::GENERIC_KW | SyntaxKind::PORT_KW))
            .filter(|t| matches!(t.kind(), SyntaxKind::IDENT | SyntaxKind::EXT_IDENT))
            .filter_map(|t| self.target(&t))
            .find(|&id| {
                self.decl(id)
                    .is_some_and(|d| matches!(d.kind, DeclKind::Entity | DeclKind::Component))
            });
        let members = unit.and_then(|u| self.decl(u)?.members.clone());
        for aspect in node.children() {
            let ports = match aspect.kind() {
                SyntaxKind::PORT_MAP_ASPECT => true,
                SyntaxKind::GENERIC_MAP_ASPECT => false,
                _ => continue,
            };
            let interface: Vec<DeclId> = members
                .iter()
                .flat_map(|m| m.iter())
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
                .collect();
            if let Some(list) = aspect
                .children()
                .find(|n| n.kind() == SyntaxKind::ASSOCIATION_LIST)
            {
                self.associations(&list, &interface, ports);
            }
        }
    }

    /// Visits the tree without recursion: expressions can nest deeply.
    fn walk(&mut self, root: &SyntaxNode) {
        use SyntaxKind::*;
        let mut events = root.preorder();
        while let Some(event) = events.next() {
            let rowan::WalkEvent::Enter(node) = event else {
                continue;
            };
            match node.kind() {
                SIGNAL_ASSIGN_STMT | VARIABLE_ASSIGN_STMT | SELECTED_ASSIGN_STMT => {
                    self.assignment(&node);
                    events.skip_subtree();
                }
                SIGNAL_DECL | VARIABLE_DECL | CONSTANT_DECL | INTERFACE_OBJECT_DECL => {
                    self.object_decl(&node);
                    events.skip_subtree();
                }
                COMPONENT_INSTANTIATION_STMT => {
                    self.instantiation(&node);
                    events.skip_subtree();
                }
                PROCESS_STMT => self.process(&node),
                CASE_STMT => self.cases.push(node.clone()),
                // Nothing checked inside expressions and the like.
                ERROR | BINARY_EXPR | UNARY_EXPR | PAREN_EXPR | AGGREGATE | LITERAL | NAME
                | SELECTED_NAME | CALL_EXPR | ATTRIBUTE_NAME | QUALIFIED_EXPR
                | SUBTYPE_INDICATION | RANGE_EXPR | CHOICES | ASSERT_STMT | REPORT_STMT
                | WAIT_STMT | RETURN_STMT | PROCEDURE_CALL_STMT | NEXT_STMT | EXIT_STMT
                | SENSITIVITY_LIST | LABEL | TYPE_DECL | SUBTYPE_DECL | ALIAS_DECL
                | ATTRIBUTE_DECL | ATTRIBUTE_SPEC | USE_CLAUSE | LIBRARY_CLAUSE => {
                    events.skip_subtree()
                }
                _ => {}
            }
        }
    }

    // ---- Latches ----------------------------------------------------------

    /// The signal a target token writes.
    fn written_signal(&self, token: &SyntaxToken) -> Option<DeclId> {
        let r = &self.result.refs[*self.refs_at.get(&token.text_range())?];
        if !r.access.writes() {
            return None;
        }
        r.targets.iter().find_map(|t| match t {
            Target::Decl(id) => self.decl(*id).filter(|d| d.kind.is_signal()).map(|_| *id),
            Target::Library(_) => None,
        })
    }

    /// Signals a combinational process assigns on some paths but not all.
    /// Scans a process once for what the latch and clock domain checks
    /// need.
    fn process(&mut self, process: &SyntaxNode) {
        let mut waits = 0;
        let mut edge = None;
        let mut error = false;
        let mut delayed = false;
        for n in process.descendants() {
            match n.kind() {
                SyntaxKind::WAIT_STMT => waits += 1,
                SyntaxKind::ERROR => error = true,
                SyntaxKind::WAVEFORM_ELEMENT => {
                    delayed |= n
                        .children_with_tokens()
                        .any(|e| e.kind() == SyntaxKind::AFTER_KW)
                }
                SyntaxKind::CALL_EXPR | SyntaxKind::ATTRIBUTE_NAME
                    if edge.is_none() && is_clock_edge(&n) =>
                {
                    edge = Some(n)
                }
                _ => {}
            }
        }
        if error {
            return;
        }
        match edge {
            Some(edge) => {
                // Testbench processes wait on events rather than clocks.
                if waits == 0
                    && let Some(clock) = self.clock_of(&edge)
                {
                    self.resets(process, &edge, clock);
                    self.clocked.push((clock, process.clone()));
                }
            }
            // Delayed assignments (`after 10 ns`) are simulation models.
            None if !delayed => self.latches(process, waits),
            None => {}
        }
    }

    fn latches(&mut self, process: &SyntaxNode, waits: usize) {
        let has_list = process
            .children()
            .any(|n| n.kind() == SyntaxKind::SENSITIVITY_LIST);
        let body: Vec<SyntaxNode> = process
            .children_with_tokens()
            .skip_while(|e| e.kind() != SyntaxKind::BEGIN_KW)
            .filter_map(|e| e.into_node())
            .collect();
        // `wait on a, b;` as the last statement is a sensitivity list.
        let trailing_wait_on = waits == 1
            && body.last().is_some_and(|last| {
                last.kind() == SyntaxKind::WAIT_STMT && {
                    let has = |k| last.children_with_tokens().any(|e| e.kind() == k);
                    has(SyntaxKind::ON_KW) && !has(SyntaxKind::UNTIL_KW) && !has(SyntaxKind::FOR_KW)
                }
            });
        let combinational = if has_list {
            waits == 0
        } else {
            trailing_wait_on
        };
        if !combinational {
            return;
        }
        let mut paths = Paths::default();
        let always = self.sequence(&body, &mut paths);
        let mut latched: Vec<(DeclId, TextRange)> = paths
            .first
            .iter()
            .filter(|(id, _)| !always.contains(id) && !paths.partial.contains(id))
            .map(|(id, range)| (*id, *range))
            .collect();
        latched.sort_by_key(|(_, r)| r.start());
        for (id, range) in latched {
            let name = self
                .decl(id)
                .map(|d| d.spelling.clone())
                .unwrap_or_default();
            self.diags.push(Diagnostic {
                code: codes::INFERRED_LATCH,
                file: self.file,
                range,
                message: format!(
                    "`{name}` is not assigned on every path through this combinational process, which infers a latch"
                ),
                fix: None,
            });
        }
    }

    /// The signals a sequence of statements assigns on every path.
    fn sequence(&self, statements: &[SyntaxNode], paths: &mut Paths) -> HashSet<DeclId> {
        let mut always = HashSet::new();
        for statement in statements {
            always.extend(self.statement(statement, paths));
        }
        always
    }

    fn statement(&self, node: &SyntaxNode, paths: &mut Paths) -> HashSet<DeclId> {
        use SyntaxKind::*;
        match node.kind() {
            SIGNAL_ASSIGN_STMT | SELECTED_ASSIGN_STMT => {
                let selected = node.kind() == SELECTED_ASSIGN_STMT;
                let target = node
                    .children_with_tokens()
                    .skip_while(|e| selected && e.kind() != SELECT_KW)
                    .filter_map(|e| e.into_node())
                    .find(|n| n.kind() != LABEL);
                let Some(target) = target else {
                    return HashSet::new();
                };
                // `s <= a when c;` without a final `else` may not assign.
                let complete = node
                    .children()
                    .find(|n| n.kind() == CONDITIONAL_WAVEFORMS)
                    .is_none_or(|w| {
                        let count = |k| w.children_with_tokens().filter(|e| e.kind() == k).count();
                        count(ELSE_KW) >= count(WHEN_KW)
                    });
                self.assigned(&target, complete, paths)
            }
            PROCEDURE_CALL_STMT => {
                // Signals passed whole to `out` parameters count as
                // assigned, but are never reported on their own: the
                // parameter modes of overloads are not always known.
                let mut always = HashSet::new();
                for name in node
                    .descendants()
                    .filter(|n| n.kind() == ASSOCIATION_ELEMENT)
                    .filter_map(|e| e.children().last())
                    .filter(|n| n.kind() == NAME)
                {
                    if let Some(token) = mark_token(&name)
                        && let Some(id) = self.written_signal(&token)
                    {
                        always.insert(id);
                    }
                }
                always
            }
            IF_STMT => {
                let mut branches: Vec<Vec<SyntaxNode>> = Vec::new();
                let mut has_else = false;
                let mut in_body = false;
                for element in node.children_with_tokens() {
                    match element {
                        rowan::NodeOrToken::Token(t) => match t.kind() {
                            IF_KW | ELSIF_KW => in_body = false,
                            THEN_KW => {
                                in_body = true;
                                branches.push(Vec::new());
                            }
                            ELSE_KW => {
                                in_body = true;
                                has_else = true;
                                branches.push(Vec::new());
                            }
                            END_KW => in_body = false,
                            _ => {}
                        },
                        rowan::NodeOrToken::Node(n) => {
                            if in_body && let Some(branch) = branches.last_mut() {
                                branch.push(n);
                            }
                        }
                    }
                }
                let sets: Vec<HashSet<DeclId>> =
                    branches.iter().map(|b| self.sequence(b, paths)).collect();
                if has_else {
                    intersection(sets)
                } else {
                    HashSet::new()
                }
            }
            CASE_STMT => {
                let sets: Vec<HashSet<DeclId>> = node
                    .children()
                    .filter(|n| n.kind() == CASE_ALTERNATIVE)
                    .map(|alternative| {
                        let body: Vec<SyntaxNode> = alternative
                            .children_with_tokens()
                            .skip_while(|e| e.kind() != ARROW)
                            .filter_map(|e| e.into_node())
                            .collect();
                        self.sequence(&body, paths)
                    })
                    .collect();
                intersection(sets)
            }
            LOOP_STMT | SEQUENTIAL_BLOCK_STMT => {
                // A loop is assumed to run its body.
                let body: Vec<SyntaxNode> = node
                    .children()
                    .filter(|n| !matches!(n.kind(), LABEL | FOR_SCHEME | WHILE_SCHEME))
                    .collect();
                self.sequence(&body, paths)
            }
            _ => HashSet::new(),
        }
    }

    /// Records an assignment to `target`; returns the signals it assigns
    /// whole when `complete`.
    fn assigned(&self, target: &SyntaxNode, complete: bool, paths: &mut Paths) -> HashSet<DeclId> {
        use SyntaxKind::*;
        let mut always = HashSet::new();
        match target.kind() {
            NAME => {
                if let Some(token) = mark_token(target)
                    && let Some(id) = self.written_signal(&token)
                {
                    paths.first.entry(id).or_insert(token.text_range());
                    if complete {
                        always.insert(id);
                    }
                }
            }
            AGGREGATE => {
                for name in target.descendants().filter(|n| n.kind() == NAME) {
                    always.extend(self.assigned(&name, complete, paths));
                }
            }
            _ => {
                // An element or a slice: whether every element is assigned
                // is not known, so the signal is left alone.
                if let Some(token) = target
                    .descendants()
                    .find(|n| n.kind() == NAME)
                    .and_then(|n| mark_token(&n))
                    && let Some(id) = self.written_signal(&token)
                {
                    paths.partial.insert(id);
                }
            }
        }
        always
    }

    // ---- Clock domains ------------------------------------------------------

    /// Signals registered on one clock and used in logic clocked by another.
    fn clock_domains(&mut self) {
        struct Process {
            clock: DeclId,
            range: TextRange,
            node: SyntaxNode,
        }
        let processes: Vec<Process> = std::mem::take(&mut self.clocked)
            .into_iter()
            .map(|(clock, node)| Process {
                clock,
                range: node.text_range(),
                node,
            })
            .collect();
        if processes.len() < 2 || processes.iter().all(|p| p.clock == processes[0].clock) {
            return;
        }
        // The clock each signal is registered on; `None` when several.
        let mut domain: HashMap<DeclId, Option<DeclId>> = HashMap::new();
        for p in &processes {
            for r in self.refs_in(p.range) {
                if !r.access.writes() {
                    continue;
                }
                for id in self.signals(r) {
                    domain
                        .entry(id)
                        .and_modify(|d| {
                            if *d != Some(p.clock) {
                                *d = None;
                            }
                        })
                        .or_insert(Some(p.clock));
                }
            }
        }
        for p in &processes {
            let mut reported: HashSet<DeclId> = HashSet::new();
            let refs: Vec<&crate::decl::Reference> = self.refs_in(p.range).collect();
            for r in refs {
                if !r.access.reads() {
                    continue;
                }
                for id in self.signals(r) {
                    let Some(Some(clock)) = domain.get(&id) else {
                        continue;
                    };
                    if *clock == p.clock || id == p.clock || reported.contains(&id) {
                        continue;
                    }
                    if self.decl(id).is_some_and(|d| self.is_memory(d)) {
                        // A dual-clock memory, read on the other port.
                        continue;
                    }
                    if is_register_copy(&p.node, r.range) {
                        // `sync <= s;`: the first stage of a synchronizer.
                        continue;
                    }
                    reported.insert(id);
                    let name = |id: DeclId| {
                        self.decl(id)
                            .map(|d| d.spelling.clone())
                            .unwrap_or_default()
                    };
                    self.diags.push(Diagnostic {
                        code: codes::CLOCK_DOMAIN_CROSSING,
                        file: self.file,
                        range: r.range,
                        message: format!(
                            "`{}` is registered on `{}` but used here on `{}` without a synchronizer",
                            name(id),
                            name(*clock),
                            name(p.clock)
                        ),
                        fix: None,
                    });
                }
            }
        }
    }

    /// The clock signal of `rising_edge(clk)`, `falling_edge(clk)` or
    /// `clk'event`.
    fn clock_of(&self, edge: &SyntaxNode) -> Option<DeclId> {
        let name = match edge.kind() {
            SyntaxKind::CALL_EXPR => edge
                .children()
                .find(|n| n.kind() == SyntaxKind::ASSOCIATION_LIST)?
                .descendants()
                .find(|n| matches!(n.kind(), SyntaxKind::NAME | SyntaxKind::SELECTED_NAME))?,
            _ => edge.children().next()?,
        };
        let id = self.target(&mark_token(&name)?)?;
        self.decl(id)?.kind.is_signal().then_some(id)
    }

    fn refs_in(&self, range: TextRange) -> impl Iterator<Item = &'a crate::decl::Reference> + '_ {
        let refs = &self.result.refs;
        let by_start = self.by_start.get_or_init(|| {
            let mut order: Vec<usize> = (0..refs.len()).collect();
            order.sort_by_key(|&i| refs[i].range.start());
            order
        });
        let from = by_start.partition_point(|&i| refs[i].range.start() < range.start());
        by_start[from..]
            .iter()
            .map(move |&i| &refs[i])
            .take_while(move |r| r.range.start() <= range.end())
            .filter(move |r| range.contains_range(r.range))
    }

    fn signals(&self, r: &crate::decl::Reference) -> Vec<DeclId> {
        r.targets
            .iter()
            .filter_map(|t| match t {
                Target::Decl(id) => self.decl(*id).filter(|d| d.kind.is_signal()).map(|_| *id),
                Target::Library(_) => None,
            })
            .collect()
    }
}

/// Assignments seen in a process: the first target range of each signal,
/// and signals assigned only in part.
#[derive(Default)]
struct Paths {
    first: HashMap<DeclId, TextRange>,
    partial: HashSet<DeclId>,
}

fn intersection(sets: Vec<HashSet<DeclId>>) -> HashSet<DeclId> {
    let mut sets = sets.into_iter();
    let Some(first) = sets.next() else {
        return HashSet::new();
    };
    sets.fold(first, |acc, s| acc.intersection(&s).copied().collect())
}

/// Whether the name at `range` is the whole value of an assignment to a
/// plain signal: `x <= s;`.
fn is_register_copy(process: &SyntaxNode, range: TextRange) -> bool {
    let Some(element) = process
        .covering_element(range)
        .into_node()
        .or_else(|| process.covering_element(range).parent())
    else {
        return false;
    };
    let name = element
        .ancestors()
        .find(|n| n.kind() == SyntaxKind::NAME)
        .unwrap_or(element);
    let Some(waveform_element) = name.parent() else {
        return false;
    };
    if waveform_element.kind() != SyntaxKind::WAVEFORM_ELEMENT {
        return false;
    }
    let Some(waveform) = waveform_element.parent() else {
        return false;
    };
    let Some(statement) = waveform.parent() else {
        return false;
    };
    statement.kind() == SyntaxKind::SIGNAL_ASSIGN_STMT
        && waveform.children().count() == 1
        && statement
            .children()
            .find(|n| n.kind() != SyntaxKind::LABEL)
            .is_some_and(|t| t.kind() == SyntaxKind::NAME)
}

//! Type checking and overload resolution, run on each unit right after its
//! names are resolved.
//!
//! Expressions are typed in two passes, as the LRM describes overload
//! resolution: [`Checker::infer`] computes bottom-up the set of types an
//! expression may have (memoized per node), and [`Checker::check`] walks
//! top-down with the type the context expects, picks the overloads that fit,
//! narrows the references of overloaded names to them, and reports what
//! cannot fit.
//!
//! The checker is conservative: anything it cannot type (a generic type, an
//! alias of a subprogram, a name from a library outside the project, an
//! attribute it does not model) is "unknown" and compatible with everything,
//! so valid code gets no false errors. Errors are only reported where every
//! candidate is rejected for a definite reason.

mod check;
mod infer;
mod operators;
mod statements;
mod subprograms;
mod types;

use rustc_hash::{FxHashMap, FxHashSet};
use vfl_syntax::green_walk::ChildTokens;
use vfl_syntax::{SyntaxKind, SyntaxNode, SyntaxToken, TextRange};

use crate::analysis::{Ctx, UnitResult};
use crate::decl::{DeclId, ElementRef, Reference, Target, Targets, UnitId};
use crate::diagnostic::{Diagnostic, codes};
use crate::syntax::is_designator;

/// A type: a base type declaration, or the type of a numeric literal.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Ty {
    Decl(DeclId),
    UInt,
    UReal,
}

/// A type with the subtype to name it by in messages (`std_logic_vector`
/// rather than its base type `std_ulogic_vector`).
#[derive(Clone, Copy, Debug)]
struct TyS {
    ty: Ty,
    shown: Option<DeclId>,
}

impl TyS {
    fn of(ty: Ty) -> TyS {
        TyS { ty, shown: None }
    }
}

/// What an expression may be.
#[derive(Clone, Debug)]
enum Val {
    /// Anything: not checkable.
    Unknown,
    /// Not a value: a type mark, a range, a unit name.
    NotValue,
    /// One of these types (several when overloaded).
    Types(Vec<TyS>),
    /// A string literal (its characters), or a bit string literal (`None`).
    Str(Option<String>),
    Aggregate,
    Null,
    /// A character literal, with its quotes.
    Char(String),
}

/// The type of a subprogram parameter, for matching.
#[derive(Clone, Copy, Debug)]
enum Param {
    Ty(Ty),
    AnyInteger,
    AnyReal,
    Any,
}

/// An operator signature: an explicit declaration or a predefined operator.
#[derive(Clone, Debug)]
struct OpSig {
    decl: Option<DeclId>,
    params: Vec<Param>,
    /// `None` when unknown.
    ret: Option<TyS>,
}

/// How an argument names its parameter.
#[derive(Clone, Debug)]
enum Formal {
    Positional,
    Named(String),
    /// `p(3) => x` or `conv(p) => x`: not analyzed.
    Complex,
}

#[derive(Clone, Debug)]
struct Arg {
    formal: Formal,
    actual: Option<SyntaxNode>,
    val: Val,
}

/// Why a subprogram does not accept a call.
#[derive(Clone, Debug)]
enum Reject {
    TooMany(usize),
    /// The argument whose formal names no parameter.
    NoParam(usize),
    Twice(DeclId),
    Missing(DeclId),
    /// Argument `index` does not have parameter `param`'s type.
    Mismatch {
        arg: usize,
        param: DeclId,
    },
}

/// What a call's prefix names.
enum Callee {
    Subprograms {
        candidates: Vec<DeclId>,
        /// The token the prefix resolves at, for narrowing its reference.
        token: Option<SyntaxToken>,
        /// Protected type methods have no reference yet: one is added.
        add_ref: bool,
    },
    Conversion(TyS),
    /// An index or slice of this value.
    Value(Val),
    Attribute,
    Other,
}

/// The type the context expects.
#[derive(Clone, Copy, Debug)]
struct Exp {
    ty: Ty,
    shown: Option<DeclId>,
}

impl From<TyS> for Exp {
    fn from(t: TyS) -> Exp {
        Exp {
            ty: t.ty,
            shown: t.shown,
        }
    }
}

/// Types of `std.standard` the checker needs.
#[derive(Default)]
struct Std {
    boolean: Option<DeclId>,
    bit: Option<DeclId>,
    integer: Option<DeclId>,
    character: Option<DeclId>,
    time: Option<DeclId>,
    string: Option<DeclId>,
    severity_level: Option<DeclId>,
}

/// The changes checking makes to a unit's result.
pub(crate) struct Outcome {
    narrowed: Vec<(usize, Targets)>,
    dropped: Vec<usize>,
    added: Vec<Reference>,
    elements: Vec<ElementRef>,
    diagnostics: Vec<Diagnostic>,
}

impl Outcome {
    pub fn apply(self, result: &mut UnitResult) {
        for (index, targets) in self.narrowed {
            result.refs[index].targets = targets;
        }
        if !self.dropped.is_empty() {
            let mut index = 0;
            let dropped: FxHashSet<usize> = self.dropped.into_iter().collect();
            result.refs.retain(|_| {
                index += 1;
                !dropped.contains(&(index - 1))
            });
        }
        if !self.added.is_empty() {
            result.refs.extend(self.added);
            result.refs.sort_by_key(|r| r.range.start());
        }
        result.elements = self.elements;
        result
            .elements
            .sort_by_key(|e| (e.range.start(), e.range.end()));
        result.elements.dedup();
        result.diagnostics.extend(self.diagnostics);
    }
}

/// Type checks one unit.
pub(crate) fn check_unit(
    ctx: &Ctx,
    unit: UnitId,
    design_unit: &SyntaxNode,
    result: &UnitResult,
) -> Outcome {
    let mut checker = Checker::new(ctx, unit, result);
    if !ctx.files[checker.file as usize].builtin {
        for child in design_unit.children() {
            checker.walk(&child);
        }
        // The hardware checks share the lookup tables.
        let rtl = crate::rtl::check_unit(
            ctx,
            unit,
            design_unit,
            result,
            &checker.refs_at,
            &checker.decls_at,
            &checker.elements,
        );
        checker.diags.extend(rtl);
    }
    Outcome {
        narrowed: checker.narrowed,
        dropped: checker.dropped,
        added: checker.added,
        elements: checker.elements,
        diagnostics: checker.diags,
    }
}

struct Checker<'a> {
    ctx: &'a Ctx<'a>,
    unit: UnitId,
    file: u32,
    result: &'a UnitResult,
    refs_at: FxHashMap<TextRange, usize>,
    /// This unit's declarations by name range.
    decls_at: FxHashMap<TextRange, DeclId>,
    std: Std,
    cache: FxHashMap<(TextRange, SyntaxKind), Val>,
    /// Types the walk learns: loop and generate parameters.
    local_types: FxHashMap<DeclId, TyS>,
    /// The return types of the enclosing subprograms (`None` in procedures).
    returns: Vec<Option<TyS>>,
    narrowed: Vec<(usize, Targets)>,
    dropped: Vec<usize>,
    added: Vec<Reference>,
    elements: Vec<ElementRef>,
    diags: Vec<Diagnostic>,
    /// Inside an `ERROR` node: nothing is reported.
    quiet: u32,
    /// Expression nesting, to give up on pathological input (a chain of
    /// 16k `&`) instead of overflowing the stack.
    depth: u32,
}

/// Expressions nested deeper than this are not typed or checked.
const MAX_DEPTH: u32 = 200;

impl<'a> Checker<'a> {
    fn new(ctx: &'a Ctx<'a>, unit: UnitId, result: &'a UnitResult) -> Self {
        let mut refs_at = FxHashMap::default();
        for (i, r) in result.refs.iter().enumerate() {
            refs_at.entry(r.range).or_insert(i);
        }
        let decls_at = result
            .decls
            .iter()
            .enumerate()
            .map(|(i, d)| {
                (
                    d.range,
                    DeclId {
                        unit,
                        index: i as u32,
                    },
                )
            })
            .collect();
        let mut std = Std::default();
        if let Some(region) = ctx.standard_region() {
            let get = |name: &str| region.get(name).first().copied();
            std = Std {
                boolean: get("boolean"),
                bit: get("bit"),
                integer: get("integer"),
                character: get("character"),
                time: get("time"),
                string: get("string"),
                severity_level: get("severity_level"),
            };
        }
        Checker {
            ctx,
            unit,
            file: ctx.units[unit as usize].file,
            result,
            refs_at,
            decls_at,
            std,
            cache: FxHashMap::default(),
            local_types: FxHashMap::default(),
            returns: Vec::new(),
            narrowed: Vec::new(),
            dropped: Vec::new(),
            added: Vec::new(),
            elements: Vec::new(),
            diags: Vec::new(),
            quiet: 0,
            depth: 0,
        }
    }

    // ---- Messages -----------------------------------------------------------

    fn ty_name(&self, ty: Ty, shown: Option<DeclId>) -> String {
        if let Some(decl) = shown.and_then(|id| self.decl(id)) {
            return decl.spelling.clone();
        }
        match ty {
            Ty::Decl(id) => self
                .decl(id)
                .map_or_else(|| "?".into(), |d| d.spelling.clone()),
            Ty::UInt => "universal_integer".into(),
            Ty::UReal => "universal_real".into(),
        }
    }

    fn describe(&self, val: &Val) -> String {
        match val {
            Val::Types(types) => {
                let mut names: Vec<String> = Vec::new();
                for t in types {
                    let name = format!("`{}`", self.ty_name(t.ty, t.shown));
                    if !names.contains(&name) {
                        names.push(name);
                    }
                }
                names.join(" or ")
            }
            Val::Str(Some(_)) => "a string literal".into(),
            Val::Str(None) => "a bit string literal".into(),
            Val::Aggregate => "an aggregate".into(),
            Val::Char(c) => format!("the character literal {c}"),
            Val::Null => "null".into(),
            Val::Unknown | Val::NotValue => "an unknown value".into(),
        }
    }

    fn error(&mut self, code: &'static str, range: TextRange, message: String) {
        if self.quiet > 0 {
            return;
        }
        self.diags.push(Diagnostic {
            code,
            file: self.file,
            range,
            message,
            fix: None,
        });
    }

    fn mismatch(&mut self, node: &SyntaxNode, val: &Val, exp: Exp) {
        let message = format!(
            "expected `{}`, found {}",
            self.ty_name(exp.ty, exp.shown),
            self.describe(val)
        );
        self.error(codes::TYPE_MISMATCH, trimmed_range(node), message);
    }

    /// Reports `val` if it cannot have the expected type.
    fn expect(&mut self, node: &SyntaxNode, val: &Val, exp: Option<Exp>) {
        if let Some(exp) = exp
            && self.definite(val)
            && !self.compat(val, exp.ty)
        {
            self.mismatch(node, val, exp);
        }
    }

    // ---- References ---------------------------------------------------------

    fn ref_index(&self, token: &SyntaxToken) -> Option<usize> {
        self.refs_at.get(&token.text_range()).copied()
    }

    fn targets(&self, token: &SyntaxToken) -> &'a [Target] {
        match self.ref_index(token) {
            Some(i) => &self.result.refs[i].targets,
            None => &[],
        }
    }

    fn target_decls(&self, token: &SyntaxToken) -> Vec<DeclId> {
        self.targets(token)
            .iter()
            .filter_map(|t| match t {
                Target::Decl(id) => Some(*id),
                Target::Library(_) => None,
            })
            .collect()
    }

    /// Narrows the reference at `token` to `keep`.
    fn narrow(&mut self, token: &SyntaxToken, keep: &[DeclId]) {
        if keep.is_empty() {
            return;
        }
        let Some(index) = self.ref_index(token) else {
            return;
        };
        let targets = &self.result.refs[index].targets;
        let kept: Targets = targets
            .iter()
            .filter(|t| matches!(t, Target::Decl(id) if keep.contains(id)))
            .cloned()
            .collect();
        if !kept.is_empty() && kept.len() < targets.len() {
            self.narrowed.push((index, kept));
        }
    }
}

// ---- Helpers ------------------------------------------------------------------

/// A numeric, character, string, bit string or null literal.
fn literal(node: &SyntaxNode) -> Val {
    let Some(token) = node.child_token(|k| !k.is_trivia()) else {
        return Val::Unknown;
    };
    match token.kind() {
        SyntaxKind::DECIMAL_LITERAL | SyntaxKind::BASED_LITERAL => {
            if token.text().contains('.') {
                Val::Types(vec![TyS::of(Ty::UReal)])
            } else {
                Val::Types(vec![TyS::of(Ty::UInt)])
            }
        }
        SyntaxKind::CHAR_LITERAL => Val::Char(token.text().to_owned()),
        SyntaxKind::STRING_LITERAL => {
            // An unterminated string has no closing quote.
            let inner = token
                .text()
                .strip_prefix('"')
                .and_then(|t| t.strip_suffix('"'));
            Val::Str(inner.map(|t| t.replace("\"\"", "\"")))
        }
        SyntaxKind::BIT_STRING_LITERAL => Val::Str(None),
        SyntaxKind::NULL_KW => Val::Null,
        _ => Val::Unknown,
    }
}

/// The only type of `val`, if it has exactly one.
fn single_type(val: &Val) -> Option<TyS> {
    match val {
        Val::Types(types) if types.len() == 1 => Some(types[0]),
        _ => None,
    }
}

/// The signature all candidates share, if they share one.
fn unique_sig(sigs: &[OpSig]) -> Option<&OpSig> {
    let first = sigs.first()?;
    let key = |s: &OpSig| -> Vec<Option<Ty>> {
        s.params
            .iter()
            .map(|p| match p {
                Param::Ty(t) => Some(*t),
                _ => None,
            })
            .collect()
    };
    let k = key(first);
    sigs.iter().all(|s| key(s) == k).then_some(first)
}

/// The suffix of a selected name: a designator or `all`.
fn selected_suffix(node: &SyntaxNode) -> Option<SyntaxToken> {
    node.last_child_token(|k| is_designator(k) || k == SyntaxKind::ALL_KW)
}

/// The attribute of an attribute name, lower case.
fn attribute_designator(node: &SyntaxNode) -> Option<String> {
    node.children_with_tokens()
        .filter_map(|e| e.into_token())
        .skip_while(|t| t.kind() != SyntaxKind::TICK)
        .nth(1)
        .map(|t| t.text().to_ascii_lowercase())
}

/// The first node after a token of kind `kind`.
fn node_after(node: &SyntaxNode, kind: SyntaxKind) -> Option<SyntaxNode> {
    node.children_with_tokens()
        .skip_while(|e| e.kind() != kind)
        .find_map(|e| e.into_node())
}

fn is_expression(kind: SyntaxKind) -> bool {
    use SyntaxKind::*;
    matches!(
        kind,
        CALL_EXPR
            | BINARY_EXPR
            | UNARY_EXPR
            | AGGREGATE
            | PAREN_EXPR
            | QUALIFIED_EXPR
            | NAME
            | SELECTED_NAME
            | ATTRIBUTE_NAME
            | LITERAL
            | PHYSICAL_LITERAL
            | CONDITIONAL_EXPR
            | RANGE_EXPR
            | ALLOCATOR
    )
}

/// A node's range without leading and trailing trivia.
fn trimmed_range(node: &SyntaxNode) -> TextRange {
    let tokens: Vec<SyntaxToken> = node
        .descendants_with_tokens()
        .filter_map(|e| e.into_token())
        .filter(|t| !t.kind().is_trivia())
        .collect();
    match (tokens.first(), tokens.last()) {
        (Some(a), Some(b)) => TextRange::new(a.text_range().start(), b.text_range().end()),
        _ => node.text_range(),
    }
}

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

use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hasher};

use rowan::NodeOrToken;
use vfl_syntax::{SyntaxKind, SyntaxNode, SyntaxToken, TextRange, identifier_key};

use crate::analysis::{Ctx, UnitResult};
use crate::decl::{
    Access, Decl, DeclId, DeclKind, ElementRef, Mode, Reference, Target, TypeDef, UnitId,
};
use crate::diagnostic::{Diagnostic, codes};
use crate::resolver::{mark_token, operator_token};
use crate::syntax::{designator, has_token, is_designator};

/// A fast hasher for small keys (ranges, ids); the default SipHash is
/// noticeably slower here, and nothing hashes untrusted keys.
#[derive(Default)]
pub(crate) struct FxHasher(u64);

impl Hasher for FxHasher {
    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.write_u64(u64::from(b));
        }
    }
    fn write_u8(&mut self, n: u8) {
        self.write_u64(u64::from(n));
    }
    fn write_u16(&mut self, n: u16) {
        self.write_u64(u64::from(n));
    }
    fn write_u32(&mut self, n: u32) {
        self.write_u64(u64::from(n));
    }
    fn write_u64(&mut self, n: u64) {
        self.0 = (self.0.rotate_left(5) ^ n).wrapping_mul(0x51_7c_c1_b7_27_22_0a_95);
    }
    fn write_usize(&mut self, n: usize) {
        self.write_u64(n as u64);
    }
    fn finish(&self) -> u64 {
        self.0
    }
}

pub(crate) type FxMap<K, V> = HashMap<K, V, BuildHasherDefault<FxHasher>>;

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
    narrowed: Vec<(usize, Vec<Target>)>,
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
            let dropped: std::collections::HashSet<usize> = self.dropped.into_iter().collect();
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
    refs_at: FxMap<TextRange, usize>,
    /// This unit's declarations by name range.
    decls_at: FxMap<TextRange, DeclId>,
    std: Std,
    cache: FxMap<(TextRange, SyntaxKind), Val>,
    /// Types the walk learns: loop and generate parameters.
    local_types: FxMap<DeclId, TyS>,
    /// The return types of the enclosing subprograms (`None` in procedures).
    returns: Vec<Option<TyS>>,
    narrowed: Vec<(usize, Vec<Target>)>,
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
        let mut refs_at = FxMap::default();
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
            cache: FxMap::default(),
            local_types: FxMap::default(),
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

    // ---- Declarations and types ---------------------------------------------

    fn decl(&self, id: DeclId) -> Option<&'a Decl> {
        if id.unit == self.unit {
            self.result.decls.get(id.index as usize)
        } else {
            self.ctx.decl(id)
        }
    }

    /// The base type of a type or subtype declaration.
    fn base(&self, mut id: DeclId) -> Option<DeclId> {
        for _ in 0..32 {
            let decl = self.decl(id)?;
            match decl.kind {
                DeclKind::Type => return Some(id),
                DeclKind::Subtype | DeclKind::IncompleteType => id = decl.subtype?,
                _ => return None,
            }
        }
        None
    }

    /// A type mark's type, shown by the mark's name.
    fn mark_type(&self, mark: DeclId) -> Option<TyS> {
        Some(TyS {
            ty: Ty::Decl(self.base(mark)?),
            shown: Some(mark),
        })
    }

    /// The type of an object, literal, unit or function result.
    fn decl_type(&self, id: DeclId) -> Option<TyS> {
        if let Some(t) = self.local_types.get(&id) {
            return Some(*t);
        }
        self.mark_type(self.decl(id)?.subtype?)
    }

    fn def(&self, ty: Ty) -> Option<&'a TypeDef> {
        match ty {
            Ty::Decl(id) => self.decl(id)?.type_def.as_deref(),
            _ => None,
        }
    }

    fn std_ty(&self, id: Option<DeclId>) -> Option<TyS> {
        id.map(|id| TyS {
            ty: Ty::Decl(id),
            shown: Some(id),
        })
    }

    fn is_std(&self, ty: Ty, id: Option<DeclId>) -> bool {
        match (ty, id) {
            (Ty::Decl(a), Some(b)) => self.same_decl(a, b),
            _ => false,
        }
    }

    /// The same type, also when a unit exists twice (a file in two
    /// libraries, alternate copies of a package).
    fn same_decl(&self, a: DeclId, b: DeclId) -> bool {
        if a == b {
            return true;
        }
        let (Some(x), Some(y)) = (self.decl(a), self.decl(b)) else {
            return false;
        };
        x.name == y.name
            && self.ctx.units[a.unit as usize].name == self.ctx.units[b.unit as usize].name
            && x.kind == y.kind
    }

    /// `ieee.std_logic_1164.std_ulogic`.
    fn is_std_ulogic(&self, ty: Ty) -> bool {
        match ty {
            Ty::Decl(id) => {
                self.decl(id).is_some_and(|d| d.name == "std_ulogic")
                    && self.ctx.units[id.unit as usize].name == "std_logic_1164"
            }
            _ => false,
        }
    }

    fn is_integer(&self, ty: Ty) -> Option<bool> {
        match ty {
            Ty::UInt => Some(true),
            Ty::UReal => Some(false),
            Ty::Decl(_) => match self.def(ty)? {
                TypeDef::Integer => Some(true),
                TypeDef::Scalar => None,
                _ => Some(false),
            },
        }
    }

    fn is_real(&self, ty: Ty) -> Option<bool> {
        match ty {
            Ty::UReal => Some(true),
            Ty::UInt => Some(false),
            Ty::Decl(_) => match self.def(ty)? {
                TypeDef::Real => Some(true),
                TypeDef::Scalar => None,
                _ => Some(false),
            },
        }
    }

    /// Whether a value of type `actual` can be used where `formal` is
    /// expected (universal types convert implicitly).
    fn ty_compat(&self, actual: Ty, formal: Ty) -> bool {
        match (actual, formal) {
            (Ty::Decl(a), Ty::Decl(b)) => {
                self.same_decl(a, b) || self.def(actual).is_none() || self.def(formal).is_none()
            }
            (Ty::UInt, Ty::UInt) | (Ty::UReal, Ty::UReal) => true,
            (Ty::UInt, Ty::Decl(_)) => self.is_integer(formal) != Some(false),
            (Ty::UReal, Ty::Decl(_)) => self.is_real(formal) != Some(false),
            _ => false,
        }
    }

    /// Whether `val` may have type `ty`.
    fn compat(&self, val: &Val, ty: Ty) -> bool {
        match val {
            Val::Unknown | Val::NotValue | Val::Null => true,
            Val::Types(types) => types.iter().any(|t| self.ty_compat(t.ty, ty)),
            Val::Str(text) => match self.def(ty) {
                None => !matches!(ty, Ty::UInt | Ty::UReal),
                Some(TypeDef::Array { indexes, element }) => {
                    if indexes.len() != 1 {
                        return false;
                    }
                    let Some(element) = element.and_then(|e| self.base(e)) else {
                        return true;
                    };
                    match self.def(Ty::Decl(element)) {
                        None => true,
                        // `character` has every character.
                        Some(TypeDef::Enum(_))
                            if self.is_std(Ty::Decl(element), self.std.character) =>
                        {
                            true
                        }
                        Some(TypeDef::Enum(literals)) => match text {
                            Some(text) => text.chars().all(|c| {
                                let mut key = [0u8; 3];
                                key[0] = b'\'';
                                key[2] = b'\'';
                                !c.is_ascii() || {
                                    key[1] = c as u8;
                                    let key = std::str::from_utf8(&key).unwrap_or_default();
                                    literals.iter().any(|l| l == key)
                                }
                            }),
                            None => true,
                        },
                        Some(_) => false,
                    }
                }
                Some(_) => false,
            },
            Val::Aggregate => match self.def(ty) {
                None => !matches!(ty, Ty::UInt | Ty::UReal),
                Some(def) => matches!(def, TypeDef::Array { .. } | TypeDef::Record(_)),
            },
            Val::Char(c) => match self.def(ty) {
                None => !matches!(ty, Ty::UInt | Ty::UReal),
                Some(TypeDef::Enum(literals)) => literals.contains(c),
                Some(_) => false,
            },
        }
    }

    fn param_compat(&self, val: &Val, param: Param) -> bool {
        match param {
            Param::Ty(ty) => self.compat(val, ty),
            Param::Any => true,
            Param::AnyInteger | Param::AnyReal => match val {
                Val::Types(types) => types.iter().any(|t| {
                    let ok = if matches!(param, Param::AnyInteger) {
                        self.is_integer(t.ty)
                    } else {
                        self.is_real(t.ty)
                    };
                    ok != Some(false)
                }),
                Val::Unknown | Val::NotValue => true,
                _ => false,
            },
        }
    }

    /// Whether every type of `val` is known, so a mismatch is certain.
    fn definite(&self, val: &Val) -> bool {
        match val {
            Val::Unknown | Val::NotValue | Val::Null => false,
            Val::Types(types) => types.iter().all(|t| match t.ty {
                Ty::Decl(_) => self.def(t.ty).is_some(),
                _ => true,
            }),
            _ => true,
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
        let kept: Vec<Target> = targets
            .iter()
            .filter(|t| matches!(t, Target::Decl(id) if keep.contains(id)))
            .cloned()
            .collect();
        if !kept.is_empty() && kept.len() < targets.len() {
            self.narrowed.push((index, kept));
        }
    }

    // ---- Subprograms --------------------------------------------------------

    fn is_subprogram(kind: DeclKind) -> bool {
        matches!(
            kind,
            DeclKind::Function | DeclKind::Procedure | DeclKind::GenericSubprogram
        )
    }

    /// Operations `std.standard` declares once for every type (`to_string`,
    /// `minimum`, file operations): their parameters are not checked.
    fn is_implicit(&self, id: DeclId) -> bool {
        Some(id.unit) == self.ctx.standard
            && self
                .decl(id)
                .is_some_and(|d| Self::is_subprogram(d.kind) && d.name != "now")
    }

    /// Candidates whose parameters are unknown: they accept any call.
    fn is_wildcard(&self, id: DeclId) -> bool {
        self.is_implicit(id) || self.decl(id).is_none_or(|d| d.params.is_none())
    }

    fn is_function(&self, id: DeclId) -> bool {
        self.decl(id).is_some_and(|d| match d.kind {
            DeclKind::Function => true,
            DeclKind::GenericSubprogram => d.subtype.is_some() || d.params.is_none(),
            _ => false,
        })
    }

    /// A function's result type; `None` when unknown.
    fn return_type(&self, id: DeclId) -> Option<TyS> {
        let decl = self.decl(id)?;
        if self.is_implicit(id) && matches!(decl.name.as_str(), "minimum" | "maximum") {
            return None;
        }
        self.decl_type(id)
    }

    fn param_type(&self, param: DeclId) -> Param {
        match self.decl_type(param) {
            Some(t) => Param::Ty(t.ty),
            None => Param::Any,
        }
    }

    /// Matches a call's arguments to a subprogram's parameters.
    fn match_call(&self, id: DeclId, args: &[Arg]) -> Result<Vec<(usize, DeclId)>, Reject> {
        if self.is_wildcard(id) {
            return Ok(Vec::new());
        }
        let params: &[DeclId] = self
            .decl(id)
            .and_then(|d| d.params.as_deref())
            .unwrap_or(&[]);
        // Cheap rejections first: most candidates of a heavily overloaded
        // name fail on the argument count.
        let positional = args
            .iter()
            .take_while(|a| matches!(a.formal, Formal::Positional))
            .count();
        let simple = args
            .iter()
            .filter(|a| !matches!(a.formal, Formal::Complex))
            .count();
        // Partial associations (`r.a => x`, `v(0) => y`) may name a formal
        // several times, so only the simple ones count.
        if positional > params.len() || simple > params.len() {
            return Err(Reject::TooMany(params.len()));
        }
        let mut assigned: Vec<Option<usize>> = vec![None; params.len()];
        let mut complex = false;
        let mut position = 0;
        for (i, arg) in args.iter().enumerate() {
            match &arg.formal {
                Formal::Positional => {
                    if position >= params.len() {
                        return Err(Reject::TooMany(params.len()));
                    }
                    assigned[position] = Some(i);
                    position += 1;
                }
                Formal::Named(name) => {
                    let Some(p) = params
                        .iter()
                        .position(|&p| self.decl(p).is_some_and(|d| d.name == *name))
                    else {
                        return Err(Reject::NoParam(i));
                    };
                    if assigned[p].is_some() {
                        return Err(Reject::Twice(params[p]));
                    }
                    assigned[p] = Some(i);
                }
                Formal::Complex => complex = true,
            }
        }
        if !complex {
            for (p, a) in params.iter().zip(&assigned) {
                if a.is_none()
                    && let Some(d) = self.decl(*p)
                    && matches!(
                        d.kind,
                        DeclKind::Parameter {
                            has_default: false,
                            ..
                        }
                    )
                {
                    return Err(Reject::Missing(*p));
                }
            }
        }
        let mut pairs = Vec::new();
        for (p, a) in params.iter().zip(&assigned) {
            let Some(a) = *a else { continue };
            if !self.param_compat(&args[a].val, self.param_type(*p)) {
                return Err(Reject::Mismatch { arg: a, param: *p });
            }
            pairs.push((a, *p));
        }
        Ok(pairs)
    }

    /// Whether `id` may be a function without arguments whose result is
    /// indexed or sliced: `f(3)` calling `f` and indexing its array result.
    fn indexable_result(&self, id: DeclId) -> bool {
        let Some(decl) = self.decl(id) else {
            return false;
        };
        let no_args = decl.params.as_deref().is_some_and(|ps| {
            ps.iter().all(|&p| {
                self.decl(p).is_some_and(|d| {
                    matches!(
                        d.kind,
                        DeclKind::Parameter {
                            has_default: true,
                            ..
                        }
                    )
                })
            })
        });
        no_args
            && self.return_type(id).is_none_or(|t| {
                matches!(
                    self.def(t.ty),
                    None | Some(TypeDef::Array { .. } | TypeDef::Access(_))
                )
            })
    }

    /// The arguments of an association list, with their inferred values.
    fn args(&mut self, list: Option<&SyntaxNode>) -> Vec<Arg> {
        let Some(list) = list else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for element in list
            .children()
            .filter(|n| n.kind() == SyntaxKind::ASSOCIATION_ELEMENT)
        {
            let has_arrow = has_token(&element, SyntaxKind::ARROW);
            let mut nodes = Vec::new();
            let mut formal_node = None;
            let mut past_arrow = !has_arrow;
            for child in element.children_with_tokens() {
                match child {
                    NodeOrToken::Token(t) if t.kind() == SyntaxKind::ARROW => past_arrow = true,
                    NodeOrToken::Node(n) if past_arrow => nodes.push(n),
                    NodeOrToken::Node(n) => formal_node = Some(n),
                    _ => {}
                }
            }
            let formal = match &formal_node {
                None => Formal::Positional,
                Some(n) if n.kind() == SyntaxKind::NAME => match mark_token(n) {
                    Some(t) if t.kind() != SyntaxKind::CHAR_LITERAL => {
                        Formal::Named(crate::syntax::designator_key(&t))
                    }
                    _ => Formal::Complex,
                },
                Some(_) => Formal::Complex,
            };
            let actual = nodes.into_iter().next();
            let val = match &actual {
                Some(n) => self.infer(n),
                None => Val::Unknown,
            };
            out.push(Arg {
                formal,
                actual,
                val,
            });
        }
        out
    }

    // ---- Inference ----------------------------------------------------------

    /// The values an expression may have, bottom-up.
    fn infer(&mut self, node: &SyntaxNode) -> Val {
        let key = (node.text_range(), node.kind());
        if let Some(v) = self.cache.get(&key) {
            return v.clone();
        }
        if self.depth >= MAX_DEPTH {
            return Val::Unknown;
        }
        self.depth += 1;
        let val = self.infer_uncached(node);
        self.depth -= 1;
        self.cache.insert(key, val.clone());
        val
    }

    fn infer_uncached(&mut self, node: &SyntaxNode) -> Val {
        use SyntaxKind::*;
        match node.kind() {
            LITERAL => literal(node),
            PHYSICAL_LITERAL => node
                .children()
                .find(|n| n.kind() == NAME)
                .and_then(|n| mark_token(&n))
                .map(|t| self.name_val(&t))
                .unwrap_or(Val::Unknown),
            PAREN_EXPR => match node.children().next() {
                Some(inner) => self.infer(&inner),
                None => Val::Unknown,
            },
            AGGREGATE => Val::Aggregate,
            QUALIFIED_EXPR => node
                .children()
                .next()
                .and_then(|mark| self.type_mark_of(&mark))
                .map_or(Val::Unknown, |t| Val::Types(vec![t])),
            NAME => match mark_token(node) {
                Some(t) if t.kind() == CHAR_LITERAL => Val::Char(t.text().to_owned()),
                Some(t) => self.name_val(&t),
                None => Val::Unknown,
            },
            SELECTED_NAME => self.selected_val(node),
            ATTRIBUTE_NAME => self.attribute_val(node, None),
            CALL_EXPR => self.call_val(node),
            BINARY_EXPR | UNARY_EXPR => {
                let (sigs, vals, _) = self.operator(node, None);
                // The predefined operators of an operand's unknown type are
                // missing from the candidates: the result is unknown too.
                if vals
                    .iter()
                    .any(|v| matches!(v, Val::Unknown | Val::NotValue))
                {
                    return Val::Unknown;
                }
                self.rets(sigs.iter().map(|s| s.ret))
            }
            CONDITIONAL_EXPR => match node.children().next() {
                Some(first) => self.infer(&first),
                None => Val::Unknown,
            },
            EXTERNAL_NAME => node
                .children()
                .find(|n| n.kind() == SUBTYPE_INDICATION)
                .and_then(|n| n.children().find(|c| c.kind() != RESOLUTION_INDICATION))
                .and_then(|mark| self.type_mark_of(&mark))
                .map_or(Val::Unknown, |t| Val::Types(vec![t])),
            RANGE_EXPR | SUBTYPE_INDICATION => Val::NotValue,
            _ => Val::Unknown,
        }
    }

    /// The union of result types; unknown if any is.
    fn rets(&self, rets: impl Iterator<Item = Option<TyS>>) -> Val {
        let mut types: Vec<TyS> = Vec::new();
        let mut any = false;
        for ret in rets {
            any = true;
            match ret {
                Some(t) => {
                    if !types.iter().any(|x| x.ty == t.ty) {
                        types.push(t);
                    }
                }
                None => return Val::Unknown,
            }
        }
        if !any || types.is_empty() {
            Val::Unknown
        } else {
            Val::Types(types)
        }
    }

    /// The type a type mark names.
    fn type_mark_of(&self, mark: &SyntaxNode) -> Option<TyS> {
        let token = mark_token(mark)?;
        let decls = self.target_decls(&token);
        let mut found: Option<DeclId> = None;
        for id in decls {
            let decl = self.decl(id)?;
            if matches!(
                decl.kind,
                DeclKind::Type | DeclKind::Subtype | DeclKind::IncompleteType
            ) {
                match found {
                    Some(f) if !self.same_decl(f, id) => return None,
                    _ => found = Some(id),
                }
            }
        }
        self.mark_type(found?)
    }

    /// What a resolved name denotes as a value.
    fn name_val(&self, token: &SyntaxToken) -> Val {
        let decls = self.target_decls(token);
        if decls.is_empty() {
            return Val::Unknown;
        }
        let mut types: Vec<TyS> = Vec::new();
        let mut value = false;
        for id in decls {
            let Some(decl) = self.decl(id) else {
                return Val::Unknown;
            };
            let ty = match decl.kind {
                DeclKind::EnumLiteral | DeclKind::PhysicalUnit => self.decl_type(id),
                DeclKind::Alias => match decl.subtype {
                    Some(_) => self.decl_type(id),
                    None => return Val::Unknown,
                },
                DeclKind::Function | DeclKind::GenericSubprogram => {
                    if self.is_wildcard(id) {
                        return Val::Unknown;
                    }
                    if !self.callable_without_args(id) {
                        continue;
                    }
                    self.return_type(id)
                }
                kind if kind.is_object() => self.decl_type(id),
                DeclKind::Procedure => continue,
                _ => {
                    // Types, units, labels, ...: not values.
                    continue;
                }
            };
            value = true;
            match ty {
                Some(t) => {
                    if !types.iter().any(|x| x.ty == t.ty) {
                        types.push(t);
                    }
                }
                None => return Val::Unknown,
            }
        }
        if !value {
            return Val::NotValue;
        }
        Val::Types(types)
    }

    fn callable_without_args(&self, id: DeclId) -> bool {
        self.decl(id)
            .and_then(|d| d.params.as_deref())
            .is_some_and(|ps| {
                ps.iter().all(|&p| {
                    self.decl(p).is_some_and(|d| {
                        matches!(
                            d.kind,
                            DeclKind::Parameter {
                                has_default: true,
                                ..
                            }
                        )
                    })
                })
            })
    }

    /// `pkg.x`, `rec.field`, `ptr.all`, `ptr.field`, `obj.method`.
    fn selected_val(&mut self, node: &SyntaxNode) -> Val {
        let Some(suffix) = selected_suffix(node) else {
            return Val::Unknown;
        };
        if self.ref_index(&suffix).is_some() {
            return self.name_val(&suffix);
        }
        let Some(prefix) = node.children().next() else {
            return Val::Unknown;
        };
        let prefix_val = self.infer(&prefix);
        let Some(ty) = single_type(&prefix_val) else {
            return Val::Unknown;
        };
        if let Some(candidates) = self.methods(ty, &suffix) {
            let rets: Vec<Option<TyS>> = candidates
                .iter()
                .filter(|&&c| self.is_function(c) && self.callable_without_args(c))
                .map(|&c| self.return_type(c))
                .collect();
            return self.rets(rets.into_iter());
        }
        if suffix.kind() == SyntaxKind::ALL_KW {
            return match self.def(ty.ty) {
                Some(TypeDef::Access(Some(d))) => self
                    .mark_type(*d)
                    .map_or(Val::Unknown, |t| Val::Types(vec![t])),
                _ => Val::Unknown,
            };
        }
        let key = crate::syntax::designator_key(&suffix);
        let element = self.element_type(ty.ty, &key);
        if element.is_some() {
            self.element_ref(&suffix, ty.ty, key);
        }
        match element {
            Some(Some(t)) => Val::Types(vec![t]),
            _ => Val::Unknown,
        }
    }

    /// The subprograms named `suffix` of a protected type.
    fn methods(&self, ty: TyS, suffix: &SyntaxToken) -> Option<Vec<DeclId>> {
        let Some(TypeDef::Protected) = self.def(ty.ty) else {
            return None;
        };
        let Ty::Decl(id) = ty.ty else { return None };
        let members = self.decl(id)?.members.clone()?;
        let key = crate::syntax::designator_key(suffix);
        Some(
            members
                .get(&key)
                .iter()
                .copied()
                .filter(|&c| self.decl(c).is_some_and(|d| Self::is_subprogram(d.kind)))
                .collect(),
        )
    }

    /// Records that `token` names element `name` of record type `ty`, or
    /// of the record it designates.
    fn element_ref(&mut self, token: &SyntaxToken, ty: Ty, name: String) {
        let record = match (ty, self.def(ty)) {
            (Ty::Decl(id), Some(TypeDef::Record(_))) => id,
            (_, Some(TypeDef::Access(Some(d)))) => match self.mark_type(*d) {
                Some(TyS {
                    ty: Ty::Decl(id), ..
                }) => id,
                _ => return,
            },
            _ => return,
        };
        self.elements.push(ElementRef {
            range: token.text_range(),
            record,
            name,
        });
    }

    /// The type of element `name` of a record (or of the record an access
    /// type designates): `Some(None)` when the element exists but its type is
    /// unknown, `None` when there is no such element or no record.
    fn element_type(&self, ty: Ty, name: &str) -> Option<Option<TyS>> {
        match self.def(ty)? {
            TypeDef::Record(elements) => {
                let e = elements.iter().find(|e| e.name == name)?;
                Some(e.subtype.and_then(|s| self.mark_type(s)))
            }
            TypeDef::Access(Some(d)) => {
                let t = self.mark_type(*d)?;
                match self.def(t.ty)? {
                    TypeDef::Record(_) => self.element_type(t.ty, name),
                    _ => None,
                }
            }
            _ => None,
        }
    }

    /// An attribute's value; `args` are the arguments of `x'attr(args)`.
    fn attribute_val(&mut self, node: &SyntaxNode, _args: Option<&SyntaxNode>) -> Val {
        let Some(attribute) = attribute_designator(node) else {
            return Val::Unknown;
        };
        let prefix = node.children().next();
        let prefix_ty = || -> Option<TyS> { None };
        let _ = prefix_ty;
        let boolean = || {
            self.std_ty(self.std.boolean)
                .map_or(Val::Unknown, |t| Val::Types(vec![t]))
        };
        match attribute.as_str() {
            "event" | "active" | "stable" | "quiet" | "ascending" => boolean(),
            "last_event" | "last_active" => self
                .std_ty(self.std.time)
                .map_or(Val::Unknown, |t| Val::Types(vec![t])),
            "image" | "instance_name" | "path_name" | "simple_name" => self
                .std_ty(self.std.string)
                .map_or(Val::Unknown, |t| Val::Types(vec![t])),
            "length" | "pos" => Val::Types(vec![TyS::of(Ty::UInt)]),
            "range" | "reverse_range" | "subtype" | "base" | "element" => Val::NotValue,
            "last_value" | "delayed" | "driving_value" => match prefix {
                Some(p) => match self.infer(&p) {
                    v @ Val::Types(_) => v,
                    _ => Val::Unknown,
                },
                None => Val::Unknown,
            },
            "val" | "succ" | "pred" | "leftof" | "rightof" | "value" => {
                match prefix.as_ref().and_then(|p| self.type_mark_of(p)) {
                    Some(t) => Val::Types(vec![t]),
                    None => Val::Unknown,
                }
            }
            "high" | "low" | "left" | "right" => {
                let Some(p) = prefix else {
                    return Val::Unknown;
                };
                let t = match self.type_mark_of(&p) {
                    Some(t) => Some(t),
                    None => single_type(&self.infer(&p)),
                };
                match t {
                    Some(t) => match self.def(t.ty) {
                        Some(TypeDef::Array { indexes, .. }) if indexes.len() == 1 => indexes[0]
                            .and_then(|i| self.mark_type(i))
                            .map_or(Val::Unknown, |t| Val::Types(vec![t])),
                        Some(
                            TypeDef::Enum(_) | TypeDef::Integer | TypeDef::Real | TypeDef::Physical,
                        ) => Val::Types(vec![t]),
                        _ => Val::Unknown,
                    },
                    None => Val::Unknown,
                }
            }
            _ => Val::Unknown,
        }
    }

    /// What a call's prefix names. `procedure` selects procedures rather
    /// than functions.
    fn callee(&mut self, prefix: &SyntaxNode, procedure: bool) -> Callee {
        match prefix.kind() {
            SyntaxKind::NAME | SyntaxKind::SELECTED_NAME => {}
            SyntaxKind::ATTRIBUTE_NAME => return Callee::Attribute,
            _ => return Callee::Value(self.infer(prefix)),
        }
        let Some(token) = mark_token(prefix) else {
            return Callee::Value(Val::Unknown);
        };
        if prefix.kind() == SyntaxKind::SELECTED_NAME && self.ref_index(&token).is_none() {
            // A protected type method, or an element of a record.
            if let Some(p) = prefix.children().next()
                && let Some(ty) = single_type(&self.infer(&p))
                && let Some(candidates) = self.methods(ty, &token)
            {
                let candidates: Vec<DeclId> = candidates
                    .into_iter()
                    .filter(|&c| procedure != self.is_function(c))
                    .collect();
                if candidates.is_empty() {
                    return Callee::Value(Val::Unknown);
                }
                return Callee::Subprograms {
                    candidates,
                    token: Some(token),
                    add_ref: true,
                };
            }
            return Callee::Value(self.infer(prefix));
        }
        let decls = self.target_decls(&token);
        if decls.is_empty() {
            return Callee::Value(Val::Unknown);
        }
        let mut candidates = Vec::new();
        let mut mark = None;
        for &id in &decls {
            let Some(decl) = self.decl(id) else {
                return Callee::Value(Val::Unknown);
            };
            match decl.kind {
                kind if Self::is_subprogram(kind) => {
                    if procedure != self.is_function(id) {
                        candidates.push(id);
                    }
                }
                DeclKind::Alias if decl.subtype.is_none() => return Callee::Value(Val::Unknown),
                DeclKind::Type | DeclKind::Subtype | DeclKind::IncompleteType => mark = Some(id),
                DeclKind::GenericType => return Callee::Value(Val::Unknown),
                kind if kind.is_object() || kind == DeclKind::Alias => {
                    return Callee::Value(self.name_val(&token));
                }
                _ => {}
            }
        }
        if !candidates.is_empty() {
            return Callee::Subprograms {
                candidates,
                token: Some(token),
                add_ref: false,
            };
        }
        if let Some(mark) = mark {
            return match self.mark_type(mark) {
                Some(t) => Callee::Conversion(t),
                None => Callee::Value(Val::Unknown),
            };
        }
        if decls
            .iter()
            .any(|&id| self.decl(id).is_some_and(|d| Self::is_subprogram(d.kind)))
        {
            // Only procedures where a function is needed, or the reverse.
            return Callee::Value(Val::Unknown);
        }
        Callee::Other
    }

    fn call_val(&mut self, node: &SyntaxNode) -> Val {
        let Some(prefix) = node.children().next() else {
            return Val::Unknown;
        };
        let list = node
            .children()
            .find(|n| n.kind() == SyntaxKind::ASSOCIATION_LIST);
        match self.callee(&prefix, false) {
            Callee::Subprograms { candidates, .. } => {
                let args = self.args(list.as_ref());
                let mut rets = Vec::new();
                for &c in &candidates {
                    if self.match_call(c, &args).is_ok() {
                        rets.push(self.return_type(c));
                    } else if self.indexable_result(c) {
                        rets.push(None);
                    }
                }
                self.rets(rets.into_iter())
            }
            Callee::Conversion(t) => Val::Types(vec![t]),
            Callee::Value(val) => self.index_val(&val, list.as_ref()),
            Callee::Attribute => self.attribute_val(&prefix, list.as_ref()),
            Callee::Other => Val::Unknown,
        }
    }

    /// The array type indexed or sliced by `list`, if known: `(array,
    /// is_slice)`.
    fn indexed_array(&mut self, val: &Val, list: Option<&SyntaxNode>) -> Option<(TyS, bool)> {
        let mut ty = single_type(val)?;
        if let Some(TypeDef::Access(Some(d))) = self.def(ty.ty) {
            ty = self.mark_type(*d)?;
        }
        let Some(TypeDef::Array { .. }) = self.def(ty.ty) else {
            return None;
        };
        let elements: Vec<SyntaxNode> = list?
            .children()
            .filter(|n| n.kind() == SyntaxKind::ASSOCIATION_ELEMENT)
            .collect();
        let slice = match elements.as_slice() {
            [one] => one.children().next().is_some_and(|n| self.is_range(&n)),
            _ => false,
        };
        Some((ty, slice))
    }

    fn index_val(&mut self, val: &Val, list: Option<&SyntaxNode>) -> Val {
        let Some((ty, slice)) = self.indexed_array(val, list) else {
            return Val::Unknown;
        };
        if slice {
            return Val::Types(vec![ty]);
        }
        let count = list.map_or(0, |l| {
            l.children()
                .filter(|n| n.kind() == SyntaxKind::ASSOCIATION_ELEMENT)
                .count()
        });
        match self.def(ty.ty) {
            Some(TypeDef::Array {
                indexes,
                element: Some(element),
            }) if indexes.len() == count => self
                .mark_type(*element)
                .map_or(Val::Unknown, |t| Val::Types(vec![t])),
            _ => Val::Unknown,
        }
    }

    /// Whether a node is a discrete range: `a to b`, `t range ...`,
    /// `x'range`, or a type mark.
    fn is_range(&self, node: &SyntaxNode) -> bool {
        match node.kind() {
            SyntaxKind::RANGE_EXPR | SyntaxKind::SUBTYPE_INDICATION => true,
            SyntaxKind::ATTRIBUTE_NAME => matches!(
                attribute_designator(node).as_deref(),
                Some("range" | "reverse_range")
            ),
            SyntaxKind::NAME | SyntaxKind::SELECTED_NAME => mark_token(node).is_some_and(|t| {
                let decls = self.target_decls(&t);
                !decls.is_empty()
                    && decls.iter().all(|&id| {
                        self.decl(id).is_some_and(|d| {
                            matches!(
                                d.kind,
                                DeclKind::Type
                                    | DeclKind::Subtype
                                    | DeclKind::IncompleteType
                                    | DeclKind::GenericType
                            )
                        })
                    })
            }),
            _ => false,
        }
    }

    // ---- Operators ----------------------------------------------------------

    /// The operator signatures that accept the operands of `node`, the
    /// operands' values, and whether candidates are unknown (an operator
    /// from a library outside the project).
    fn operator(&mut self, node: &SyntaxNode, exp: Option<Exp>) -> (Vec<OpSig>, Vec<Val>, bool) {
        let operands: Vec<SyntaxNode> = node.children().collect();
        let vals: Vec<Val> = operands.iter().map(|n| self.infer(n)).collect();
        let Some(op) = operator_token(node) else {
            return (Vec::new(), vals, true);
        };
        let symbol = op.text().to_ascii_lowercase();
        let opaque = self
            .result
            .opaque_ops
            .binary_search_by_key(&op.text_range().start(), |r| r.start())
            .is_ok();
        let mut sigs = Vec::new();
        for id in self.target_decls(&op) {
            let Some(decl) = self.decl(id) else { continue };
            if decl.kind == DeclKind::Alias || self.is_wildcard(id) {
                sigs.push(OpSig {
                    decl: Some(id),
                    params: vec![Param::Any; vals.len()],
                    ret: None,
                });
                continue;
            }
            if !Self::is_subprogram(decl.kind) {
                continue;
            }
            let params: &[DeclId] = decl.params.as_deref().unwrap_or(&[]);
            if params.len() != vals.len() {
                continue;
            }
            sigs.push(OpSig {
                decl: Some(id),
                params: params.iter().map(|&p| self.param_type(p)).collect(),
                ret: self.return_type(id),
            });
        }
        let mut types: Vec<TyS> = Vec::new();
        for v in &vals {
            if let Val::Types(ts) = v {
                for t in ts {
                    if !types.iter().any(|x| x.ty == t.ty) {
                        types.push(*t);
                    }
                }
            }
        }
        if let Some(exp) = exp
            && !types.iter().any(|x| x.ty == exp.ty)
        {
            types.push(TyS {
                ty: exp.ty,
                shown: exp.shown,
            });
        }
        for t in types {
            self.predefined(&symbol, vals.len(), t, &mut sigs);
        }
        let viable: Vec<OpSig> = sigs
            .into_iter()
            .filter(|s| {
                s.params
                    .iter()
                    .zip(&vals)
                    .all(|(p, v)| self.param_compat(v, *p))
            })
            .collect();
        (viable, vals, opaque)
    }

    /// The predefined operators of type `t` named `op`.
    fn predefined(&self, op: &str, arity: usize, t: TyS, out: &mut Vec<OpSig>) {
        let ty = t.ty;
        let def = match ty {
            Ty::UInt => Some(&TypeDef::Integer),
            Ty::UReal => Some(&TypeDef::Real),
            Ty::Decl(_) => self.def(ty),
        };
        let Some(def) = def else { return };
        let boolean = self.std_ty(self.std.boolean);
        let same = Param::Ty(ty);
        let mut add = |params: Vec<Param>, ret: Option<TyS>| {
            out.push(OpSig {
                decl: None,
                params,
                ret,
            })
        };
        let integer = matches!(def, TypeDef::Integer | TypeDef::Scalar);
        let real = matches!(def, TypeDef::Real | TypeDef::Scalar);
        let numeric = integer || real || matches!(def, TypeDef::Physical);
        let scalar = numeric || matches!(def, TypeDef::Enum(_));
        let bitlike = self.is_std(ty, self.std.boolean) || self.is_std(ty, self.std.bit);
        // The matching operators are predefined for `bit` and `std_ulogic`.
        let matching = |t: Ty| self.is_std(t, self.std.bit) || self.is_std_ulogic(t);
        // A one-dimensional array and its element type.
        let array = match def {
            TypeDef::Array { indexes, element } if indexes.len() == 1 => {
                Some(element.and_then(|e| self.mark_type(e)))
            }
            _ => None,
        };
        let element_bitlike = matches!(array, Some(Some(e))
            if self.is_std(e.ty, self.std.boolean) || self.is_std(e.ty, self.std.bit));
        let element_discrete = match array {
            Some(Some(e)) => matches!(
                self.def(e.ty),
                None | Some(TypeDef::Enum(_) | TypeDef::Integer | TypeDef::Scalar)
            ),
            Some(None) => true,
            None => false,
        };
        let element = |e: Option<TyS>| e.map_or(Param::Any, |e| Param::Ty(e.ty));
        match (op, arity) {
            ("=" | "/=", 2) => {
                if !matches!(def, TypeDef::File(_) | TypeDef::Protected) {
                    add(vec![same, same], boolean);
                }
            }
            ("<" | "<=" | ">" | ">=", 2) => {
                if scalar || element_discrete {
                    add(vec![same, same], boolean);
                }
            }
            ("?=" | "?/=" | "?<" | "?<=" | "?>" | "?>=", 2) => {
                if matching(ty) {
                    add(vec![same, same], Some(t));
                } else if matches!(op, "?=" | "?/=")
                    && let Some(Some(e)) = array
                    && matching(e.ty)
                {
                    add(vec![same, same], Some(e));
                }
            }
            ("and" | "or" | "nand" | "nor" | "xor" | "xnor", 2) => {
                if bitlike {
                    add(vec![same, same], Some(t));
                } else if element_bitlike {
                    let e = element(array.flatten());
                    add(vec![same, same], Some(t));
                    add(vec![same, e], Some(t));
                    add(vec![e, same], Some(t));
                }
            }
            ("and" | "or" | "nand" | "nor" | "xor" | "xnor", 1) => {
                if element_bitlike {
                    add(vec![same], array.flatten());
                }
            }
            ("not", 1) => {
                if bitlike || element_bitlike {
                    add(vec![same], Some(t));
                }
            }
            ("sll" | "srl" | "sla" | "sra" | "rol" | "ror", 2) => {
                if element_bitlike {
                    add(vec![same, Param::AnyInteger], Some(t));
                }
            }
            ("+" | "-", 2) => {
                if numeric {
                    add(vec![same, same], Some(t));
                }
            }
            ("+" | "-" | "abs", 1) => {
                if numeric {
                    add(vec![same], Some(t));
                }
            }
            ("&", 2) => {
                if let Some(e) = array {
                    let e = element(e);
                    add(vec![same, same], Some(t));
                    add(vec![same, e], Some(t));
                    add(vec![e, same], Some(t));
                    add(vec![e, e], Some(t));
                }
            }
            ("*", 2) => {
                if integer || real {
                    add(vec![same, same], Some(t));
                }
                if ty == Ty::UReal {
                    add(vec![same, Param::Ty(Ty::UInt)], Some(t));
                    add(vec![Param::Ty(Ty::UInt), same], Some(t));
                }
                if matches!(def, TypeDef::Physical) {
                    add(vec![same, Param::AnyInteger], Some(t));
                    add(vec![same, Param::AnyReal], Some(t));
                    add(vec![Param::AnyInteger, same], Some(t));
                    add(vec![Param::AnyReal, same], Some(t));
                }
            }
            ("/", 2) => {
                if integer || real {
                    add(vec![same, same], Some(t));
                }
                if ty == Ty::UReal {
                    add(vec![same, Param::Ty(Ty::UInt)], Some(t));
                }
                if matches!(def, TypeDef::Physical) {
                    add(vec![same, Param::AnyInteger], Some(t));
                    add(vec![same, Param::AnyReal], Some(t));
                    add(vec![same, same], Some(TyS::of(Ty::UInt)));
                }
            }
            ("mod" | "rem", 2) => {
                if integer || matches!(def, TypeDef::Physical) {
                    add(vec![same, same], Some(t));
                }
            }
            ("**", 2) => {
                if integer || real {
                    add(vec![same, Param::AnyInteger], Some(t));
                }
            }
            ("??", 1) if self.is_std(ty, self.std.bit) => add(vec![same], boolean),
            _ => {}
        }
    }

    // ---- Checking -----------------------------------------------------------

    /// Checks an expression against the type its context expects (if any),
    /// resolving the overloads inside it.
    fn check(&mut self, node: &SyntaxNode, exp: Option<Exp>) {
        if self.depth >= MAX_DEPTH {
            return;
        }
        self.depth += 1;
        self.check_node(node, exp);
        self.depth -= 1;
    }

    fn check_node(&mut self, node: &SyntaxNode, exp: Option<Exp>) {
        use SyntaxKind::*;
        match node.kind() {
            PAREN_EXPR => {
                if let Some(inner) = node.children().next() {
                    self.check(&inner, exp);
                }
            }
            CALL_EXPR => self.check_call(node, exp, false),
            BINARY_EXPR | UNARY_EXPR => self.check_operator(node, exp),
            AGGREGATE => self.check_aggregate(node, exp),
            QUALIFIED_EXPR => {
                let mut children = node.children();
                let mark = children.next();
                let t = mark.as_ref().and_then(|m| self.type_mark_of(m));
                if let Some(operand) = children.next() {
                    match operand.kind() {
                        PAREN_EXPR => {
                            if let Some(inner) = operand.children().next() {
                                self.check(&inner, t.map(Exp::from));
                            }
                        }
                        _ => self.check(&operand, t.map(Exp::from)),
                    }
                }
                let val = self.infer(node);
                self.expect(node, &val, exp);
            }
            CONDITIONAL_EXPR => self.conditional(node, exp),
            NAME => {
                if let Some(token) = mark_token(node) {
                    self.check_name(node, &token, exp);
                }
            }
            SELECTED_NAME => self.check_selected(node, exp),
            ATTRIBUTE_NAME => {
                if let Some(prefix) = node.children().next() {
                    self.check_prefix(&prefix);
                }
                for rest in node.children().skip(1) {
                    self.check(&rest, None);
                }
                let val = self.infer(node);
                self.expect(node, &val, exp);
            }
            LITERAL | PHYSICAL_LITERAL => {
                let val = self.infer(node);
                self.expect(node, &val, exp);
            }
            RANGE_EXPR => {
                for child in node.children() {
                    self.check(&child, exp);
                }
            }
            ALLOCATOR => {
                // `new t'(x)`: the qualified expression checks itself.
                for child in node.children() {
                    for inner in child.children() {
                        if inner.kind() == QUALIFIED_EXPR {
                            self.check(&inner, None);
                        }
                    }
                }
            }
            ERROR => {
                self.quiet += 1;
                for child in node.children() {
                    self.check(&child, None);
                }
                self.quiet -= 1;
            }
            SUBTYPE_INDICATION | EXTERNAL_NAME => {}
            _ => {
                for child in node.children() {
                    self.check(&child, None);
                }
            }
        }
    }

    /// The prefix of an attribute: a type mark, an object, or a name.
    fn check_prefix(&mut self, prefix: &SyntaxNode) {
        match prefix.kind() {
            SyntaxKind::NAME => {}
            SyntaxKind::SELECTED_NAME | SyntaxKind::CALL_EXPR | SyntaxKind::ATTRIBUTE_NAME => {
                self.check(prefix, None)
            }
            _ => {}
        }
    }

    fn check_name(&mut self, node: &SyntaxNode, token: &SyntaxToken, exp: Option<Exp>) {
        let val = self.infer(node);
        if let Some(exp) = exp {
            // An overloaded enumeration literal or function: keep the ones
            // of the expected type.
            let decls = self.target_decls(token);
            if decls.len() > 1 {
                let keep: Vec<DeclId> = decls
                    .iter()
                    .copied()
                    .filter(|&id| {
                        let t = match self.decl(id).map(|d| d.kind) {
                            Some(DeclKind::Function | DeclKind::GenericSubprogram) => {
                                if !self.callable_without_args(id) {
                                    return false;
                                }
                                self.return_type(id)
                            }
                            _ => self.decl_type(id),
                        };
                        t.is_none_or(|t| self.ty_compat(t.ty, exp.ty))
                    })
                    .collect();
                self.narrow(token, &keep);
            }
        }
        self.expect(node, &val, exp);
    }

    fn check_selected(&mut self, node: &SyntaxNode, exp: Option<Exp>) {
        let Some(suffix) = selected_suffix(node) else {
            return;
        };
        if self.ref_index(&suffix).is_some() {
            self.check_name(node, &suffix, exp);
            return;
        }
        let Some(prefix) = node.children().next() else {
            return;
        };
        if let Some(ty) = single_type(&self.infer(&prefix))
            && let Some(candidates) = self.methods(ty, &suffix)
        {
            // `obj.f` calls a method without arguments.
            self.check(&prefix, None);
            let keep: Vec<DeclId> = candidates
                .iter()
                .copied()
                .filter(|&c| {
                    self.is_function(c)
                        && self.callable_without_args(c)
                        && exp.is_none_or(|e| {
                            self.return_type(c)
                                .is_none_or(|t| self.ty_compat(t.ty, e.ty))
                        })
                })
                .collect();
            let targets = if keep.is_empty() { &candidates } else { &keep };
            self.added.push(Reference {
                range: suffix.text_range(),
                targets: targets.iter().map(|&c| Target::Decl(c)).collect(),
                access: Access::Other,
            });
            let val = self.infer(node);
            self.expect(node, &val, exp);
            return;
        }
        self.check(&prefix, None);
        if is_designator(suffix.kind()) && suffix.kind() != SyntaxKind::CHAR_LITERAL {
            let prefix_val = self.infer(&prefix);
            if let Some(ty) = single_type(&prefix_val)
                && self.definite(&prefix_val)
            {
                let key = crate::syntax::designator_key(&suffix);
                let is_record = match self.def(ty.ty) {
                    Some(TypeDef::Record(_)) => true,
                    Some(TypeDef::Access(Some(d))) => self
                        .mark_type(*d)
                        .is_some_and(|t| matches!(self.def(t.ty), Some(TypeDef::Record(_)))),
                    _ => false,
                };
                if is_record && self.element_type(ty.ty, &key).is_none() {
                    let message = format!(
                        "`{}` is not an element of `{}`",
                        suffix.text(),
                        self.ty_name(ty.ty, ty.shown)
                    );
                    self.error(codes::UNRESOLVED_NAME, suffix.text_range(), message);
                    return;
                }
            }
        }
        let val = self.infer(node);
        self.expect(node, &val, exp);
    }

    fn conditional(&mut self, node: &SyntaxNode, exp: Option<Exp>) {
        let mut after_when = false;
        for child in node.children_with_tokens() {
            match child {
                NodeOrToken::Token(t) => match t.kind() {
                    SyntaxKind::WHEN_KW => after_when = true,
                    SyntaxKind::ELSE_KW => after_when = false,
                    _ => {}
                },
                NodeOrToken::Node(n) => {
                    if after_when {
                        self.condition(&n);
                    } else {
                        self.check(&n, exp);
                    }
                }
            }
        }
    }

    /// A condition: boolean, or (VHDL-2008) a type with a `??` operator.
    fn condition(&mut self, node: &SyntaxNode) {
        let boolean = self.std_ty(self.std.boolean).map(Exp::from);
        let val = self.infer(node);
        match boolean {
            Some(b) if self.compat(&val, b.ty) => self.check(node, Some(b)),
            Some(b) => {
                self.check(node, None);
                // `??` makes bit and std_ulogic (any enumeration with a `??`)
                // conditions; other types never are.
                if let Val::Types(types) = &val
                    && self.definite(&val)
                    && types
                        .iter()
                        .all(|t| !matches!(self.def(t.ty), Some(TypeDef::Enum(_))))
                {
                    self.mismatch(node, &val, b);
                }
            }
            None => self.check(node, None),
        }
    }

    fn check_operator(&mut self, node: &SyntaxNode, exp: Option<Exp>) {
        // The context may admit operators the operands alone don't show:
        // `"0" & x` builds an array of the expected type.
        let (viable, vals, opaque) = self.operator(node, exp);
        let operands: Vec<SyntaxNode> = node.children().collect();
        let Some(op) = operator_token(node) else {
            return;
        };
        let symbol = op.text().to_ascii_lowercase();
        if viable.is_empty() {
            for operand in &operands {
                self.check(operand, None);
            }
            // `&` builds arrays from elements, which only the context types.
            // Literals alone (`'A' /= 'a'`, `B"1100" sll 3`) take their
            // type from the context, which may not be known here.
            let known = vals.iter().all(|v| self.definite(v))
                && vals.iter().any(|v| {
                    matches!(v, Val::Types(ts) if ts.iter().any(|t| matches!(t.ty, Ty::Decl(_))))
                });
            if !opaque && known && symbol != "&" {
                let operands: Vec<String> = vals.iter().map(|v| self.describe(v)).collect();
                let message = format!("no `{}` operator for {}", op.text(), operands.join(" and "));
                self.error(codes::NO_MATCHING_OVERLOAD, op.text_range(), message);
            }
            return;
        }
        let mut chosen = viable;
        if let Some(exp) = exp {
            let fitting: Vec<OpSig> = chosen
                .iter()
                .filter(|s| s.ret.is_none_or(|r| self.ty_compat(r.ty, exp.ty)))
                .cloned()
                .collect();
            if fitting.is_empty() {
                let val = self.rets(chosen.iter().map(|s| s.ret));
                for operand in &operands {
                    self.check(operand, None);
                }
                self.expect(node, &val, Some(exp));
                return;
            }
            chosen = fitting;
        }
        // Narrow the operator's reference to the declarations chosen.
        if let Some(index) = self.ref_index(&op) {
            let explicit: Vec<DeclId> = chosen.iter().filter_map(|s| s.decl).collect();
            if explicit.is_empty() {
                self.dropped.push(index);
            } else if chosen.iter().all(|s| s.decl.is_some()) {
                self.narrow(&op, &explicit);
            }
        }
        let unique = unique_sig(&chosen);
        for (i, operand) in operands.iter().enumerate() {
            let param = unique.and_then(|s| s.params.get(i).copied());
            let exp = match param {
                Some(Param::Ty(ty)) => Some(Exp { ty, shown: None }),
                _ => None,
            };
            // A universal operand of a specific operator stays unchecked.
            self.check(
                operand,
                exp.filter(|e| !matches!(e.ty, Ty::UInt | Ty::UReal)),
            );
        }
    }

    fn check_call(&mut self, node: &SyntaxNode, exp: Option<Exp>, procedure: bool) {
        let Some(prefix) = node.children().next() else {
            return;
        };
        let list = node
            .children()
            .find(|n| n.kind() == SyntaxKind::ASSOCIATION_LIST);
        match self.callee(&prefix, procedure) {
            Callee::Subprograms {
                candidates,
                token,
                add_ref,
            } => self.check_subprogram_call(
                node,
                &prefix,
                list.as_ref(),
                candidates,
                token,
                add_ref,
                exp,
                procedure,
            ),
            Callee::Conversion(t) => {
                if prefix.kind() == SyntaxKind::SELECTED_NAME {
                    self.check(&prefix, None);
                }
                let args = self.args(list.as_ref());
                for arg in &args {
                    if let Some(actual) = &arg.actual {
                        self.check(actual, None);
                    }
                }
                if let [arg] = args.as_slice()
                    && let Some(actual) = &arg.actual
                    && !self.convertible(&arg.val, t.ty)
                {
                    let message = format!(
                        "cannot convert {} to `{}`",
                        self.describe(&arg.val),
                        self.ty_name(t.ty, t.shown)
                    );
                    self.error(codes::TYPE_MISMATCH, trimmed_range(actual), message);
                }
                let val = Val::Types(vec![t]);
                self.expect(node, &val, exp);
            }
            Callee::Value(prefix_val) => {
                self.check(&prefix, None);
                self.check_indexes(&prefix_val, list.as_ref());
                let val = self.infer(node);
                self.expect(node, &val, exp);
            }
            Callee::Attribute => {
                self.check(&prefix, None);
                let args = self.args(list.as_ref());
                // `t'image(x)`, `t'pos(x)`, ...: the argument is a `t`.
                let attribute = attribute_designator(&prefix);
                let arg_exp = match attribute.as_deref() {
                    Some("image" | "pos" | "succ" | "pred" | "leftof" | "rightof") => prefix
                        .children()
                        .next()
                        .and_then(|p| self.type_mark_of(&p))
                        .map(Exp::from),
                    Some("value") => self.std_ty(self.std.string).map(Exp::from),
                    _ => None,
                };
                for arg in &args {
                    if let Some(actual) = &arg.actual {
                        self.check(actual, arg_exp);
                    }
                }
                let val = self.infer(node);
                self.expect(node, &val, exp);
            }
            Callee::Other => {
                for arg in self.args(list.as_ref()) {
                    if let Some(actual) = &arg.actual {
                        self.check(actual, None);
                    }
                }
            }
        }
    }

    /// Whether a type conversion from `val` to `to` is legal: between
    /// numeric types, between closely related arrays, or to the same type.
    fn convertible(&self, val: &Val, to: Ty) -> bool {
        let Val::Types(types) = val else {
            return true;
        };
        if !self.definite(val) {
            return true;
        }
        types.iter().any(|t| self.closely_related(t.ty, to, 0))
    }

    fn closely_related(&self, a: Ty, b: Ty, depth: u32) -> bool {
        if self.ty_compat(a, b) || depth > 8 {
            return true;
        }
        let numeric = |t: Ty| self.is_integer(t) != Some(false) || self.is_real(t) != Some(false);
        let (Some(x), Some(y)) = (self.def_of(a), self.def_of(b)) else {
            return true;
        };
        match (x, y) {
            (
                TypeDef::Integer | TypeDef::Real | TypeDef::Scalar,
                TypeDef::Integer | TypeDef::Real | TypeDef::Scalar,
            ) => numeric(a) && numeric(b),
            (
                TypeDef::Array {
                    indexes: i1,
                    element: e1,
                },
                TypeDef::Array {
                    indexes: i2,
                    element: e2,
                },
            ) => {
                i1.len() == i2.len()
                    && match (
                        e1.and_then(|e| self.mark_type(e)),
                        e2.and_then(|e| self.mark_type(e)),
                    ) {
                        (Some(e1), Some(e2)) => self.closely_related(e1.ty, e2.ty, depth + 1),
                        _ => true,
                    }
            }
            _ => false,
        }
    }

    fn def_of(&self, ty: Ty) -> Option<&'a TypeDef> {
        match ty {
            Ty::UInt => Some(&TypeDef::Integer),
            Ty::UReal => Some(&TypeDef::Real),
            Ty::Decl(_) => self.def(ty),
        }
    }

    /// The indexes of an indexed name or the range of a slice.
    fn check_indexes(&mut self, prefix_val: &Val, list: Option<&SyntaxNode>) {
        let Some(list) = list else { return };
        let array = self.indexed_array(prefix_val, Some(list));
        let elements: Vec<SyntaxNode> = list
            .children()
            .filter(|n| n.kind() == SyntaxKind::ASSOCIATION_ELEMENT)
            .collect();
        let indexes = match array.and_then(|(t, _)| self.def(t.ty)) {
            Some(TypeDef::Array { indexes, .. }) => indexes.clone(),
            _ => Vec::new(),
        };
        for (i, element) in elements.iter().enumerate() {
            let Some(actual) = element.children().last() else {
                continue;
            };
            let exp = if indexes.len() == elements.len() || array.is_some_and(|(_, s)| s) {
                indexes
                    .get(i)
                    .copied()
                    .flatten()
                    .and_then(|ix| self.mark_type(ix))
                    .map(Exp::from)
            } else {
                None
            };
            match actual.kind() {
                SyntaxKind::RANGE_EXPR => self.check(&actual, exp),
                _ if self.is_range(&actual) => self.check(&actual, None),
                _ => self.check(&actual, exp),
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn check_subprogram_call(
        &mut self,
        node: &SyntaxNode,
        prefix: &SyntaxNode,
        list: Option<&SyntaxNode>,
        candidates: Vec<DeclId>,
        token: Option<SyntaxToken>,
        add_ref: bool,
        exp: Option<Exp>,
        procedure: bool,
    ) {
        if prefix.kind() == SyntaxKind::SELECTED_NAME
            && let Some(p) = prefix.children().next()
        {
            self.check(&p, None);
        }
        let args = self.args(list);
        let mut rejects = Vec::new();
        let mut viable: Vec<(DeclId, Vec<(usize, DeclId)>)> = Vec::new();
        let mut indexed = false;
        for &c in &candidates {
            // `f(1)` may also call `f` without arguments and index its result.
            if !procedure && self.indexable_result(c) {
                indexed = true;
            }
            match self.match_call(c, &args) {
                Ok(pairs) => viable.push((c, pairs)),
                Err(reject) => rejects.push((c, reject)),
            }
        }
        let check_args_loosely = |me: &mut Self| {
            for arg in &args {
                if let Some(actual) = &arg.actual {
                    me.check(actual, None);
                }
            }
        };
        if viable.is_empty() {
            check_args_loosely(self);
            if !indexed {
                self.report_rejects(node, prefix, &args, &rejects);
            }
            if add_ref && let Some(token) = &token {
                self.added.push(Reference {
                    range: token.text_range(),
                    targets: candidates.iter().map(|&c| Target::Decl(c)).collect(),
                    access: Access::Other,
                });
            }
            return;
        }
        // Keep the functions whose result fits the context.
        if let Some(exp) = exp
            && !procedure
        {
            let fitting: Vec<(DeclId, Vec<(usize, DeclId)>)> = viable
                .iter()
                .filter(|(c, _)| {
                    self.return_type(*c)
                        .is_none_or(|t| self.ty_compat(t.ty, exp.ty))
                })
                .cloned()
                .collect();
            if fitting.is_empty() && !indexed {
                check_args_loosely(self);
                let val = self.rets(viable.iter().map(|(c, _)| self.return_type(*c)));
                self.expect(node, &val, Some(exp));
                return;
            }
            if !fitting.is_empty() {
                viable = fitting;
            }
        }
        let chosen: Vec<DeclId> = viable.iter().map(|(c, _)| *c).collect();
        if let Some(token) = &token {
            if add_ref {
                self.added.push(Reference {
                    range: token.text_range(),
                    targets: chosen.iter().map(|&c| Target::Decl(c)).collect(),
                    access: Access::Other,
                });
            } else if !indexed {
                self.narrow(token, &chosen);
            }
        }
        // With one candidate left, its parameter types guide the arguments.
        let unique = self.unique_candidate(&viable);
        let mut expected: Vec<Option<Exp>> = vec![None; args.len()];
        if let Some((_, pairs)) = unique {
            for &(a, p) in pairs {
                expected[a] = self.decl_type(p).map(Exp::from);
            }
        }
        for (arg, exp) in args.iter().zip(expected) {
            if let Some(actual) = &arg.actual {
                self.check(actual, exp);
            }
        }
    }

    /// The single candidate left, counting homographs (a subprogram
    /// declared in a package and again in its body) as one.
    fn unique_candidate<'v>(
        &self,
        viable: &'v [(DeclId, Vec<(usize, DeclId)>)],
    ) -> Option<&'v (DeclId, Vec<(usize, DeclId)>)> {
        let first = viable.first()?;
        let signature = |(c, pairs): &(DeclId, Vec<(usize, DeclId)>)| {
            let params: Vec<Option<Ty>> = pairs
                .iter()
                .map(|(_, p)| self.decl_type(*p).map(|t| t.ty))
                .collect();
            (params, self.return_type(*c).map(|t| t.ty))
        };
        let first_sig = signature(first);
        viable
            .iter()
            .all(|v| signature(v) == first_sig)
            .then_some(first)
    }

    fn report_rejects(
        &mut self,
        node: &SyntaxNode,
        prefix: &SyntaxNode,
        args: &[Arg],
        rejects: &[(DeclId, Reject)],
    ) {
        let range = mark_token(prefix).map_or(trimmed_range(node), |t| t.text_range());
        let name = mark_token(prefix).map_or_else(String::new, |t| t.text().to_owned());
        if let [(c, reject)] = rejects {
            // One subprogram: say exactly what is wrong.
            let what = self.decl(*c).map_or("subprogram", |d| d.kind.describe());
            match reject {
                Reject::TooMany(n) => self.error(
                    codes::NO_MATCHING_OVERLOAD,
                    range,
                    format!(
                        "too many arguments: {what} `{name}` takes {n}, not {}",
                        args.len()
                    ),
                ),
                Reject::NoParam(a) => {
                    let p = match &args[*a].formal {
                        Formal::Named(p) => p.clone(),
                        _ => String::new(),
                    };
                    self.error(
                        codes::NO_MATCHING_OVERLOAD,
                        range,
                        format!("{what} `{name}` has no parameter `{p}`"),
                    )
                }
                Reject::Twice(p) => {
                    let p = self.decl(*p).map_or("", |d| d.spelling.as_str());
                    self.error(
                        codes::NO_MATCHING_OVERLOAD,
                        range,
                        format!("parameter `{p}` of `{name}` is associated twice"),
                    )
                }
                Reject::Missing(p) => {
                    let p = self.decl(*p).map_or("", |d| d.spelling.as_str());
                    self.error(
                        codes::NO_MATCHING_OVERLOAD,
                        range,
                        format!("missing argument for parameter `{p}` of `{name}`"),
                    )
                }
                Reject::Mismatch { arg, param } => {
                    if let Some(actual) = &args[*arg].actual {
                        let t = self.decl_type(*param);
                        if let Some(t) = t {
                            let val = args[*arg].val.clone();
                            self.mismatch(actual, &val, t.into());
                        }
                    }
                }
            }
            return;
        }
        // Several overloads: argument mismatches only when every argument
        // is typed, so the list of types says why.
        let types: Vec<String> = args
            .iter()
            .map(|a| match &a.formal {
                Formal::Named(n) => format!("{n} => {}", self.describe(&a.val)),
                _ => self.describe(&a.val),
            })
            .collect();
        let arity_only = rejects
            .iter()
            .all(|(_, r)| !matches!(r, Reject::Mismatch { .. }));
        if !arity_only && !args.iter().all(|a| self.definite(&a.val)) {
            return;
        }
        let message = if args.is_empty() {
            format!("no overload of `{name}` takes no arguments")
        } else {
            format!("no overload of `{name}` accepts ({})", types.join(", "))
        };
        self.error(codes::NO_MATCHING_OVERLOAD, range, message);
    }

    fn check_aggregate(&mut self, node: &SyntaxNode, exp: Option<Exp>) {
        let val = Val::Aggregate;
        let def = exp.and_then(|e| self.def(e.ty));
        let elements: Vec<SyntaxNode> = node
            .children()
            .filter(|n| n.kind() == SyntaxKind::ELEMENT_ASSOCIATION)
            .collect();
        let parts = |e: &SyntaxNode| -> (Option<SyntaxNode>, Option<SyntaxNode>) {
            let choices = e.children().find(|n| n.kind() == SyntaxKind::CHOICES);
            let value = e
                .children()
                .filter(|n| n.kind() != SyntaxKind::CHOICES)
                .last();
            (choices, value)
        };
        match (exp, def) {
            (Some(exp), Some(TypeDef::Array { indexes, element })) if indexes.len() == 1 => {
                let element = element.and_then(|e| self.mark_type(e));
                let index = indexes[0].and_then(|i| self.mark_type(i)).map(Exp::from);
                for e in &elements {
                    let (choices, value) = parts(e);
                    if let Some(choices) = choices {
                        for choice in choices.children() {
                            if self.is_range(&choice) && choice.kind() != SyntaxKind::RANGE_EXPR {
                                continue;
                            }
                            self.check(&choice, index);
                        }
                    }
                    let Some(value) = value else { continue };
                    let v = self.infer(&value);
                    match element {
                        Some(el) if self.compat(&v, el.ty) => self.check(&value, Some(el.into())),
                        Some(el) => {
                            if self.compat(&v, exp.ty) {
                                self.check(&value, Some(exp));
                            } else {
                                self.check(&value, None);
                                self.expect(&value, &v, Some(el.into()));
                            }
                        }
                        None => self.check(&value, None),
                    }
                }
            }
            (Some(exp), Some(TypeDef::Record(fields))) => {
                let mut position = 0;
                for e in &elements {
                    let (choices, value) = parts(e);
                    let field = match &choices {
                        None => {
                            position += 1;
                            fields.get(position - 1).map(|f| Some(f.clone()))
                        }
                        Some(choices) => {
                            let names: Vec<SyntaxNode> = choices.children().collect();
                            match names.as_slice() {
                                [n] if n.kind() == SyntaxKind::NAME => {
                                    let token = mark_token(n);
                                    let key = token
                                        .as_ref()
                                        .map(|t| identifier_key(t.text()))
                                        .unwrap_or_default();
                                    match fields.iter().find(|f| f.name == key) {
                                        Some(f) => {
                                            if let Some(token) = &token {
                                                self.element_ref(token, exp.ty, key.clone());
                                            }
                                            Some(Some(f.clone()))
                                        }
                                        None => {
                                            if let Some(token) = token {
                                                let message = format!(
                                                    "`{}` is not an element of `{}`",
                                                    token.text(),
                                                    self.ty_name(exp.ty, exp.shown)
                                                );
                                                self.error(
                                                    codes::UNRESOLVED_NAME,
                                                    token.text_range(),
                                                    message,
                                                );
                                            }
                                            Some(None)
                                        }
                                    }
                                }
                                _ => Some(None),
                            }
                        }
                    };
                    let Some(value) = value else { continue };
                    let exp = field
                        .flatten()
                        .and_then(|f| f.subtype)
                        .and_then(|s| self.mark_type(s))
                        .map(Exp::from);
                    self.check(&value, exp);
                }
            }
            _ => {
                for e in &elements {
                    let (choices, value) = parts(e);
                    if let Some(choices) = choices
                        && exp.is_none()
                    {
                        // Record element names are not expressions.
                        for choice in choices.children() {
                            if choice.kind() != SyntaxKind::NAME {
                                self.check(&choice, None);
                            }
                        }
                    }
                    if let Some(value) = value {
                        self.check(&value, None);
                    }
                }
                self.expect(node, &val, exp);
            }
        }
    }

    // ---- Statements and declarations ------------------------------------------

    fn walk(&mut self, node: &SyntaxNode) {
        use SyntaxKind::*;
        match node.kind() {
            SIGNAL_ASSIGN_STMT | VARIABLE_ASSIGN_STMT | SELECTED_ASSIGN_STMT => {
                self.assignment(node)
            }
            IF_STMT | IF_GENERATE_STMT => {
                let mut condition_next = false;
                for child in node.children_with_tokens() {
                    match child {
                        NodeOrToken::Token(t) if t.kind().is_trivia() => {}
                        NodeOrToken::Token(t) => {
                            condition_next = matches!(t.kind(), IF_KW | ELSIF_KW)
                        }
                        NodeOrToken::Node(n) => {
                            if condition_next && n.kind() != LABEL {
                                condition_next = false;
                                self.condition(&n);
                            } else {
                                self.walk(&n);
                            }
                        }
                    }
                }
            }
            CASE_STMT | CASE_GENERATE_STMT => {
                let mut selector = None;
                for child in node.children() {
                    match child.kind() {
                        LABEL => {}
                        CASE_ALTERNATIVE => {
                            for inner in child.children() {
                                if inner.kind() == CHOICES {
                                    for choice in inner.children() {
                                        if self.is_range(&choice) && choice.kind() != RANGE_EXPR {
                                            continue;
                                        }
                                        self.check(&choice, selector);
                                    }
                                } else {
                                    self.walk(&inner);
                                }
                            }
                        }
                        _ if selector.is_none() => {
                            self.check(&child, None);
                            let val = self.infer(&child);
                            selector = single_type(&val).map(Exp::from);
                        }
                        _ => self.walk(&child),
                    }
                }
            }
            LOOP_STMT => {
                for child in node.children() {
                    match child.kind() {
                        WHILE_SCHEME => {
                            if let Some(c) = child.children().next() {
                                self.condition(&c);
                            }
                        }
                        FOR_SCHEME => self.for_scheme(&child, &child),
                        _ => self.walk(&child),
                    }
                }
            }
            FOR_GENERATE_STMT => {
                for child in node.children() {
                    if child.kind() != GENERATE_BODY && child.kind() != LABEL {
                        self.for_range(node, &child);
                    } else {
                        self.walk(&child);
                    }
                }
            }
            EXIT_STMT | NEXT_STMT => {
                for child in node.children() {
                    if child.kind() != LABEL {
                        self.condition(&child);
                    }
                }
            }
            WAIT_STMT => {
                let mut clause = None;
                for child in node.children_with_tokens() {
                    match child {
                        NodeOrToken::Token(t) => {
                            if matches!(t.kind(), UNTIL_KW | FOR_KW | ON_KW) {
                                clause = Some(t.kind());
                            }
                        }
                        NodeOrToken::Node(n) => match clause {
                            Some(UNTIL_KW) => self.condition(&n),
                            Some(FOR_KW) => {
                                let time = self.std_ty(self.std.time).map(Exp::from);
                                self.check(&n, time)
                            }
                            _ => {}
                        },
                    }
                }
            }
            ASSERT_STMT | REPORT_STMT => {
                let mut clause = if node.kind() == ASSERT_STMT {
                    Some(ASSERT_KW)
                } else {
                    Some(REPORT_KW)
                };
                for child in node.children_with_tokens() {
                    match child {
                        NodeOrToken::Token(t) => {
                            if matches!(t.kind(), REPORT_KW | SEVERITY_KW) {
                                clause = Some(t.kind());
                            }
                        }
                        NodeOrToken::Node(n) if n.kind() == LABEL => {}
                        NodeOrToken::Node(n) => match clause {
                            Some(ASSERT_KW) => self.condition(&n),
                            Some(REPORT_KW) => {
                                let string = self.std_ty(self.std.string).map(Exp::from);
                                self.check(&n, string)
                            }
                            Some(SEVERITY_KW) => {
                                let severity = self.std_ty(self.std.severity_level).map(Exp::from);
                                self.check(&n, severity)
                            }
                            _ => {}
                        },
                    }
                }
            }
            RETURN_STMT => {
                let exp = self.returns.last().copied().flatten().map(Exp::from);
                for child in node.children() {
                    if child.kind() != LABEL {
                        if child.kind() == CONDITIONAL_EXPR {
                            self.conditional(&child, exp);
                        } else {
                            self.check(&child, exp);
                        }
                    }
                }
            }
            PROCEDURE_CALL_STMT => {
                for child in node.children() {
                    match child.kind() {
                        LABEL => {}
                        CALL_EXPR => self.check_call(&child, None, true),
                        NAME | SELECTED_NAME => self.procedure_without_args(&child),
                        _ => self.check(&child, None),
                    }
                }
            }
            COMPONENT_INSTANTIATION_STMT => self.instantiation(node),
            BLOCK_STMT => {
                // A guard condition sits between `block (` and `)`.
                for child in node.children() {
                    if child.kind() == LABEL {
                        continue;
                    }
                    if is_expression(child.kind()) {
                        self.condition(&child);
                    } else {
                        self.walk(&child);
                    }
                }
            }
            SIGNAL_DECL | CONSTANT_DECL | VARIABLE_DECL | INTERFACE_OBJECT_DECL => {
                self.object_default(node)
            }
            SUBPROGRAM_BODY => {
                let ret = node
                    .children()
                    .find(|n| matches!(n.kind(), FUNCTION_SPEC | PROCEDURE_SPEC))
                    .and_then(|spec| designator(&spec))
                    .and_then(|name| self.decls_at.get(&name.text_range()).copied())
                    .and_then(|id| {
                        if self.is_function(id) {
                            self.return_type(id)
                        } else {
                            None
                        }
                    });
                self.returns.push(ret);
                for child in node.children() {
                    self.walk(&child);
                }
                self.returns.pop();
            }
            ERROR => {
                self.quiet += 1;
                for child in node.children() {
                    self.walk(&child);
                }
                self.quiet -= 1;
            }
            // Not expressions, or not checked.
            SUBTYPE_INDICATION
            | TYPE_DECL
            | SUBTYPE_DECL
            | ALIAS_DECL
            | ATTRIBUTE_DECL
            | ATTRIBUTE_SPEC
            | USE_CLAUSE
            | LIBRARY_CLAUSE
            | CONTEXT_REFERENCE
            | LABEL
            | SENSITIVITY_LIST
            | PSL_DIRECTIVE
            | CONFIGURATION_SPEC
            | BINDING_INDICATION
            | BLOCK_CONFIGURATION
            | COMPONENT_CONFIGURATION
            | DISCONNECTION_SPEC
            | PACKAGE_INSTANTIATION_DECL
            | SUBPROGRAM_INSTANTIATION_DECL
            | GROUP_DECL
            | GROUP_TEMPLATE_DECL
            | MODE_VIEW_DECL
            | FILE_DECL
            | GENERIC_MAP_ASPECT
            | PORT_MAP_ASPECT
            | INTERFACE_PACKAGE_DECL
            | INTERFACE_TYPE_DECL
            | FUNCTION_SPEC
            | PROCEDURE_SPEC
            | EXTERNAL_NAME
            | SIGNATURE => {
                if matches!(node.kind(), FUNCTION_SPEC | PROCEDURE_SPEC) {
                    // Parameter defaults.
                    for child in node.descendants() {
                        if child.kind() == INTERFACE_OBJECT_DECL {
                            self.object_default(&child);
                        }
                    }
                }
            }
            kind if is_expression(kind) => self.check(node, None),
            _ => {
                for child in node.children() {
                    self.walk(&child);
                }
            }
        }
    }

    /// `p;` or `obj.p;`: a procedure call without arguments.
    fn procedure_without_args(&mut self, name: &SyntaxNode) {
        let Callee::Subprograms {
            candidates,
            token,
            add_ref,
        } = self.callee(name, true)
        else {
            self.check(name, None);
            return;
        };
        let viable: Vec<DeclId> = candidates
            .iter()
            .copied()
            .filter(|&c| self.match_call(c, &[]).is_ok())
            .collect();
        if let Some(token) = &token {
            if add_ref {
                let targets = if viable.is_empty() {
                    &candidates
                } else {
                    &viable
                };
                self.added.push(Reference {
                    range: token.text_range(),
                    targets: targets.iter().map(|&c| Target::Decl(c)).collect(),
                    access: Access::Other,
                });
            } else {
                self.narrow(token, &viable);
            }
        }
        if viable.is_empty() {
            let rejects: Vec<(DeclId, Reject)> = candidates
                .iter()
                .filter_map(|&c| self.match_call(c, &[]).err().map(|r| (c, r)))
                .collect();
            self.report_rejects(name, name, &[], &rejects);
        }
        if name.kind() == SyntaxKind::SELECTED_NAME
            && let Some(p) = name.children().next()
        {
            self.check(&p, None);
        }
    }

    /// `:= default` of an object declaration.
    fn object_default(&mut self, node: &SyntaxNode) {
        let Some(value) = node_after(node, SyntaxKind::COLON_EQ) else {
            return;
        };
        let exp = node
            .children()
            .find(|n| n.kind() == SyntaxKind::IDENT_LIST)
            .and_then(|list| crate::syntax::idents(&list).into_iter().next())
            .and_then(|t| self.decls_at.get(&t.text_range()).copied())
            .and_then(|id| self.decl_type(id))
            .map(Exp::from);
        if value.kind() == SyntaxKind::CONDITIONAL_EXPR {
            self.conditional(&value, exp);
        } else {
            self.check(&value, exp);
        }
    }

    fn assignment(&mut self, node: &SyntaxNode) {
        use SyntaxKind::*;
        let operator = if node.kind() == VARIABLE_ASSIGN_STMT {
            COLON_EQ
        } else {
            LTE
        };
        let selected = node.kind() == SELECTED_ASSIGN_STMT;
        let mut target_exp = None;
        let mut selector = None;
        let mut seen_select = !selected;
        let mut seen_operator = false;
        let mut target_done = false;
        let mut force = false;
        for child in node.children_with_tokens() {
            let n = match child {
                NodeOrToken::Token(t) => {
                    match t.kind() {
                        SELECT_KW => seen_select = true,
                        FORCE_KW => force = true,
                        k if k == operator => seen_operator = true,
                        COLON_EQ if selected => seen_operator = true,
                        _ => {}
                    }
                    continue;
                }
                NodeOrToken::Node(n) => n,
            };
            if n.kind() == LABEL {
                continue;
            }
            if !seen_select {
                // `with selector select`.
                self.check(&n, None);
                selector = single_type(&self.infer(&n)).map(Exp::from);
                continue;
            }
            if !seen_operator && !target_done {
                target_done = true;
                if n.kind() != AGGREGATE {
                    self.check(&n, None);
                    target_exp = single_type(&self.infer(&n)).map(Exp::from);
                } else {
                    self.check(&n, None);
                }
                continue;
            }
            match n.kind() {
                DELAY_MECHANISM => {
                    let time = self.std_ty(self.std.time).map(Exp::from);
                    for e in n.children() {
                        self.check(&e, time);
                    }
                }
                WAVEFORM => self.waveform(&n, target_exp),
                CONDITIONAL_WAVEFORMS => self.conditional_waveforms(&n, target_exp),
                SELECTED_WAVEFORMS => self.selected_waveforms(&n, target_exp, selector),
                CONDITIONAL_EXPR => self.conditional(&n, target_exp),
                _ => {
                    let _ = force;
                    self.check(&n, target_exp)
                }
            }
        }
    }

    fn waveform(&mut self, node: &SyntaxNode, exp: Option<Exp>) {
        let time = self.std_ty(self.std.time).map(Exp::from);
        for element in node
            .children()
            .filter(|n| n.kind() == SyntaxKind::WAVEFORM_ELEMENT)
        {
            let mut after = false;
            for child in element.children_with_tokens() {
                match child {
                    NodeOrToken::Token(t) => after |= t.kind() == SyntaxKind::AFTER_KW,
                    NodeOrToken::Node(n) => {
                        if after {
                            self.check(&n, time);
                        } else if n.kind() == SyntaxKind::LITERAL
                            && has_token(&n, SyntaxKind::NULL_KW)
                        {
                            // A null transaction.
                        } else {
                            self.check(&n, exp);
                        }
                    }
                }
            }
        }
    }

    fn conditional_waveforms(&mut self, node: &SyntaxNode, exp: Option<Exp>) {
        let mut after_when = false;
        for child in node.children_with_tokens() {
            match child {
                NodeOrToken::Token(t) => match t.kind() {
                    SyntaxKind::WHEN_KW => after_when = true,
                    SyntaxKind::ELSE_KW => after_when = false,
                    _ => {}
                },
                NodeOrToken::Node(n) => {
                    if after_when {
                        self.condition(&n);
                    } else if n.kind() == SyntaxKind::WAVEFORM {
                        self.waveform(&n, exp);
                    } else {
                        self.check(&n, exp);
                    }
                }
            }
        }
    }

    fn selected_waveforms(&mut self, node: &SyntaxNode, exp: Option<Exp>, selector: Option<Exp>) {
        for child in node.children() {
            match child.kind() {
                SyntaxKind::WAVEFORM => self.waveform(&child, exp),
                SyntaxKind::CHOICES => {
                    for choice in child.children() {
                        if self.is_range(&choice) && choice.kind() != SyntaxKind::RANGE_EXPR {
                            continue;
                        }
                        self.check(&choice, selector);
                    }
                }
                _ => self.check(&child, exp),
            }
        }
    }

    /// A for loop's range: its parameter gets the range's type.
    fn for_scheme(&mut self, scheme: &SyntaxNode, _loop: &SyntaxNode) {
        for child in scheme.children() {
            self.for_range(scheme, &child);
        }
    }

    fn for_range(&mut self, owner: &SyntaxNode, range: &SyntaxNode) {
        let param = designator(owner).and_then(|t| self.decls_at.get(&t.text_range()).copied());
        let ty = self.range_type(range);
        if let (Some(param), Some(ty)) = (param, ty) {
            self.local_types.insert(param, ty);
        }
        match range.kind() {
            SyntaxKind::RANGE_EXPR => self.check(range, ty.map(Exp::from)),
            SyntaxKind::ATTRIBUTE_NAME => self.check(range, None),
            _ => {}
        }
    }

    /// The type of a discrete range.
    fn range_type(&mut self, range: &SyntaxNode) -> Option<TyS> {
        match range.kind() {
            SyntaxKind::RANGE_EXPR => {
                let bounds: Vec<Val> = range.children().map(|b| self.infer(&b)).collect();
                let mut found = None;
                let mut universal = false;
                for b in &bounds {
                    match single_type(b) {
                        Some(t) if matches!(t.ty, Ty::UInt) => universal = true,
                        Some(t) if matches!(t.ty, Ty::Decl(_)) => {
                            found.get_or_insert(t);
                        }
                        _ => return None,
                    }
                }
                found.or_else(|| {
                    if universal {
                        self.std_ty(self.std.integer)
                    } else {
                        None
                    }
                })
            }
            SyntaxKind::SUBTYPE_INDICATION => range
                .children()
                .find(|n| n.kind() != SyntaxKind::RESOLUTION_INDICATION)
                .and_then(|m| self.type_mark_of(&m)),
            SyntaxKind::NAME | SyntaxKind::SELECTED_NAME => self.type_mark_of(range),
            SyntaxKind::ATTRIBUTE_NAME => {
                let attribute = attribute_designator(range)?;
                if !matches!(attribute.as_str(), "range" | "reverse_range") {
                    return None;
                }
                let prefix = range.children().next()?;
                let t = match self.type_mark_of(&prefix) {
                    Some(t) => t,
                    None => single_type(&self.infer(&prefix))?,
                };
                match self.def(t.ty)? {
                    TypeDef::Array { indexes, .. } => {
                        // `x'range(n)` is not modeled: only one dimension.
                        if indexes.len() != 1 {
                            return None;
                        }
                        indexes[0].and_then(|i| self.mark_type(i))
                    }
                    TypeDef::Enum(_) | TypeDef::Integer | TypeDef::Physical | TypeDef::Real => {
                        Some(t)
                    }
                    _ => None,
                }
            }
            _ => None,
        }
    }

    fn instantiation(&mut self, node: &SyntaxNode) {
        // The instantiated unit's generics and ports, in order.
        let unit = node.children().find(|n| {
            matches!(
                n.kind(),
                SyntaxKind::NAME | SyntaxKind::SELECTED_NAME | SyntaxKind::CALL_EXPR
            )
        });
        let region = unit
            .as_ref()
            .and_then(mark_token)
            .and_then(|t| {
                self.target_decls(&t).into_iter().find_map(|id| {
                    self.decl(id)
                        .filter(|d| matches!(d.kind, DeclKind::Entity | DeclKind::Component))
                })
            })
            .and_then(|d| d.members.clone());
        for aspect in node.children() {
            let generic = match aspect.kind() {
                SyntaxKind::GENERIC_MAP_ASPECT => true,
                SyntaxKind::PORT_MAP_ASPECT => false,
                _ => continue,
            };
            let formals: Vec<DeclId> = region
                .as_ref()
                .map(|r| {
                    r.iter()
                        .filter(|&id| {
                            self.decl(id).is_some_and(|d| match d.kind {
                                DeclKind::Port { .. } => !generic,
                                DeclKind::Generic { .. }
                                | DeclKind::GenericType
                                | DeclKind::GenericSubprogram
                                | DeclKind::GenericPackage => generic,
                                _ => false,
                            })
                        })
                        .collect()
                })
                .unwrap_or_default();
            let Some(list) = aspect
                .children()
                .find(|n| n.kind() == SyntaxKind::ASSOCIATION_LIST)
            else {
                continue;
            };
            let mut position = 0;
            for element in list
                .children()
                .filter(|n| n.kind() == SyntaxKind::ASSOCIATION_ELEMENT)
            {
                let has_arrow = has_token(&element, SyntaxKind::ARROW);
                let nodes: Vec<SyntaxNode> = element.children().collect();
                let (formal_node, actual) = if has_arrow {
                    match nodes.as_slice() {
                        [f, a] => (Some(f.clone()), Some(a.clone())),
                        [f] if node_after(&element, SyntaxKind::ARROW).is_none() => {
                            (Some(f.clone()), None)
                        }
                        [a] => (None, Some(a.clone())),
                        _ => (None, None),
                    }
                } else {
                    (None, nodes.into_iter().next())
                };
                let formal = if has_arrow {
                    formal_node
                        .as_ref()
                        .filter(|f| f.kind() == SyntaxKind::NAME)
                        .and_then(mark_token)
                        .and_then(|t| {
                            self.target_decls(&t)
                                .into_iter()
                                .find(|id| formals.contains(id))
                        })
                } else {
                    position += 1;
                    formals.get(position - 1).copied()
                };
                let Some(actual) = actual else { continue };
                let exp = formal.and_then(|f| {
                    let decl = self.decl(f)?;
                    match decl.kind {
                        DeclKind::Generic { .. } => self.decl_type(f),
                        DeclKind::Port { mode, .. } => {
                            // An output's actual may be wrapped in a
                            // conversion from the port's type.
                            if mode != Mode::In && actual.kind() == SyntaxKind::CALL_EXPR {
                                let prefix = actual.children().next()?;
                                if !matches!(self.callee(&prefix, false), Callee::Value(_)) {
                                    return None;
                                }
                            }
                            if mode == Mode::View {
                                return None;
                            }
                            self.decl_type(f)
                        }
                        _ => None,
                    }
                });
                if exp.is_none()
                    && formal.is_some_and(|f| {
                        self.decl(f).is_some_and(|d| {
                            matches!(
                                d.kind,
                                DeclKind::GenericType
                                    | DeclKind::GenericSubprogram
                                    | DeclKind::GenericPackage
                            )
                        })
                    })
                {
                    // A type, subprogram or package actual.
                    continue;
                }
                let actual = if actual.kind() == SyntaxKind::ERROR {
                    continue;
                } else {
                    actual
                };
                self.check(&actual, exp.map(Exp::from));
            }
        }
    }
}

// ---- Helpers ------------------------------------------------------------------

/// A numeric, character, string, bit string or null literal.
fn literal(node: &SyntaxNode) -> Val {
    let Some(token) = node
        .children_with_tokens()
        .filter_map(|e| e.into_token())
        .find(|t| !t.kind().is_trivia())
    else {
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
    node.children_with_tokens()
        .filter_map(|e| e.into_token())
        .filter(|t| is_designator(t.kind()) || t.kind() == SyntaxKind::ALL_KW)
        .last()
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

//! Checks of the code of a file that rules turn on one by one: indexes
//! and values out of their static range (`sema/index-range`,
//! `sema/value-range`), dead code (`sema/unreachable-code`,
//! `sema/constant-condition`, `sema/unused-subprogram`), comparisons of
//! vectors of different widths (`sema/comparison-width`), constructs
//! synthesis does not support in RTL (`sema/synthesis-*`), and names
//! spelled with another case than their declaration
//! (`style/identifier-case`).
//!
//! Computed on demand for the files the rules are on for, like the driver
//! checks, so that they cost nothing otherwise. Each design unit is walked
//! once, building syntax nodes only for the kinds the enabled checks look
//! at; names are looked up in the references name resolution and type
//! checking recorded.
//!
//! Static values are integer expressions of literals and constants; with
//! the `generics` option of `sema/index-range`, generics with a default
//! value count as constants of that value. A finding that depends on a
//! generic's default is not reported under an `if` or `case` generate,
//! which often exists to handle other values.

use std::cell::{Cell, RefCell};

use rustc_hash::{FxHashMap, FxHashSet};
use vfl_syntax::green_walk::{ChildTokens, GreenAt, descendants_where, descendants_where_green};
use vfl_syntax::{SyntaxKind, SyntaxNode, TextRange, Vhdl};

use crate::analysis::Analysis;
use crate::decl::{Access, Decl, DeclId, DeclKind, FileId, Reference, Target, TypeDef, UnitId};
use crate::diagnostic::{Diagnostic, Edit, Fix, codes};
use crate::resolver::mark_token;
use crate::rtl::cases::declaration_node;
use crate::rtl::{literal_width, static_int};
use crate::syntax::is_designator;

/// Which checks to run, with their options.
#[derive(Debug, Clone, Copy, Default)]
pub struct CodeChecks {
    /// `sema/index-range`.
    pub index_range: bool,
    /// Its `generics` option: generics with a default count as constants.
    pub range_generics: bool,
    /// `sema/value-range`.
    pub value_range: bool,
    /// `sema/unreachable-code`.
    pub unreachable_code: bool,
    /// `sema/constant-condition`.
    pub constant_condition: bool,
    /// Its `constants` option: conditions that are a boolean constant.
    pub condition_constants: bool,
    /// `sema/unused-subprogram`.
    pub unused_subprogram: bool,
    /// Its `public` option: also the subprograms of packages, unused in
    /// the whole project.
    pub public_subprograms: bool,
    /// `sema/comparison-width`.
    pub comparison_width: bool,
    /// `sema/synthesis-delay`.
    pub synthesis_delay: bool,
    /// `sema/synthesis-file-io`.
    pub synthesis_file_io: bool,
    /// `sema/synthesis-loop`.
    pub synthesis_loop: bool,
    /// `sema/synthesis-initial-value`.
    pub synthesis_initial_value: bool,
    /// `sema/synthesis-real-port`.
    pub synthesis_real_port: bool,
    /// `style/identifier-case`.
    pub identifier_case: bool,
    /// Its `libraries` option: names declared in `std` and `ieee` too.
    pub case_libraries: bool,
}

impl CodeChecks {
    fn any(&self) -> bool {
        self.index_range
            || self.value_range
            || self.unreachable_code
            || self.constant_condition
            || self.unused_subprogram
            || self.comparison_width
            || self.synthesis()
            || self.identifier_case
    }

    fn synthesis(&self) -> bool {
        self.synthesis_delay
            || self.synthesis_file_io
            || self.synthesis_loop
            || self.synthesis_initial_value
            || self.synthesis_real_port
    }
}

/// How deep names are followed through constants, generics and subtypes.
const MAX_DEPTH: u32 = 16;

type IntegerBoundsCache = FxHashMap<DeclId, (Option<(Option<i64>, Option<i64>)>, bool)>;

/// A green node of the file being checked.
type Green<'g> = GreenAt<'g, Vhdl>;

/// The bounds of a one-dimensional index constraint.
#[derive(Debug, Clone, Copy)]
struct Bounds {
    left: i64,
    right: i64,
    ascending: bool,
}

impl Bounds {
    fn lo(&self) -> i64 {
        if self.ascending {
            self.left
        } else {
            self.right
        }
    }

    fn hi(&self) -> i64 {
        if self.ascending {
            self.right
        } else {
            self.left
        }
    }

    fn is_null(&self) -> bool {
        self.lo() > self.hi()
    }

    fn direction(&self) -> &'static str {
        if self.ascending { "to" } else { "downto" }
    }

    fn describe(&self) -> String {
        format!("{} {} {}", self.left, self.direction(), self.right)
    }
}

impl Analysis {
    /// The findings of the checks of `checks` in a file.
    pub fn code_checks(&self, file: FileId, checks: &CodeChecks) -> Vec<Diagnostic> {
        let Some(analyzed) = self.files().get(file as usize) else {
            return Vec::new();
        };
        if analyzed.builtin || analyzed.foreign.is_some() || !checks.any() {
            return Vec::new();
        }
        let root = analyzed.syntax();
        let design_units: Vec<SyntaxNode> = root
            .children()
            .filter(|n| n.kind() == SyntaxKind::DESIGN_UNIT)
            .collect();
        let values = Values {
            analysis: self,
            generics: checks.range_generics,
            cache: RefCell::default(),
            bounds: RefCell::default(),
            integer_bounds: RefCell::default(),
            roots: RefCell::default(),
            used_generic: Cell::new(false),
        };
        values.roots.borrow_mut().insert(file, root.clone());
        let mut out = Vec::new();
        // A file in several libraries has its units once per library: the
        // first is enough.
        let mut seen = FxHashSet::default();
        for unit in self.file_units(file) {
            let info = &self.units()[unit as usize];
            if !seen.insert(info.index) {
                continue;
            }
            let Some(node) = design_units.get(info.index) else {
                continue;
            };
            let mut checker = Checker {
                analysis: self,
                file,
                unit,
                text: &analyzed.text,
                refs: self.unit_references(unit),
                checks,
                values: &values,
                rtl: None,
                decls_at: std::cell::OnceCell::new(),
                simulation_only: Vec::new(),
                out: &mut out,
            };
            checker.run(node, info.kind);
        }
        out.sort_by_key(|d| (d.range.start(), d.code));
        out.dedup_by(|a, b| a.range == b.range && a.code == b.code);
        out
    }
}

// ---- Static values ---------------------------------------------------------

/// Static values of names across the project, cached.
struct Values<'a> {
    analysis: &'a Analysis,
    generics: bool,
    /// The value of each declaration looked at, and whether it comes from a
    /// generic's default.
    cache: RefCell<FxHashMap<DeclId, (Option<i64>, bool)>>,
    /// The index bounds of each array object looked at, and whether they
    /// come from a generic's default.
    bounds: RefCell<FxHashMap<DeclId, (Option<Bounds>, bool)>>,
    /// The same for the bounds of integer objects and subtypes.
    integer_bounds: RefCell<IntegerBoundsCache>,
    roots: RefCell<FxHashMap<FileId, SyntaxNode>>,
    /// Set when a value used since it was last cleared came from a
    /// generic's default.
    used_generic: Cell<bool>,
}

impl<'a> Values<'a> {
    fn root(&self, file: FileId) -> Option<SyntaxNode> {
        if let Some(root) = self.roots.borrow().get(&file) {
            return Some(root.clone());
        }
        let root = self.analysis.files().get(file as usize)?.syntax();
        self.roots.borrow_mut().insert(file, root.clone());
        Some(root)
    }

    /// The declaration statement of a declaration.
    fn declaration(&self, decl: &Decl) -> Option<SyntaxNode> {
        declaration_node(&self.root(decl.file)?, decl.range)
    }

    /// The single declaration the reference at `range` of `unit` names.
    fn target(&self, unit: UnitId, range: TextRange) -> Option<DeclId> {
        let refs = self.analysis.unit_references(unit);
        let i = refs.partition_point(|r| r.range.start() < range.start());
        let r = refs[i..]
            .iter()
            .take_while(|r| r.range.start() == range.start())
            .find(|r| r.range == range)?;
        single_decl(r)
    }

    /// The value of a static integer expression of `unit`.
    fn int(&self, expr: &SyntaxNode, unit: UnitId, depth: u32) -> Option<i64> {
        if depth > MAX_DEPTH {
            return None;
        }
        static_int(expr, &|name| {
            let token = mark_token(name)?;
            self.value(self.target(unit, token.text_range())?, depth + 1)
        })
    }

    /// The value of a constant, or of a generic with a default.
    fn value(&self, id: DeclId, depth: u32) -> Option<i64> {
        if let Some(&(value, generic)) = self.cache.borrow().get(&id) {
            if generic {
                self.used_generic.set(true);
            }
            return value;
        }
        let outer = self.used_generic.replace(false);
        let value = self.compute_value(id, depth);
        let generic = self.used_generic.get();
        self.used_generic.set(outer || generic);
        self.cache.borrow_mut().insert(id, (value, generic));
        value
    }

    fn compute_value(&self, id: DeclId, depth: u32) -> Option<i64> {
        let decl = self.analysis.decl(id)?;
        match decl.kind {
            DeclKind::Constant if decl.value.is_some() => decl.value,
            DeclKind::Constant if self.generics => self.initial_value(decl, id.unit, depth),
            DeclKind::Generic { has_default: true } if self.generics => {
                let value = self.initial_value(decl, id.unit, depth);
                self.used_generic.set(true);
                value
            }
            _ => None,
        }
    }

    /// The value of the expression after `:=` in a declaration.
    fn initial_value(&self, decl: &Decl, unit: UnitId, depth: u32) -> Option<i64> {
        let node = self.declaration(decl)?;
        let expr = initial_expression(&node)?;
        self.int(&expr, unit, depth + 1)
    }

    /// The bounds of a static range of `unit`: `0 to 7`.
    fn range(&self, range: &SyntaxNode, unit: UnitId, depth: u32) -> Option<Bounds> {
        if range.kind() != SyntaxKind::RANGE_EXPR {
            return None;
        }
        let mut bounds = range.children();
        let left = self.int(&bounds.next()?, unit, depth)?;
        let right = self.int(&bounds.next()?, unit, depth)?;
        Some(Bounds {
            left,
            right,
            ascending: range.has_child(|k| k == SyntaxKind::TO_KW),
        })
    }

    /// The bounds of the index of a one-dimensional array object, from its
    /// subtype indication or the subtypes and the type it names.
    fn object_bounds(&self, id: DeclId) -> Option<Bounds> {
        self.cached(&self.bounds, id, || self.compute_object_bounds(id))
    }

    /// A value of a declaration computed once, with whether it came from a
    /// generic's default.
    fn cached<T: Copy>(
        &self,
        cache: &RefCell<FxHashMap<DeclId, (T, bool)>>,
        id: DeclId,
        compute: impl FnOnce() -> T,
    ) -> T {
        if let Some(&(value, generic)) = cache.borrow().get(&id) {
            if generic {
                self.used_generic.set(true);
            }
            return value;
        }
        let outer = self.used_generic.replace(false);
        let value = compute();
        let generic = self.used_generic.get();
        self.used_generic.set(outer || generic);
        cache.borrow_mut().insert(id, (value, generic));
        value
    }

    fn compute_object_bounds(&self, id: DeclId) -> Option<Bounds> {
        let decl = self.analysis.decl(id)?;
        let node = self.declaration(decl)?;
        let indication = node
            .children()
            .find(|n| n.kind() == SyntaxKind::SUBTYPE_INDICATION)?;
        self.indication_bounds(&indication, id.unit, 0)
    }

    fn indication_bounds(
        &self,
        indication: &SyntaxNode,
        unit: UnitId,
        depth: u32,
    ) -> Option<Bounds> {
        use SyntaxKind::*;
        if depth > MAX_DEPTH {
            return None;
        }
        let mut mark = indication
            .children()
            .find(|n| matches!(n.kind(), CALL_EXPR | NAME | SELECTED_NAME))?;
        // `t(0 to 3)(7 downto 0)` (VHDL-2008): the index constraint comes
        // first, the element's after it.
        while mark.kind() == CALL_EXPR {
            match mark.children().next() {
                Some(prefix) if prefix.kind() == CALL_EXPR => mark = prefix,
                _ => break,
            }
        }
        if mark.kind() == CALL_EXPR {
            let list = mark.children().find(|n| n.kind() == ASSOCIATION_LIST)?;
            let range = single_association(&list)?;
            return self.range(&range, unit, depth);
        }
        let mark = self.target(unit, mark_token(&mark)?.text_range())?;
        self.mark_bounds(mark, depth + 1)
    }

    /// The index bounds a constrained array subtype or type gives.
    fn mark_bounds(&self, mark: DeclId, depth: u32) -> Option<Bounds> {
        let decl = self.analysis.decl(mark)?;
        let node = self.declaration(decl)?;
        match decl.kind {
            DeclKind::Subtype => {
                let indication = node
                    .children()
                    .find(|n| n.kind() == SyntaxKind::SUBTYPE_INDICATION)?;
                self.indication_bounds(&indication, mark.unit, depth)
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
                self.range(&index, mark.unit, depth)
            }
            _ => None,
        }
    }

    /// The low and high bounds of an integer object's subtype, or of a
    /// subtype: either may be unknown (`natural` has no static high
    /// bound here).
    fn integer_bounds(&self, id: DeclId) -> Option<(Option<i64>, Option<i64>)> {
        self.cached(&self.integer_bounds, id, || self.compute_integer_bounds(id))
    }

    fn compute_integer_bounds(&self, id: DeclId) -> Option<(Option<i64>, Option<i64>)> {
        let mut id = id;
        for depth in 0..MAX_DEPTH {
            let decl = self.analysis.decl(id)?;
            let node = self.declaration(decl)?;
            let range = if decl.kind == DeclKind::Type {
                if !matches!(decl.type_def.as_deref(), Some(TypeDef::Integer)) {
                    return None;
                }
                // `type t is range 0 to 15`.
                node.children()
                    .find(|n| n.kind() == SyntaxKind::RANGE_TYPE_DEF)?
                    .descendants()
                    .find(|n| n.kind() == SyntaxKind::RANGE_EXPR)
            } else {
                let indication = node
                    .children()
                    .find(|n| n.kind() == SyntaxKind::SUBTYPE_INDICATION)?;
                indication
                    .children()
                    .find(|n| n.kind() == SyntaxKind::RANGE_CONSTRAINT)
                    .and_then(|c| c.children().find(|n| n.kind() == SyntaxKind::RANGE_EXPR))
            };
            if let Some(range) = range {
                // The type must be an integer type.
                if decl.kind != DeclKind::Type && !self.is_integer(id) {
                    return None;
                }
                let mut bounds = range.children();
                let left = self.int(&bounds.next()?, id.unit, depth);
                let right = self.int(&bounds.next()?, id.unit, depth);
                let bounds = if range.has_child(|k| k == SyntaxKind::TO_KW) {
                    (left, right)
                } else {
                    (right, left)
                };
                return (bounds.0.is_some() || bounds.1.is_some()).then_some(bounds);
            }
            id = decl.subtype?;
        }
        None
    }

    /// Whether a declaration's type is an integer type.
    fn is_integer(&self, id: DeclId) -> bool {
        let mut id = id;
        for _ in 0..MAX_DEPTH {
            let Some(decl) = self.analysis.decl(id) else {
                return false;
            };
            if decl.kind == DeclKind::Type {
                return matches!(decl.type_def.as_deref(), Some(TypeDef::Integer));
            }
            let Some(next) = decl.subtype else {
                return false;
            };
            id = next;
        }
        false
    }

    /// The bounds of a loop or generate parameter, from its static range.
    fn parameter_bounds(&self, id: DeclId) -> Option<Bounds> {
        let decl = self.analysis.decl(id)?;
        let root = self.root(decl.file)?;
        let token = root.covering_element(decl.range);
        let parent = match token {
            rowan::NodeOrToken::Node(n) => n,
            rowan::NodeOrToken::Token(t) => t.parent()?,
        };
        let scheme = parent.ancestors().find(|n| {
            matches!(
                n.kind(),
                SyntaxKind::FOR_SCHEME | SyntaxKind::FOR_GENERATE_STMT
            )
        })?;
        let range = scheme
            .children()
            .find(|n| n.kind() == SyntaxKind::RANGE_EXPR)?;
        self.range(&range, id.unit, 0)
    }

    /// The type definition of an object's type.
    fn object_type(&self, decl: &Decl) -> Option<&'a TypeDef> {
        let mut mark = decl.subtype?;
        for _ in 0..MAX_DEPTH {
            let d = self.analysis.decl(mark)?;
            match d.kind {
                DeclKind::Type => return d.type_def.as_deref(),
                DeclKind::Subtype => mark = d.subtype?,
                _ => return None,
            }
        }
        None
    }
}

/// The expression after `:=` in a declaration.
fn initial_expression(node: &SyntaxNode) -> Option<SyntaxNode> {
    // Count the child nodes before `:=` on the green tree: stepping over
    // the tokens with a cursor builds each of them.
    let colon_eq = SyntaxKind::COLON_EQ as u16;
    let mut before = 0;
    for child in node.green().children() {
        match child {
            rowan::NodeOrToken::Token(t) if t.kind().0 == colon_eq => {
                return node.children().nth(before);
            }
            rowan::NodeOrToken::Node(_) => before += 1,
            rowan::NodeOrToken::Token(_) => {}
        }
    }
    None
}

/// The single element of an association list, when it is positional.
fn single_association(list: &SyntaxNode) -> Option<SyntaxNode> {
    let mut elements = list
        .children()
        .filter(|n| n.kind() == SyntaxKind::ASSOCIATION_ELEMENT);
    let element = elements.next()?;
    if elements.next().is_some() || element.has_child(|k| k == SyntaxKind::ARROW) {
        return None;
    }
    let mut inner = element.children();
    let value = inner.next()?;
    inner.next().is_none().then_some(value)
}

fn single_decl(r: &Reference) -> Option<DeclId> {
    let mut ids = r.targets.iter().filter_map(|t| match t {
        Target::Decl(id) => Some(*id),
        Target::Library(_) => None,
    });
    let id = ids.next()?;
    ids.next().is_none().then_some(id)
}

/// Strips parentheses.
fn unparen(node: &SyntaxNode) -> SyntaxNode {
    let mut node = node.clone();
    while node.kind() == SyntaxKind::PAREN_EXPR {
        match node.children().next() {
            Some(inner) => node = inner,
            None => break,
        }
    }
    node
}

/// Whether a statement kind is a statement (concurrent or sequential).
fn is_statement(kind: SyntaxKind) -> bool {
    use SyntaxKind::*;
    matches!(
        kind,
        PROCESS_STMT
            | BLOCK_STMT
            | COMPONENT_INSTANTIATION_STMT
            | FOR_GENERATE_STMT
            | IF_GENERATE_STMT
            | CASE_GENERATE_STMT
            | SIGNAL_ASSIGN_STMT
            | SELECTED_ASSIGN_STMT
            | VARIABLE_ASSIGN_STMT
            | PROCEDURE_CALL_STMT
            | ASSERT_STMT
            | WAIT_STMT
            | REPORT_STMT
            | IF_STMT
            | CASE_STMT
            | LOOP_STMT
            | NEXT_STMT
            | EXIT_STMT
            | RETURN_STMT
            | NULL_STMT
            | SEQUENTIAL_BLOCK_STMT
            | PSL_DIRECTIVE
    )
}

// ---- The checks ------------------------------------------------------------

struct Checker<'a, 'v> {
    analysis: &'a Analysis,
    file: FileId,
    unit: UnitId,
    text: &'a str,
    /// The unit's references, in source order.
    refs: &'a [Reference],
    checks: &'a CodeChecks,
    values: &'v Values<'a>,
    /// Whether the unit is an architecture of an entity with ports.
    rtl: Option<bool>,
    /// The unit's declarations by name range, built when needed.
    decls_at: std::cell::OnceCell<FxHashMap<TextRange, u32>>,
    /// The regions between `-- synthesis translate_off` and
    /// `-- synthesis translate_on` comments, where synthesis checks stop.
    simulation_only: Vec<TextRange>,
    out: &'v mut Vec<Diagnostic>,
}

impl<'a> Checker<'a, '_> {
    fn report(&mut self, code: &'static str, range: TextRange, message: String) {
        if code.starts_with("sema/synthesis-")
            && self.simulation_only.iter().any(|r| r.contains_range(range))
        {
            return;
        }
        self.out.push(Diagnostic {
            code,
            file: self.file,
            range,
            message,
            fix: None,
        });
    }

    fn decl(&self, id: DeclId) -> Option<&'a Decl> {
        self.analysis.decl(id)
    }

    /// The references inside `range`.
    fn refs_in(&self, range: TextRange) -> &'a [Reference] {
        let from = self
            .refs
            .partition_point(|r| r.range.start() < range.start());
        let to = self.refs.partition_point(|r| r.range.start() < range.end());
        &self.refs[from..to.max(from)]
    }

    fn reference_at(&self, range: TextRange) -> Option<&'a Reference> {
        self.refs_in(range).iter().find(|r| r.range == range)
    }

    /// The single declaration a name (`s`, `pkg.s`) refers to.
    fn named(&self, name: &SyntaxNode) -> Option<(DeclId, &'a Decl)> {
        if !matches!(name.kind(), SyntaxKind::NAME | SyntaxKind::SELECTED_NAME) {
            return None;
        }
        let id = single_decl(self.reference_at(mark_token(name)?.text_range())?)?;
        Some((id, self.decl(id)?))
    }

    fn int(&self, expr: &SyntaxNode) -> Option<i64> {
        self.values.int(expr, self.unit, 0)
    }

    fn line(&self, range: TextRange) -> u32 {
        let file = &self.analysis.files()[self.file as usize];
        file.line_index.line_col(&file.text, range.start()).line + 1
    }

    fn run(&mut self, design_unit: &SyntaxNode, kind: DeclKind) {
        use SyntaxKind::*;
        let checks = *self.checks;
        if kind == DeclKind::Architecture && checks.synthesis() {
            self.rtl = Some(self.is_rtl(design_unit));
            if self.rtl == Some(true) {
                self.simulation_only = simulation_only(design_unit);
            }
        }
        let rtl = self.rtl == Some(true);
        // Expressions and assignments are many: the green tree rules most of
        // them out before a syntax node is built for them.
        // Units with syntax errors may have lost statements.
        let has_error = Cell::new(false);
        let wanted = |g: Green<'_>| match g.kind() {
            ERROR => {
                has_error.set(true);
                false
            }
            CALL_EXPR => checks.index_range && self.may_index(g),
            SIGNAL_DECL if rtl && checks.synthesis_initial_value => true,
            SIGNAL_DECL | VARIABLE_DECL | CONSTANT_DECL | INTERFACE_OBJECT_DECL => {
                checks.value_range && self.initializes_integer(g)
            }
            VARIABLE_ASSIGN_STMT | SIGNAL_ASSIGN_STMT => {
                checks.value_range && self.assigns_integer(g)
            }
            RETURN_STMT | EXIT_STMT | NEXT_STMT => checks.unreachable_code,
            WAIT_STMT => checks.unreachable_code || (rtl && checks.synthesis_delay),
            IF_STMT | CONDITIONAL_WAVEFORMS => checks.constant_condition,
            BINARY_EXPR => checks.comparison_width && self.predefined_comparison(g),
            WAVEFORM_ELEMENT => rtl && checks.synthesis_delay,
            PROCESS_STMT => rtl && (checks.synthesis_file_io || checks.synthesis_loop),
            _ => false,
        };
        let nodes = descendants_where_green(design_unit, wanted);
        let parses = !has_error.get();
        for node in nodes {
            match node.kind() {
                CALL_EXPR => self.index_range(&node),
                SIGNAL_DECL | VARIABLE_DECL | CONSTANT_DECL | INTERFACE_OBJECT_DECL => {
                    if checks.value_range {
                        self.initial_value_range(&node);
                    }
                    if node.kind() == SIGNAL_DECL && rtl && checks.synthesis_initial_value {
                        self.initial_value(&node);
                    }
                }
                VARIABLE_ASSIGN_STMT | SIGNAL_ASSIGN_STMT => self.assigned_value_range(&node),
                RETURN_STMT | EXIT_STMT | NEXT_STMT if parses => self.unreachable(&node),
                WAIT_STMT => {
                    if checks.unreachable_code && parses {
                        self.unreachable(&node);
                    }
                    if rtl && checks.synthesis_delay {
                        self.wait_for(&node);
                    }
                }
                IF_STMT | CONDITIONAL_WAVEFORMS => self.constant_conditions(&node),
                BINARY_EXPR => self.comparison_width(&node),
                WAVEFORM_ELEMENT => self.after_delay(&node),
                PROCESS_STMT => {
                    if checks.synthesis_file_io {
                        self.file_io(&node);
                    }
                    if checks.synthesis_loop {
                        self.loops(&node);
                    }
                }
                _ => {}
            }
        }
        if checks.unused_subprogram {
            self.unused_subprograms(kind);
        }
        if checks.synthesis_real_port && kind == DeclKind::Entity {
            self.real_ports();
        }
        if checks.identifier_case {
            self.identifier_case();
        }
    }

    /// The declaration a name (`s`, `pkg.s`) of the green tree refers to.
    fn green_named(&self, name: Green<'_>) -> Option<(DeclId, &'a Decl)> {
        let mut designators = name.tokens().filter(|(k, _)| is_designator(*k));
        let token = match name.kind() {
            SyntaxKind::NAME => designators.next()?,
            SyntaxKind::SELECTED_NAME => designators.last()?,
            _ => return None,
        };
        let id = single_decl(self.reference_at(token.1)?)?;
        Some((id, self.decl(id)?))
    }

    /// Whether a call can be an index or slice `index_range` reports: of
    /// a one-dimensional array object with known bounds, by a single
    /// positional index that is not a signal or variable.
    fn may_index(&self, call: Green<'_>) -> bool {
        use SyntaxKind::*;
        let mut nodes = call.nodes();
        let Some((id, decl)) = nodes.next().and_then(|p| self.green_named(p)) else {
            return false;
        };
        if !matches!(
            decl.kind,
            DeclKind::Signal
                | DeclKind::Variable
                | DeclKind::Constant
                | DeclKind::Port { .. }
                | DeclKind::Generic { .. }
        ) || !matches!(self.values.object_type(decl), Some(TypeDef::Array { indexes, .. }) if indexes.len() == 1)
        {
            return false;
        }
        let Some(list) = nodes.find(|n| n.kind() == ASSOCIATION_LIST) else {
            return false;
        };
        let mut elements = list.nodes().filter(|n| n.kind() == ASSOCIATION_ELEMENT);
        let (Some(element), None) = (elements.next(), elements.next()) else {
            return false;
        };
        let mut inner = element.nodes();
        let (Some(index), None) = (inner.next(), inner.next()) else {
            return false;
        };
        if index.kind() == RANGE_EXPR {
            return true;
        }
        if self.values.object_bounds(id).is_none_or(|b| b.is_null()) {
            return false;
        }
        match index.kind() {
            NAME | SELECTED_NAME => self.green_named(index).is_some_and(|(_, d)| {
                matches!(
                    d.kind,
                    DeclKind::LoopParameter
                        | DeclKind::GenerateParameter
                        | DeclKind::Constant
                        | DeclKind::Generic { .. }
                )
            }),
            _ => true,
        }
    }

    /// Whether an assignment's target is an integer object.
    fn assigns_integer(&self, statement: Green<'_>) -> bool {
        statement
            .nodes()
            .next()
            .and_then(|t| self.green_named(t))
            .is_some_and(|(_, d)| {
                d.kind.is_object() && d.subtype.is_some_and(|s| self.values.is_integer(s))
            })
    }

    /// Whether a binary expression is a relational operator with the
    /// predefined meaning: no explicit operator was chosen for it.
    fn predefined_comparison(&self, expr: Green<'_>) -> bool {
        use SyntaxKind::*;
        expr.tokens()
            .find(|(k, _)| !k.is_trivia())
            .is_some_and(|(k, range)| {
                matches!(k, EQ | NEQ | LT | LTE | GT | GTE) && self.reference_at(range).is_none()
            })
    }

    /// Whether a node is inside an `if` or `case` generate.
    fn in_conditional_generate(node: &SyntaxNode) -> bool {
        node.ancestors().any(|n| {
            matches!(
                n.kind(),
                SyntaxKind::IF_GENERATE_STMT | SyntaxKind::CASE_GENERATE_STMT
            )
        })
    }

    // ---- sema/index-range ---------------------------------------------------

    /// An index or slice of an array object, against the object's bounds.
    fn index_range(&mut self, node: &SyntaxNode) {
        let Some(prefix) = node.children().next() else {
            return;
        };
        let Some((id, decl)) = self.named(&prefix) else {
            return;
        };
        if !matches!(
            decl.kind,
            DeclKind::Signal
                | DeclKind::Variable
                | DeclKind::Constant
                | DeclKind::Port { .. }
                | DeclKind::Generic { .. }
        ) {
            return;
        }
        let Some(list) = node
            .children()
            .find(|n| n.kind() == SyntaxKind::ASSOCIATION_LIST)
        else {
            return;
        };
        let Some(index) = single_association(&list) else {
            return;
        };
        if !matches!(self.values.object_type(decl), Some(TypeDef::Array { indexes, .. }) if indexes.len() == 1)
        {
            return;
        }
        self.values.used_generic.set(false);
        let name = &decl.spelling;
        let message = if index.kind() == SyntaxKind::RANGE_EXPR {
            let Some(slice) = self.values.range(&index, self.unit, 0) else {
                return;
            };
            let bounds = self.values.object_bounds(id);
            if slice.is_null() {
                if self.values.used_generic.get() {
                    return;
                }
                format!(
                    "the slice `{}` of `{name}` is a null range: it has no elements",
                    slice.describe()
                )
            } else if let Some(bounds) = bounds.filter(|b| !b.is_null()) {
                if slice.ascending != bounds.ascending {
                    format!(
                        "the slice `{}` goes `{}` but `{name}` is declared `{}` ({})",
                        slice.describe(),
                        slice.direction(),
                        bounds.direction(),
                        bounds.describe()
                    )
                } else if slice.lo() < bounds.lo() || slice.hi() > bounds.hi() {
                    format!(
                        "the slice `{}` is outside the range of `{name}` ({})",
                        slice.describe(),
                        bounds.describe()
                    )
                } else {
                    return;
                }
            } else {
                return;
            }
        } else {
            let Some(bounds) = self.values.object_bounds(id).filter(|b| !b.is_null()) else {
                return;
            };
            if let Some(value) = self.int(&index) {
                if value >= bounds.lo() && value <= bounds.hi() {
                    return;
                }
                format!(
                    "index {value} is outside the range of `{name}` ({})",
                    bounds.describe()
                )
            } else if let Some((param, p)) = self.named(&index).filter(|(_, d)| {
                matches!(
                    d.kind,
                    DeclKind::LoopParameter | DeclKind::GenerateParameter
                )
            }) {
                let Some(range) = self.values.parameter_bounds(param) else {
                    return;
                };
                if range.is_null() || (range.lo() >= bounds.lo() && range.hi() <= bounds.hi()) {
                    return;
                }
                let value = if range.lo() < bounds.lo() {
                    range.lo()
                } else {
                    range.hi()
                };
                format!(
                    "`{}` takes the value {value}, outside the range of `{name}` ({})",
                    p.spelling,
                    bounds.describe()
                )
            } else {
                return;
            }
        };
        let message = if self.values.used_generic.get() {
            if Self::in_conditional_generate(node) {
                return;
            }
            format!("{message}, with generics at their default values")
        } else {
            message
        };
        self.report(codes::INDEX_RANGE, index.text_range(), message);
    }

    // ---- sema/value-range ---------------------------------------------------

    /// The declaration of this unit whose name is at `range`.
    fn decl_at(&self, range: TextRange) -> Option<DeclId> {
        let unit = self.unit;
        let decls_at = self.decls_at.get_or_init(|| {
            self.analysis
                .unit_decls(unit)
                .iter()
                .enumerate()
                .map(|(i, d)| (d.range, i as u32))
                .collect()
        });
        let &index = decls_at.get(&range)?;
        Some(DeclId { unit, index })
    }

    /// Whether an object declaration gives an integer object an initial
    /// value.
    fn initializes_integer(&self, decl: Green<'_>) -> bool {
        if !decl.tokens().any(|(k, _)| k == SyntaxKind::COLON_EQ) {
            return false;
        }
        decl.nodes()
            .find(|n| n.kind() == SyntaxKind::IDENT_LIST)
            .and_then(|l| l.tokens().find(|(k, _)| *k == SyntaxKind::IDENT))
            .and_then(|(_, range)| self.decl_at(range))
            .and_then(|id| self.decl(id))
            .and_then(|d| d.subtype)
            .is_some_and(|s| self.values.is_integer(s))
    }

    /// The initial value of an object, against its integer subtype.
    fn initial_value_range(&mut self, node: &SyntaxNode) {
        let Some(expr) = initial_expression(node) else {
            return;
        };
        let Some(name) = node
            .children()
            .find(|n| n.kind() == SyntaxKind::IDENT_LIST)
            .and_then(|l| l.child_token(|k| k == SyntaxKind::IDENT))
        else {
            return;
        };
        let Some(id) = self.decl_at(name.text_range()) else {
            return;
        };
        self.check_value(id, &expr);
    }

    /// The value assigned to an integer object.
    fn assigned_value_range(&mut self, node: &SyntaxNode) {
        let Some(target) = node.children().next() else {
            return;
        };
        let Some((id, decl)) = self.named(&target) else {
            return;
        };
        if !decl.kind.is_object() {
            return;
        }
        if node.kind() == SyntaxKind::VARIABLE_ASSIGN_STMT {
            if let Some(expr) = node.children().nth(1) {
                self.check_value(id, &expr);
            }
            return;
        }
        let values: Vec<SyntaxNode> =
            descendants_where(node, |k| k == SyntaxKind::WAVEFORM_ELEMENT)
                .into_iter()
                .filter_map(|w| w.children().next())
                .collect();
        for value in values {
            self.check_value(id, &value);
        }
    }

    fn check_value(&mut self, id: DeclId, expr: &SyntaxNode) {
        // Only expressions of literals and constants: cheap to rule out
        // anything else first.
        if !matches!(
            expr.kind(),
            SyntaxKind::LITERAL
                | SyntaxKind::UNARY_EXPR
                | SyntaxKind::BINARY_EXPR
                | SyntaxKind::PAREN_EXPR
                | SyntaxKind::NAME
                | SyntaxKind::SELECTED_NAME
        ) {
            return;
        }
        let Some(decl) = self.decl(id) else { return };
        if decl.subtype.is_none_or(|s| !self.values.is_integer(s)) {
            return;
        }
        let Some(value) = self.int(expr) else { return };
        let Some((lo, hi)) = self.values.integer_bounds(id) else {
            return;
        };
        if lo.is_some_and(|lo| value < lo) || hi.is_some_and(|hi| value > hi) {
            let range = match (lo, hi) {
                (Some(lo), Some(hi)) => format!("{lo} to {hi}"),
                (Some(lo), None) => format!("at least {lo}"),
                (None, Some(hi)) => format!("at most {hi}"),
                (None, None) => return,
            };
            self.report(
                codes::VALUE_RANGE,
                expr.text_range(),
                format!(
                    "{value} is outside the range of `{}` ({range})",
                    decl.spelling
                ),
            );
        }
    }

    // ---- sema/unreachable-code ----------------------------------------------

    /// Statements after a `return`, an unconditional `exit` or `next`, or a
    /// `wait;`, in the same sequence.
    fn unreachable(&mut self, node: &SyntaxNode) {
        use SyntaxKind::*;
        let what = match node.kind() {
            RETURN_STMT => "return",
            EXIT_STMT | NEXT_STMT if node.has_child(|k| k == WHEN_KW) => return,
            EXIT_STMT => "exit",
            NEXT_STMT => "next",
            WAIT_STMT
                if node.has_child(|k| matches!(k, ON_KW | UNTIL_KW | FOR_KW))
                    || node.children().next().is_some() =>
            {
                return;
            }
            WAIT_STMT => "wait",
            _ => return,
        };
        let mut next = node.next_sibling_or_token();
        while let Some(element) = next {
            next = match &element {
                rowan::NodeOrToken::Node(n) => n.next_sibling_or_token(),
                rowan::NodeOrToken::Token(t) => t.next_sibling_or_token(),
            };
            match element {
                rowan::NodeOrToken::Token(t) if t.kind().is_trivia() => {}
                rowan::NodeOrToken::Token(_) => return,
                rowan::NodeOrToken::Node(n) if is_statement(n.kind()) => {
                    let message = format!(
                        "this statement is never reached: the `{what}` at line {} comes first",
                        self.line(node.text_range())
                    );
                    self.report(codes::UNREACHABLE_CODE, n.text_range(), message);
                    return;
                }
                rowan::NodeOrToken::Node(_) => return,
            }
        }
    }

    // ---- sema/constant-condition --------------------------------------------

    /// Conditions of `if` statements and conditional assignments that are
    /// always true or always false. `if true generate` is a common way to
    /// group statements, and `if false generate` to disable them: `if`
    /// generates are left alone.
    fn constant_conditions(&mut self, node: &SyntaxNode) {
        use SyntaxKind::*;
        let mut after = false;
        let mut conditions = Vec::new();
        for element in node.children_with_tokens() {
            match element {
                rowan::NodeOrToken::Token(t) => {
                    if !t.kind().is_trivia() {
                        after = match node.kind() {
                            CONDITIONAL_WAVEFORMS => t.kind() == WHEN_KW,
                            _ => matches!(t.kind(), IF_KW | ELSIF_KW),
                        };
                    }
                }
                rowan::NodeOrToken::Node(n) => {
                    if after && n.kind() != LABEL {
                        conditions.push(n);
                        after = false;
                    }
                }
            }
        }
        for condition in conditions {
            let constants = self.checks.condition_constants;
            let Some(value) = self.boolean_value(&condition, self.unit, constants, 0) else {
                continue;
            };
            let message = if value {
                "this condition is always `true`: the branches after it never run"
            } else {
                "this condition is always `false`: its branch never runs"
            };
            self.report(
                codes::CONSTANT_CONDITION,
                condition.text_range(),
                message.into(),
            );
        }
    }

    /// The value of a boolean condition of `unit` that is a literal, or a
    /// constant when `constants`, possibly negated.
    fn boolean_value(
        &self,
        condition: &SyntaxNode,
        unit: UnitId,
        constants: bool,
        depth: u32,
    ) -> Option<bool> {
        if depth > MAX_DEPTH {
            return None;
        }
        let condition = unparen(condition);
        match condition.kind() {
            SyntaxKind::UNARY_EXPR => {
                let operator = condition.child_token(|k| !k.is_trivia())?;
                if operator.kind() != SyntaxKind::NOT_KW {
                    return None;
                }
                let operand = condition.children().next()?;
                Some(!self.boolean_value(&operand, unit, constants, depth + 1)?)
            }
            SyntaxKind::NAME => {
                let token = mark_token(&condition)?;
                let literal = match token.text().to_ascii_lowercase().as_str() {
                    "true" => Some(true),
                    "false" => Some(false),
                    _ => None,
                };
                let target = self.values.target(unit, token.text_range());
                if let Some(literal) = literal {
                    // The literal of `boolean`, not of another enumeration.
                    let boolean = target.and_then(|id| self.decl(id)).is_none_or(|d| {
                        self.analysis.files()[d.file as usize].builtin
                            && d.kind == DeclKind::EnumLiteral
                    });
                    return boolean.then_some(literal);
                }
                if !constants {
                    return None;
                }
                let id = target?;
                let decl = self.decl(id)?;
                if decl.kind != DeclKind::Constant {
                    return None;
                }
                let node = self.values.declaration(decl)?;
                let expr = initial_expression(&node)?;
                self.boolean_value(&expr, id.unit, constants, depth + 1)
            }
            _ => None,
        }
    }

    // ---- sema/comparison-width ----------------------------------------------

    /// A predefined comparison of two arrays of different static widths.
    fn comparison_width(&mut self, node: &SyntaxNode) {
        use SyntaxKind::*;
        let Some(operator) = node.child_token(|k| !k.is_trivia()) else {
            return;
        };
        if !matches!(operator.kind(), EQ | NEQ | LT | LTE | GT | GTE) {
            return;
        }
        let mut operands = node.children();
        let (Some(left), Some(right)) = (operands.next(), operands.next()) else {
            return;
        };
        // An explicit operator, such as `numeric_std`'s, compares values.
        if self.reference_at(operator.text_range()).is_some() {
            return;
        }
        let (Some((lw, lobj)), Some((rw, robj))) = (self.width(&left), self.width(&right)) else {
            return;
        };
        if lw == rw || !(lobj || robj) {
            return;
        }
        let result = if matches!(operator.kind(), EQ | NEQ) {
            format!(
                "so `{}` is always {}",
                operator.text(),
                operator.kind() == NEQ
            )
        } else {
            "so the comparison is element by element from the left, not by value".into()
        };
        self.report(
            codes::COMPARISON_WIDTH,
            node.text_range(),
            format!("comparison of {lw} elements with {rw} elements, {result}"),
        );
    }

    /// The static number of elements of an array operand, and whether it is
    /// (part of) an object rather than a literal.
    fn width(&self, expr: &SyntaxNode) -> Option<(u32, bool)> {
        use SyntaxKind::*;
        let expr = unparen(expr);
        match expr.kind() {
            LITERAL => Some((literal_width(&expr.first_token()?)?, false)),
            NAME | SELECTED_NAME => {
                let (_, decl) = self.named(&expr)?;
                if !decl.kind.is_object() {
                    return None;
                }
                match self.values.object_type(decl)? {
                    TypeDef::Array { indexes, .. } if indexes.len() == 1 => {
                        Some((decl.width?, true))
                    }
                    _ => None,
                }
            }
            CALL_EXPR => {
                let (_, decl) = self.named(&expr.children().next()?)?;
                if !decl.kind.is_object() {
                    return None;
                }
                let list = expr.children().find(|n| n.kind() == ASSOCIATION_LIST)?;
                let range = single_association(&list)?;
                let bounds = self.values.range(&range, self.unit, 0)?;
                let width = (bounds.hi() - bounds.lo() + 1).max(0);
                Some((u32::try_from(width).ok()?, true))
            }
            BINARY_EXPR => {
                let operator = expr.child_token(|k| !k.is_trivia())?;
                if operator.kind() != AMP {
                    return None;
                }
                let mut operands = expr.children();
                let (a, ao) = self.width(&operands.next()?)?;
                let (b, bo) = self.width(&operands.next()?)?;
                Some((a + b, ao || bo))
            }
            _ => None,
        }
    }

    // ---- sema/unused-subprogram ---------------------------------------------

    /// Functions and procedures nothing calls. Those of a package or an
    /// entity only with the `public` option, against the whole project.
    fn unused_subprograms(&mut self, kind: DeclKind) {
        let unit = self.unit;
        let decls = self.analysis.unit_decls(unit);
        let id_of = |i: usize| DeclId {
            unit,
            index: i as u32,
        };
        let subprograms: FxHashMap<TextRange, usize> = decls
            .iter()
            .enumerate()
            .filter(|(_, d)| matches!(d.kind, DeclKind::Function | DeclKind::Procedure))
            .map(|(i, d)| (d.range, i))
            .collect();
        if subprograms.is_empty() {
            return;
        }
        // A body completing an earlier declaration: the reference at its
        // name names that declaration.
        let mut completes: FxHashMap<usize, DeclId> = FxHashMap::default();
        for r in self.refs {
            if r.access == Access::Other
                && let Some(&i) = subprograms.get(&r.range)
                && let Some(target) = single_decl(r)
                && target != id_of(i)
            {
                completes.insert(i, target);
            }
        }
        let public_unit = matches!(
            kind,
            DeclKind::Package | DeclKind::Entity | DeclKind::PackageInstance
        );
        let mut candidates: Vec<usize> = Vec::new();
        let mut public = false;
        for &i in subprograms.values() {
            let d = &decls[i];
            // Operators are called through expressions that type checking
            // may leave unresolved.
            if d.name.starts_with('"') || in_protected_type(decls, d) {
                continue;
            }
            // A body is reported with its declaration.
            if completes.contains_key(&i) {
                continue;
            }
            if public_unit && d.parent == Some(id_of(0)) {
                if !self.checks.public_subprograms {
                    continue;
                }
                public = true;
            }
            candidates.push(i);
        }
        if candidates.is_empty() {
            return;
        }
        candidates.sort_unstable();
        // Each candidate, and the bodies completing it here, by declaration.
        let mut family: FxHashMap<DeclId, usize> =
            candidates.iter().map(|&i| (id_of(i), i)).collect();
        for (&body, spec) in &completes {
            if let Some(&i) = family.get(spec) {
                family.insert(id_of(body), i);
            }
        }
        // With the `public` option, the bodies of other units (a package
        // body) completing a candidate join it.
        if public {
            for other in 0..self.analysis.units().len() as UnitId {
                if other == unit {
                    continue;
                }
                let other_decls = self.analysis.unit_decls(other);
                for r in self.analysis.unit_references(other) {
                    if r.access != Access::Other {
                        continue;
                    }
                    let Some(&i) = single_decl(r).and_then(|t| family.get(&t)) else {
                        continue;
                    };
                    if let Some(index) = other_decls.iter().position(|d| {
                        d.range == r.range
                            && matches!(d.kind, DeclKind::Function | DeclKind::Procedure)
                    }) {
                        family.insert(
                            DeclId {
                                unit: other,
                                index: index as u32,
                            },
                            i,
                        );
                    }
                }
            }
        }
        // Where each candidate's declarations are, by unit.
        let mut extents: FxHashMap<usize, Vec<(UnitId, TextRange)>> = FxHashMap::default();
        for (id, &i) in &family {
            if let Some(d) = self.decl(*id) {
                extents.entry(i).or_default().push((id.unit, d.full_range));
            }
        }
        let mut used: FxHashSet<usize> = FxHashSet::default();
        // A reference counts unless it is in the subprogram itself (a
        // recursive call, an end label) or names a body's declaration.
        let mut count = |refs: &[Reference], ref_unit: UnitId| {
            for r in refs {
                for t in &r.targets {
                    let Target::Decl(id) = t else { continue };
                    let Some(&i) = family.get(id) else { continue };
                    if used.contains(&i) {
                        continue;
                    }
                    let inside = extents.get(&i).is_some_and(|e| {
                        e.iter()
                            .any(|(u, e)| *u == ref_unit && e.contains_range(r.range))
                    });
                    let completion = r.access == Access::Other
                        && if ref_unit == unit {
                            subprograms.contains_key(&r.range)
                        } else {
                            self.analysis
                                .unit_decls(ref_unit)
                                .iter()
                                .any(|d| d.range == r.range)
                        };
                    if !inside && !completion {
                        used.insert(i);
                    }
                }
            }
        };
        count(self.refs, unit);
        if public {
            for other in 0..self.analysis.units().len() as UnitId {
                if other != unit {
                    count(self.analysis.unit_references(other), other);
                }
            }
        }
        for i in candidates {
            if used.contains(&i) {
                continue;
            }
            let d = &decls[i];
            self.report(
                codes::UNUSED_SUBPROGRAM,
                d.range,
                format!("{} `{}` is never called", d.kind.describe(), d.spelling),
            );
        }
    }

    // ---- sema/synthesis-* ---------------------------------------------------

    /// Whether an architecture is RTL: its entity has ports.
    fn is_rtl(&self, design_unit: &SyntaxNode) -> bool {
        let Some(body) = design_unit
            .children()
            .find(|n| n.kind() == SyntaxKind::ARCHITECTURE_BODY)
        else {
            return false;
        };
        let Some(entity) = body
            .children()
            .find(|n| n.kind() == SyntaxKind::NAME)
            .and_then(|n| self.named(&n))
            .map(|(_, d)| d)
        else {
            return false;
        };
        entity.members.as_ref().is_some_and(|members| {
            members.iter().any(|id| {
                self.decl(id)
                    .is_some_and(|d| matches!(d.kind, DeclKind::Port { .. }))
            })
        })
    }

    /// `after` in a waveform.
    fn after_delay(&mut self, node: &SyntaxNode) {
        if let Some(after) = node.child_token(|k| k == SyntaxKind::AFTER_KW) {
            let range = TextRange::new(after.text_range().start(), node.text_range().end());
            self.report(
                codes::SYNTHESIS_DELAY,
                range,
                "synthesis ignores `after` delays: simulation and hardware differ".into(),
            );
        }
    }

    /// `wait for`.
    fn wait_for(&mut self, node: &SyntaxNode) {
        if let Some(keyword) = node.child_token(|k| k == SyntaxKind::FOR_KW) {
            let range = TextRange::new(keyword.text_range().start(), node.text_range().end());
            self.report(
                codes::SYNTHESIS_DELAY,
                range,
                "`wait for` cannot be synthesized".into(),
            );
        }
    }

    /// Files and `std.textio` in a process.
    fn file_io(&mut self, process: &SyntaxNode) {
        let file_decl = descendants_where(process, |k| k == SyntaxKind::FILE_DECL)
            .into_iter()
            .next()
            .map(|n| (n.text_range(), "a file".to_owned()));
        let textio = self.textio_unit();
        let reference = self.refs_in(process.text_range()).iter().find_map(|r| {
            let id = single_decl(r)?;
            let decl = self.decl(id)?;
            let file_object = decl.kind == DeclKind::File
                || matches!(decl.kind, DeclKind::Parameter { .. })
                    && matches!(self.values.object_type(decl), Some(TypeDef::File(_)));
            (file_object || Some(id.unit) == textio)
                .then(|| (r.range, format!("`{}`", decl.spelling)))
        });
        let first = match (file_decl, reference) {
            (Some(a), Some(b)) => Some(if a.0.start() <= b.0.start() { a } else { b }),
            (a, b) => a.or(b),
        };
        if let Some((range, what)) = first {
            self.report(
                codes::SYNTHESIS_FILE_IO,
                range,
                format!("file input and output ({what}) in a process cannot be synthesized"),
            );
        }
    }

    /// The unit of `std.textio`.
    fn textio_unit(&self) -> Option<UnitId> {
        self.analysis
            .units()
            .iter()
            .position(|u| {
                u.kind == DeclKind::Package
                    && u.library == "std"
                    && u.name == "textio"
                    && self.analysis.files()[u.file as usize].builtin
            })
            .map(|u| u as UnitId)
    }

    /// Loops of a process whose bounds or condition read a signal or a
    /// variable.
    fn loops(&mut self, process: &SyntaxNode) {
        use SyntaxKind::*;
        for scheme in descendants_where(process, |k| matches!(k, FOR_SCHEME | WHILE_SCHEME)) {
            let read = self.refs_in(scheme.text_range()).iter().find_map(|r| {
                if !r.access.reads() {
                    return None;
                }
                let decl = self.decl(single_decl(r)?)?;
                matches!(
                    decl.kind,
                    DeclKind::Signal | DeclKind::Variable | DeclKind::Port { .. }
                )
                .then(|| decl.spelling.clone())
            });
            let Some(name) = read else { continue };
            let message = if scheme.kind() == FOR_SCHEME {
                format!(
                    "the range of this loop depends on `{name}`, which is not static: synthesis needs a fixed number of iterations"
                )
            } else {
                format!(
                    "this `while` loop depends on `{name}`: synthesis needs a fixed number of iterations"
                )
            };
            self.report(codes::SYNTHESIS_LOOP, scheme.text_range(), message);
        }
    }

    /// A signal with an initial value.
    fn initial_value(&mut self, node: &SyntaxNode) {
        let Some(expr) = initial_expression(node) else {
            return;
        };
        self.report(
            codes::SYNTHESIS_INITIAL_VALUE,
            expr.text_range(),
            "initial value of a signal in RTL: ASICs and some FPGAs ignore it; use a reset".into(),
        );
    }

    /// Ports of type `real` (or arrays of `real`) of an entity.
    fn real_ports(&mut self) {
        let decls = self.analysis.unit_decls(self.unit);
        let mut found = Vec::new();
        for d in decls {
            if !matches!(d.kind, DeclKind::Port { .. }) {
                continue;
            }
            let real = match self.values.object_type(d) {
                Some(TypeDef::Real) => true,
                Some(TypeDef::Array {
                    element: Some(element),
                    ..
                }) => self.is_real(*element),
                _ => false,
            };
            if real {
                found.push((d.range, d.spelling.clone()));
            }
        }
        for (range, name) in found {
            self.report(
                codes::SYNTHESIS_REAL_PORT,
                range,
                format!("port `{name}` has a `real` type, which synthesis does not support"),
            );
        }
    }

    fn is_real(&self, mut mark: DeclId) -> bool {
        for _ in 0..MAX_DEPTH {
            let Some(d) = self.decl(mark) else {
                return false;
            };
            match d.kind {
                DeclKind::Type => return matches!(d.type_def.as_deref(), Some(TypeDef::Real)),
                DeclKind::Subtype => match d.subtype {
                    Some(s) => mark = s,
                    None => return false,
                },
                _ => return false,
            }
        }
        false
    }

    // ---- style/identifier-case ----------------------------------------------

    /// Names spelled with another case than their declaration.
    fn identifier_case(&mut self) {
        let mut found = Vec::new();
        for r in self.refs {
            let Some(text) = self
                .text
                .get(usize::from(r.range.start())..usize::from(r.range.end()))
            else {
                continue;
            };
            if !text.starts_with(|c: char| c.is_ascii_alphabetic()) {
                continue;
            }
            let mut spellings = r.targets.iter().filter_map(|t| match t {
                Target::Decl(id) => self.decl(*id),
                Target::Library(_) => None,
            });
            let Some(first) = spellings.next() else {
                continue;
            };
            if first.spelling == text
                || !first.spelling.eq_ignore_ascii_case(text)
                || spellings.clone().any(|d| d.spelling == text)
            {
                continue;
            }
            let file = &self.analysis.files()[first.file as usize];
            if file.foreign.is_some() || file.builtin && !self.checks.case_libraries {
                continue;
            }
            found.push((r.range, text.to_owned(), first.spelling.clone()));
        }
        for (range, text, spelling) in found {
            self.out.push(Diagnostic {
                code: codes::IDENTIFIER_CASE,
                file: self.file,
                range,
                message: format!("`{text}` is declared as `{spelling}`"),
                fix: Some(Fix {
                    title: format!("Write `{spelling}`"),
                    edits: vec![Edit {
                        range,
                        text: spelling,
                    }],
                    safe: true,
                }),
            });
        }
    }
}

/// Members of protected types are called through objects.
fn in_protected_type(decls: &[Decl], decl: &Decl) -> bool {
    let mut parent = decl.parent;
    while let Some(id) = parent {
        let Some(p) = decls.get(id.index as usize) else {
            return false;
        };
        if p.kind == DeclKind::ProtectedBody || (p.kind == DeclKind::Type && p.members.is_some()) {
            return true;
        }
        parent = p.parent;
    }
    false
}

/// The regions of a unit between `translate_off` and `translate_on`
/// comments (`-- synthesis translate_off`, `-- pragma translate_off`,
/// `-- RTL_SYNTHESIS OFF`), which synthesis tools skip.
fn simulation_only(node: &SyntaxNode) -> Vec<TextRange> {
    let mut out = Vec::new();
    let mut start = None;
    vfl_syntax::green_walk::for_each_token(node, |kind, range, text| {
        if !matches!(kind, SyntaxKind::LINE_COMMENT | SyntaxKind::BLOCK_COMMENT) {
            return;
        }
        let text = text.to_ascii_lowercase().replace([' ', '\t'], "_");
        if text.contains("translate_off") || text.contains("synthesis_off") {
            start.get_or_insert(range.start());
        } else if (text.contains("translate_on") || text.contains("synthesis_on"))
            && let Some(start) = start.take()
        {
            out.push(TextRange::new(start, range.end()));
        }
    });
    if let Some(start) = start {
        out.push(TextRange::new(start, node.text_range().end()));
    }
    out
}

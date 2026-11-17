//! Checks other VHDL tools have (compilers, Sigasi, GHDL, vhdl_ls, RTL
//! linters) that the rest of the analysis does not make: illegal
//! statements and targets, component and entity mismatches, process and
//! loop shapes that hang a simulation, hidden declarations, clocks used as
//! data or made by logic, and more.
//!
//! Computed on demand for the files their rules are on for, like the
//! [`crate::CodeChecks`], so that they cost nothing otherwise. Each design
//! unit is walked once; names are looked up in the references name
//! resolution and type checking recorded.

mod clocks;
mod legality;
mod processes;
mod structure;

use rustc_hash::FxHashMap;
use vfl_syntax::green_walk::ChildTokens;
use vfl_syntax::{SyntaxKind, SyntaxNode, SyntaxToken, TextRange};

use crate::analysis::Analysis;
use crate::decl::{Decl, DeclId, DeclKind, FileId, Reference, Target, UnitId};
use crate::diagnostic::{Diagnostic, Fix};
use crate::resolver::mark_token;
use crate::syntax::is_designator;

/// The codes of these checks.
pub mod extra_codes {
    pub const READ_ONLY_TARGET: &str = "sema/read-only-target";
    pub const ASSIGNMENT_KIND: &str = "sema/assignment-kind";
    pub const MISPLACED_STATEMENT: &str = "sema/misplaced-statement";
    pub const RETURN_VALUE: &str = "sema/return-value";
    pub const COMPONENT_MISMATCH: &str = "sema/component-mismatch";
    pub const MISSING_WAIT: &str = "sema/missing-wait";
    pub const WAIT_IN_SENSITIZED_PROCESS: &str = "sema/wait-in-sensitized-process";
    pub const HIDDEN_DECLARATION: &str = "sema/hidden-declaration";
    pub const CLOCK_AS_DATA: &str = "sema/clock-as-data";
    pub const UNCONSTRAINED_INTEGER: &str = "sema/unconstrained-integer";
    pub const MIXED_CLOCK_EDGES: &str = "sema/mixed-clock-edges";
    pub const GATED_CLOCK: &str = "sema/gated-clock";
    pub const RESET_SOURCE: &str = "sema/reset-source";
    pub const IMPURE_FUNCTION: &str = "sema/impure-function";
    pub const CIRCULAR_DEPENDENCY: &str = "sema/circular-dependency";
    pub const MISSING_BODY: &str = "sema/missing-body";
    pub const ASSOCIATION: &str = "sema/association";
    pub const AMBIGUOUS_CALL: &str = "sema/ambiguous-call";
    pub const CONFIGURATION: &str = "sema/configuration";
    pub const ATTRIBUTE_SPECIFICATION: &str = "sema/attribute-specification";
    pub const CONSTRAINT: &str = "sema/constraint";
    pub const CONFLICTING_USE: &str = "sema/conflicting-use";
    pub const DECLARED_BEFORE: &str = "sema/declared-before";
    pub const STANDARD_FEATURE: &str = "sema/standard-feature";
    pub const SENSITIVITY_LIST_FORM: &str = "sema/sensitivity-list-form";
    pub const INFINITE_LOOP: &str = "sema/infinite-loop";
    pub const STATIC_CONDITION: &str = "sema/static-condition";
    pub const POSTPONED_DELTA: &str = "sema/postponed-delta";
    pub const PORT_BOUNDS: &str = "sema/port-bounds";
    pub const DEFAULT_ASSOCIATION: &str = "sema/default-association";
    pub const STATE_TYPE: &str = "sema/state-type";
    pub const UNNEEDED_OTHERS: &str = "sema/unneeded-others";
    pub const MODE_VIEW: &str = "sema/mode-view";
    pub const SUPERFLUOUS_RESET: &str = "sema/superfluous-reset";
    pub const MULTI_BIT_SYNCHRONIZER: &str = "sema/multi-bit-synchronizer";
    pub const UNREGISTERED_OUTPUT: &str = "sema/unregistered-output";
    pub const INTERNAL_TRISTATE: &str = "sema/internal-tristate";
    pub const METAVALUE_ASSIGNMENT: &str = "sema/metavalue-assignment";
    pub const UNSAFE_FSM: &str = "sema/unsafe-fsm";
    pub const ASSOCIATION_ORDER: &str = "style/association-order";

    /// Every code above.
    pub const ALL: &[&str] = &[
        READ_ONLY_TARGET,
        ASSIGNMENT_KIND,
        MISPLACED_STATEMENT,
        RETURN_VALUE,
        COMPONENT_MISMATCH,
        MISSING_WAIT,
        WAIT_IN_SENSITIZED_PROCESS,
        HIDDEN_DECLARATION,
        CLOCK_AS_DATA,
        UNCONSTRAINED_INTEGER,
        MIXED_CLOCK_EDGES,
        GATED_CLOCK,
        RESET_SOURCE,
        IMPURE_FUNCTION,
        CIRCULAR_DEPENDENCY,
        MISSING_BODY,
        ASSOCIATION,
        AMBIGUOUS_CALL,
        CONFIGURATION,
        ATTRIBUTE_SPECIFICATION,
        CONSTRAINT,
        CONFLICTING_USE,
        DECLARED_BEFORE,
        STANDARD_FEATURE,
        SENSITIVITY_LIST_FORM,
        INFINITE_LOOP,
        STATIC_CONDITION,
        POSTPONED_DELTA,
        PORT_BOUNDS,
        DEFAULT_ASSOCIATION,
        STATE_TYPE,
        UNNEEDED_OTHERS,
        MODE_VIEW,
        SUPERFLUOUS_RESET,
        MULTI_BIT_SYNCHRONIZER,
        UNREGISTERED_OUTPUT,
        INTERNAL_TRISTATE,
        METAVALUE_ASSIGNMENT,
        UNSAFE_FSM,
        ASSOCIATION_ORDER,
    ];
}

/// Whether each check is on: called with a rule id.
pub type Enabled<'a> = &'a dyn Fn(&str) -> bool;

impl Analysis {
    /// The findings of the checks `enabled` turns on in a file.
    pub fn extra_checks(
        &self,
        file: FileId,
        enabled: Enabled,
        standard: vfl_syntax::Standard,
    ) -> Vec<Diagnostic> {
        let Some(analyzed) = self.files().get(file as usize) else {
            return Vec::new();
        };
        if analyzed.builtin || analyzed.foreign.is_some() {
            return Vec::new();
        }
        let root = analyzed.syntax();
        let design_units: Vec<SyntaxNode> = root
            .children()
            .filter(|n| n.kind() == SyntaxKind::DESIGN_UNIT)
            .collect();
        let mut out = Vec::new();
        let mut seen = rustc_hash::FxHashSet::default();
        for unit in self.file_units(file) {
            let info = &self.units()[unit as usize];
            if !seen.insert(info.index) {
                continue;
            }
            let Some(node) = design_units.get(info.index) else {
                continue;
            };
            if node.descendants().any(|n| n.kind() == SyntaxKind::ERROR) {
                continue;
            }
            let ctx = Unit {
                analysis: self,
                file,
                unit,
                text: &analyzed.text,
                refs: self.unit_references(unit),
                decls: self.unit_decls(unit),
                node: node.clone(),
                enabled,
                standard,
                by_range: std::cell::OnceCell::new(),
            };
            ctx.run(&mut out);
        }
        if enabled(extra_codes::DECLARED_BEFORE) || enabled(extra_codes::CIRCULAR_DEPENDENCY) {
            legality::file_order(self, file, enabled, &mut out);
        }
        out.sort_by_key(|d| (d.range.start(), d.code));
        out.dedup_by(|a, b| a.range == b.range && a.code == b.code);
        out
    }
}

/// One design unit being checked.
pub(crate) struct Unit<'a> {
    pub analysis: &'a Analysis,
    pub file: FileId,
    pub unit: UnitId,
    pub text: &'a str,
    pub refs: &'a [Reference],
    pub decls: &'a [Decl],
    pub node: SyntaxNode,
    pub enabled: Enabled<'a>,
    pub standard: vfl_syntax::Standard,
    by_range: std::cell::OnceCell<FxHashMap<TextRange, DeclId>>,
}

impl<'a> Unit<'a> {
    fn run(&self, out: &mut Vec<Diagnostic>) {
        legality::check(self, out);
        processes::check(self, out);
        clocks::check(self, out);
        structure::check(self, out);
    }

    pub fn on(&self, code: &str) -> bool {
        (self.enabled)(code)
    }

    pub fn push(
        &self,
        out: &mut Vec<Diagnostic>,
        code: &'static str,
        range: TextRange,
        message: String,
    ) {
        out.push(Diagnostic {
            code,
            file: self.file,
            range,
            message,
            fix: None,
        });
    }

    pub fn push_fix(
        &self,
        out: &mut Vec<Diagnostic>,
        code: &'static str,
        range: TextRange,
        message: String,
        fix: Fix,
    ) {
        out.push(Diagnostic {
            code,
            file: self.file,
            range,
            message,
            fix: Some(fix),
        });
    }

    pub fn decl(&self, id: DeclId) -> Option<&'a Decl> {
        if id.unit == self.unit {
            self.decls.get(id.index as usize)
        } else {
            self.analysis.decl(id)
        }
    }

    /// The id of one of this unit's declarations from its name range.
    pub fn decl_at(&self, range: TextRange) -> Option<DeclId> {
        self.by_range
            .get_or_init(|| {
                self.decls
                    .iter()
                    .enumerate()
                    .map(|(i, d)| {
                        (
                            d.range,
                            DeclId {
                                unit: self.unit,
                                index: i as u32,
                            },
                        )
                    })
                    .collect()
            })
            .get(&range)
            .copied()
    }

    /// The reference at exactly `range`.
    pub fn reference(&self, range: TextRange) -> Option<&'a Reference> {
        let i = self
            .refs
            .partition_point(|r| r.range.start() < range.start());
        self.refs[i..]
            .iter()
            .take_while(|r| r.range.start() == range.start())
            .find(|r| r.range == range)
    }

    /// The references inside `range`.
    pub fn refs_in(&self, range: TextRange) -> &'a [Reference] {
        let from = self
            .refs
            .partition_point(|r| r.range.start() < range.start());
        let to = self.refs.partition_point(|r| r.range.start() < range.end());
        &self.refs[from..to.max(from)]
    }

    /// The single declaration the token at `range` names.
    pub fn target(&self, range: TextRange) -> Option<DeclId> {
        single_decl(self.reference(range)?)
    }

    /// The declaration a name (`s`, `pkg.s`, `s(3)`) refers to.
    pub fn named(&self, name: &SyntaxNode) -> Option<(DeclId, &'a Decl)> {
        let id = self.target(mark_token(name)?.text_range())?;
        Some((id, self.decl(id)?))
    }

    /// The object an assignment target writes: `s`, `s(3)`, `r.f`,
    /// `s(3 downto 0)`, `pkg.s`.
    pub fn target_object(&self, target: &SyntaxNode) -> Option<(DeclId, &'a Decl)> {
        let mut node = target.clone();
        for _ in 0..16 {
            match node.kind() {
                SyntaxKind::NAME => return self.named(&node),
                SyntaxKind::CALL_EXPR => node = node.children().next()?,
                SyntaxKind::SELECTED_NAME => {
                    let suffix = node.last_child_token(is_designator)?;
                    if let Some(id) = self.target(suffix.text_range())
                        && let Some(decl) = self.decl(id)
                        && decl.kind.is_object()
                    {
                        return Some((id, decl));
                    }
                    node = node.children().next()?;
                }
                _ => return None,
            }
        }
        None
    }

    /// The base type of an object or subtype.
    pub fn base_type(&self, decl: &Decl) -> Option<DeclId> {
        self.analysis.base_type(decl.subtype?)
    }

    /// The name of a declaration of `std.standard`.
    pub fn is_standard(&self, id: DeclId, names: &[&str]) -> bool {
        let Some(decl) = self.decl(id) else {
            return false;
        };
        let info = &self.analysis.units()[id.unit as usize];
        info.library == "std" && info.name == "standard" && names.contains(&decl.name.as_str())
    }

    pub fn line(&self, range: TextRange) -> u32 {
        let file = &self.analysis.files()[self.file as usize];
        file.line_index.line_col(&file.text, range.start()).line + 1
    }
}

/// The single declaration a reference names.
pub(crate) fn single_decl(r: &Reference) -> Option<DeclId> {
    let mut ids = r.targets.iter().filter_map(|t| match t {
        Target::Decl(id) => Some(*id),
        Target::Library(_) => None,
    });
    let id = ids.next()?;
    ids.next().is_none().then_some(id)
}

/// The first token of `node` of kind `kind`, among its own tokens.
pub(crate) fn token(node: &SyntaxNode, kind: SyntaxKind) -> Option<SyntaxToken> {
    node.children_with_tokens()
        .filter_map(|e| e.into_token())
        .find(|t| t.kind() == kind)
}

/// A node's range without leading and trailing trivia.
pub(crate) fn trimmed(node: &SyntaxNode) -> TextRange {
    let mut tokens = node
        .descendants_with_tokens()
        .filter_map(|e| e.into_token())
        .filter(|t| !t.kind().is_trivia());
    match tokens.next() {
        Some(first) => {
            let last = tokens.last().unwrap_or_else(|| first.clone());
            TextRange::new(first.text_range().start(), last.text_range().end())
        }
        None => node.text_range(),
    }
}

/// The statements of a process or subprogram body: the nodes after `begin`.
pub(crate) fn statements(body: &SyntaxNode) -> Vec<SyntaxNode> {
    body.children_with_tokens()
        .skip_while(|e| e.kind() != SyntaxKind::BEGIN_KW)
        .filter_map(|e| e.into_node())
        .collect()
}

/// Whether a declaration kind is a design unit's.
pub(crate) fn is_unit_kind(kind: DeclKind) -> bool {
    matches!(
        kind,
        DeclKind::Entity
            | DeclKind::Architecture
            | DeclKind::Package
            | DeclKind::PackageBody
            | DeclKind::PackageInstance
            | DeclKind::Context
            | DeclKind::Configuration
    )
}

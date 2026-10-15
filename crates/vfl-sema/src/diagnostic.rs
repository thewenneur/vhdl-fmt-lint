//! Problems found by analysis, with optional fixes.

use vfl_syntax::TextRange;

use crate::decl::FileId;

/// A text replacement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edit {
    pub range: TextRange,
    pub text: String,
}

/// A fix for a diagnostic: edits to one file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fix {
    pub title: String,
    pub edits: Vec<Edit>,
    /// Safe fixes keep the design's meaning and are applied by `--fix`;
    /// the others need `--unsafe-fixes`.
    pub safe: bool,
}

/// What removing `item` from a comma-separated list takes: the item and
/// the comma after it, or the comma before it when it is last. `None` when
/// it is the only item.
pub(crate) fn list_item_removal(text: &str, item: TextRange) -> Option<TextRange> {
    let start = usize::from(item.start());
    let end = usize::from(item.end());
    let after = &text[end..];
    let gap = after.len() - after.trim_start().len();
    if after[gap..].starts_with(',') {
        let rest = &after[gap + 1..];
        let spaces = rest.len() - rest.trim_start_matches([' ', '\t']).len();
        let end = end + gap + 1 + spaces;
        return Some(TextRange::new(item.start(), (end as u32).into()));
    }
    let before = text[..start].trim_end();
    before
        .strip_suffix(',')
        .map(|b| TextRange::new((b.len() as u32).into(), item.end()))
}

/// A problem found by semantic analysis. `code` is the lint rule id
/// (`sema/unresolved-name`); the rule's configured level decides whether
/// and how it is reported.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub code: &'static str,
    pub file: FileId,
    pub range: TextRange,
    pub message: String,
    pub fix: Option<Fix>,
}

/// The codes semantic analysis reports.
pub mod codes {
    pub const UNRESOLVED_NAME: &str = "sema/unresolved-name";
    pub const UNRESOLVED_UNIT: &str = "sema/unresolved-unit";
    pub const UNKNOWN_LIBRARY: &str = "sema/unknown-library";
    pub const DUPLICATE_DECLARATION: &str = "sema/duplicate-declaration";
    pub const DUPLICATE_UNIT: &str = "sema/duplicate-unit";
    pub const UNASSOCIATED_PORT: &str = "sema/unassociated-port";
    pub const UNUSED_SIGNAL: &str = "sema/unused-signal";
    pub const UNUSED_VARIABLE: &str = "sema/unused-variable";
    pub const UNUSED_CONSTANT: &str = "sema/unused-constant";
    pub const UNUSED_PORT: &str = "sema/unused-port";
    pub const UNUSED_GENERIC: &str = "sema/unused-generic";
    pub const UNUSED_USE: &str = "sema/unused-use";
    pub const UNREAD_SIGNAL: &str = "sema/unread-signal";
    pub const UNDRIVEN_SIGNAL: &str = "sema/undriven-signal";
    pub const SENSITIVITY_LIST: &str = "sema/sensitivity-list";
    pub const UNNEEDED_SENSITIVITY: &str = "sema/unneeded-sensitivity";
    pub const TYPE_MISMATCH: &str = "sema/type-mismatch";
    pub const NO_MATCHING_OVERLOAD: &str = "sema/no-matching-overload";
    pub const WIDTH_MISMATCH: &str = "sema/width-mismatch";
    pub const INFERRED_LATCH: &str = "sema/inferred-latch";
    pub const MISSING_BRANCH_ASSIGNMENT: &str = "sema/missing-branch-assignment";
    /// [`MISSING_BRANCH_ASSIGNMENT`] in a clocked process, reported only
    /// when the rule's `clocked` option is on.
    pub const MISSING_BRANCH_ASSIGNMENT_CLOCKED: &str = "sema/missing-branch-assignment#clocked";
    pub const CLOCK_DOMAIN_CROSSING: &str = "sema/clock-domain-crossing";
    pub const OPEN_INPUT: &str = "sema/open-input";
    pub const INOUT_PORT: &str = "sema/inout-port";
    pub const MISSING_RESET: &str = "sema/missing-reset";
    pub const MIXED_RESET: &str = "sema/mixed-reset";
    pub const RESET_POLARITY: &str = "sema/reset-polarity";
    pub const FSM_UNREACHABLE_STATE: &str = "sema/fsm-unreachable-state";
    pub const FSM_STUCK_STATE: &str = "sema/fsm-stuck-state";
}

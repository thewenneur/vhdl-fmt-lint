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
}

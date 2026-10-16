//! FPGA vendor files: Quartus settings (`.qsf`, `.qip`), timing
//! constraints (`.sdc`), Vivado constraints (`.xdc`) and Tcl scripts
//! (`.tcl`). [`tcl`] reads them keeping where everything is, [`eval`]
//! resolves the paths they name, and [`check`] finds what is wrong in
//! them: syntax, misspelled commands, options and names, missing
//! arguments and files, duplicate assignments, pin conflicts and ports the
//! design does not have.
//!
//! The rules are those of `vfl-lint` under `vendor/`; it applies their
//! levels, suppressions and waivers.

pub mod check;
pub mod eval;
pub mod known;
mod suggest;
pub mod tcl;

use std::path::Path;

use rustc_hash::{FxHashMap, FxHashSet};
use vfl_syntax::TextRange;

pub use check::check;

/// The kind of a vendor file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    /// Quartus settings (`.qsf`).
    Qsf,
    /// Quartus IP file lists (`.qip`).
    Qip,
    /// Synopsys Design Constraints (`.sdc`), as Quartus, Vivado, Libero,
    /// Lattice and Gowin read them.
    Sdc,
    /// Vivado constraints (`.xdc`).
    Xdc,
    /// Tcl scripts (`.tcl`), such as Vivado build scripts.
    Tcl,
}

impl Kind {
    /// The kind of the file at `path`, by its extension.
    pub fn of(path: &Path) -> Option<Kind> {
        let extension = path.extension()?.to_str()?.to_ascii_lowercase();
        Some(match extension.as_str() {
            "qsf" => Kind::Qsf,
            "qip" => Kind::Qip,
            "sdc" => Kind::Sdc,
            "xdc" => Kind::Xdc,
            "tcl" => Kind::Tcl,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Kind::Qsf => "Quartus settings",
            Kind::Qip => "Quartus IP file",
            Kind::Sdc => "SDC constraints",
            Kind::Xdc => "Vivado constraints",
            Kind::Tcl => "Tcl script",
        }
    }
}

/// The extensions of the vendor files vfl lints.
pub const EXTENSIONS: &[&str] = &["qsf", "qip", "sdc", "xdc", "tcl"];

/// Whether `path` is a vendor file vfl lints.
pub fn is_vendor_file(path: &Path) -> bool {
    Kind::of(path).is_some()
}

/// The rule ids, as in `vfl-lint`.
pub mod rules {
    pub const SYNTAX: &str = "vendor/syntax";
    pub const UNKNOWN_COMMAND: &str = "vendor/unknown-command";
    pub const UNKNOWN_OPTION: &str = "vendor/unknown-option";
    pub const MISSPELLED_NAME: &str = "vendor/misspelled-name";
    pub const MISSING_ARGUMENT: &str = "vendor/missing-argument";
    pub const MISSING_FILE: &str = "vendor/missing-file";
    pub const DUPLICATE_ASSIGNMENT: &str = "vendor/duplicate-assignment";
    pub const PIN_CONFLICT: &str = "vendor/pin-conflict";
    pub const UNKNOWN_PORT: &str = "vendor/unknown-port";
}

/// A problem found in a vendor file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub rule: &'static str,
    pub range: TextRange,
    pub message: String,
}

/// The ports of the design's entities and modules, by lower-case name, for
/// `vendor/unknown-port`.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Ports {
    by_unit: FxHashMap<String, FxHashSet<String>>,
}

impl Ports {
    /// Adds the ports of entity or module `unit`.
    pub fn add(&mut self, unit: &str, ports: impl IntoIterator<Item = impl AsRef<str>>) {
        let set = self.by_unit.entry(unit.to_ascii_lowercase()).or_default();
        set.extend(ports.into_iter().map(|p| p.as_ref().to_ascii_lowercase()));
    }

    /// Whether `unit` is an entity or module of the design.
    pub fn knows(&self, unit: &str) -> bool {
        self.by_unit.contains_key(&unit.to_ascii_lowercase())
    }

    pub fn is_empty(&self) -> bool {
        self.by_unit.is_empty()
    }

    /// Whether `port` is a port of `top`, or of any unit when `top` is
    /// unknown.
    pub fn has(&self, top: Option<&str>, port: &str) -> bool {
        let port = port.to_ascii_lowercase();
        if let Some(ports) = top.and_then(|top| self.by_unit.get(&top.to_ascii_lowercase())) {
            return ports.contains(&port);
        }
        self.by_unit.values().any(|ports| ports.contains(&port))
    }
}

/// What the checks need besides the file.
#[derive(Default)]
pub struct Context<'a> {
    /// The project root, where relative paths of scripts may also start.
    pub root: Option<&'a Path>,
    /// The design's ports; `vendor/unknown-port` is skipped without them.
    pub ports: Option<&'a Ports>,
    /// Report every unknown command, not only the near misses of known
    /// ones.
    pub strict_commands: bool,
}

/// The comments of a file, for suppressions (`# vfl-ignore rule`).
pub fn comments(text: &str) -> Vec<TextRange> {
    tcl::parse(text).comments
}

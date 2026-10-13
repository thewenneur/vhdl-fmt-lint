//! Semantic analysis of VHDL designs across files.
//!
//! [`Analysis::new`] takes parsed files and the libraries they belong to
//! (see `vfl-project`), adds the bundled `std` and `ieee` libraries, and
//! resolves every name of every design unit: use clauses, library units,
//! declarations in nested regions, ports and generics of instantiated
//! entities and components. The result gives declarations, references (for
//! go-to-definition, find-references, rename) and diagnostics (for the
//! linter).
//!
//! Resolution is by name: it does not resolve overloads or check types. A
//! name it cannot check is left alone rather than reported, so valid code
//! gets no false errors. See `docs/design.md`, "Semantic model".
//!
//! ```
//! use std::sync::Arc;
//! use vfl_sema::{Analysis, FileInput};
//! use vfl_syntax::{Standard, parse};
//!
//! let text = "library ieee; use ieee.std_logic_1164.all;\n\
//!             entity e is port (a : in std_logic; b : out std_logic); end;\n\
//!             architecture rtl of e is begin b <= not c; end;\n";
//! let input = FileInput {
//!     path: "e.vhd".into(),
//!     text: Arc::from(text),
//!     green: parse(text, Standard::Vhdl2008).green().clone(),
//!     libraries: vec!["work".into()],
//! };
//! let analysis = Analysis::new(vec![input]);
//! let codes: Vec<_> = analysis.diagnostics().iter().map(|d| d.code).collect();
//! assert!(codes.contains(&"sema/unresolved-name")); // `c`
//! ```

mod analysis;
mod checks;
mod decl;
mod diagnostic;
mod resolver;
mod stdlib;
mod syntax;

pub use analysis::{Analysis, AnalyzedFile, FileInput, UnitInfo};
pub use decl::{Access, Decl, DeclId, DeclKind, FileId, Mode, Reference, Region, Target, UnitId};
pub use diagnostic::{Diagnostic, Edit, Fix, codes};

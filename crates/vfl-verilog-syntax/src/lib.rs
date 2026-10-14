//! Lossless, error-tolerant Verilog and SystemVerilog lexer and parser.
//!
//! [`parse`] turns source text into a concrete syntax tree (a rowan green
//! tree) that holds every byte of the input, so printing the tree gives back
//! the input exactly. Parsing never fails: malformed input yields `ERROR`
//! nodes and [`SyntaxError`]s, and the rest of the file is still parsed.
//!
//! The preprocessor is not run as a separate pass. Compiler directives stay
//! in the tree as trivia tokens, and code in a conditional branch that is
//! not compiled (`` `ifdef `` of an undefined macro) becomes one `INACTIVE`
//! trivia token. Macro uses are not expanded: a macro is parsed as an
//! expression where an expression goes, and as an item or a statement where
//! those go, which covers how macros are used in practice.
//!
//! The grammar covers Verilog (IEEE 1364) and the synthesizable part of
//! SystemVerilog (IEEE 1800): modules, interfaces, packages, programs,
//! types, functions, tasks, generate constructs, statements and
//! expressions. Classes, covergroups, properties, sequences, clocking
//! blocks, constraints and the like are kept as opaque runs of tokens, so
//! that testbench files still parse.
//!
//! ```
//! use vfl_verilog_syntax::{Standard, SyntaxKind, parse};
//!
//! let text = "module m(input logic a, output logic b); assign b = ~a; endmodule\n";
//! let parse = parse(text, Standard::Sv2017);
//! assert!(parse.errors().is_empty());
//! assert_eq!(parse.syntax().to_string(), text);
//! assert!(parse.syntax().descendants().any(|n| n.kind() == SyntaxKind::MODULE_DECL));
//! ```

mod grammar;
mod lexer;
mod parser;
mod syntax_kind;

use std::fmt;
use std::str::FromStr;

pub use lexer::{Token, lex};
pub use parser::TokenSet;
pub use rowan::{GreenNode, TextRange, TextSize};
pub use syntax_kind::{SyntaxElement, SyntaxKind, SyntaxNode, SyntaxToken, Verilog};
pub use vfl_syntax::{LineCol, LineIndex, SyntaxError};

/// A Verilog or SystemVerilog revision. It decides which words are
/// reserved; a `` `begin_keywords `` directive changes it for part of a
/// file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum Standard {
    /// IEEE 1364-1995.
    Verilog1995,
    /// IEEE 1364-2001.
    Verilog2001,
    /// IEEE 1364-2005.
    Verilog2005,
    /// IEEE 1800-2005.
    Sv2005,
    /// IEEE 1800-2009.
    Sv2009,
    /// IEEE 1800-2012.
    Sv2012,
    /// IEEE 1800-2017.
    #[default]
    Sv2017,
    /// IEEE 1800-2023.
    Sv2023,
}

impl Standard {
    pub const ALL: [Standard; 8] = [
        Standard::Verilog1995,
        Standard::Verilog2001,
        Standard::Verilog2005,
        Standard::Sv2005,
        Standard::Sv2009,
        Standard::Sv2012,
        Standard::Sv2017,
        Standard::Sv2023,
    ];

    /// The name used in configuration files and by `` `begin_keywords ``.
    pub fn as_str(self) -> &'static str {
        match self {
            Standard::Verilog1995 => "1364-1995",
            Standard::Verilog2001 => "1364-2001",
            Standard::Verilog2005 => "1364-2005",
            Standard::Sv2005 => "1800-2005",
            Standard::Sv2009 => "1800-2009",
            Standard::Sv2012 => "1800-2012",
            Standard::Sv2017 => "1800-2017",
            Standard::Sv2023 => "1800-2023",
        }
    }

    /// Whether this is a SystemVerilog revision.
    pub fn is_systemverilog(self) -> bool {
        self >= Standard::Sv2005
    }
}

impl fmt::Display for Standard {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownStandard(pub String);

impl fmt::Display for UnknownStandard {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "unknown Verilog standard `{}` (expected 1364-1995, 1364-2001, 1364-2005, \
             1800-2005, 1800-2009, 1800-2012, 1800-2017 or 1800-2023)",
            self.0
        )
    }
}

impl std::error::Error for UnknownStandard {}

impl FromStr for Standard {
    type Err = UnknownStandard;

    /// Accepts the `` `begin_keywords `` names (`1364-2005`, `1800-2017`),
    /// and `1995`, `2001`, `2005` for Verilog and `sv2005` ... `sv2023` for
    /// SystemVerilog.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "1364-1995" | "1995" => Standard::Verilog1995,
            "1364-2001" | "1364-2001-noconfig" | "2001" => Standard::Verilog2001,
            "1364-2005" | "2005" => Standard::Verilog2005,
            "1800-2005" | "sv2005" => Standard::Sv2005,
            "1800-2009" | "sv2009" => Standard::Sv2009,
            "1800-2012" | "sv2012" => Standard::Sv2012,
            "1800-2017" | "sv2017" => Standard::Sv2017,
            "1800-2023" | "sv2023" => Standard::Sv2023,
            _ => return Err(UnknownStandard(s.to_owned())),
        })
    }
}

/// Reads an included file, given the name written after `` `include ``.
pub type IncludeReader<'a> = &'a dyn Fn(&str) -> Option<String>;

/// How to parse a file, beyond its text.
#[derive(Clone, Copy, Default)]
pub struct Options<'a> {
    pub standard: Standard,
    /// Macros defined before the file (`+define+` on a command line).
    pub defines: &'a [String],
    /// Reads an included file, given the name written after `` `include ``.
    /// Only the macros it defines are used, for `` `ifdef `` and to tell
    /// macros that take arguments from those that don't. Without it,
    /// included files are not read.
    pub include: Option<IncludeReader<'a>>,
}

/// The result of parsing a file: a syntax tree and the errors found.
#[derive(Debug, Clone)]
pub struct Parse {
    green: GreenNode,
    errors: Vec<SyntaxError>,
    includes: Vec<String>,
}

impl Parse {
    /// The root `SOURCE_FILE` node.
    pub fn syntax(&self) -> SyntaxNode {
        SyntaxNode::new_root(self.green.clone())
    }

    pub fn green(&self) -> &GreenNode {
        &self.green
    }

    /// Lexer and parser errors, sorted by position.
    pub fn errors(&self) -> &[SyntaxError] {
        &self.errors
    }

    /// The names written after `` `include `` in the file, and in the files
    /// it includes when they were read, in order.
    pub fn includes(&self) -> &[String] {
        &self.includes
    }

    /// An indented dump of the tree, for tests and debugging.
    pub fn debug_tree(&self) -> String {
        debug_tree(&self.syntax())
    }
}

/// Parses a whole file with no predefined macros, without reading included
/// files.
pub fn parse(text: &str, standard: Standard) -> Parse {
    parse_with(
        text,
        &Options {
            standard,
            ..Options::default()
        },
    )
}

/// Parses a whole file.
pub fn parse_with(text: &str, options: &Options) -> Parse {
    let lexed = lex(text, options);
    let mut parser = parser::Parser::new(text, &lexed.tokens);
    parser.source_file();
    let (green, parse_errors) = parser.finish();
    let mut errors = lexed.errors;
    errors.extend(parse_errors);
    errors.sort_by_key(|e| (e.range.start(), e.range.end()));
    Parse {
        green,
        errors,
        includes: lexed.includes,
    }
}

/// An indented dump of a subtree: one line per node or token.
pub fn debug_tree(node: &SyntaxNode) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    let mut indent = 0;
    for event in node.preorder_with_tokens() {
        match event {
            rowan::WalkEvent::Enter(element) => {
                let _ = match &element {
                    rowan::NodeOrToken::Node(n) => {
                        writeln!(out, "{:indent$}{:?}@{:?}", "", n.kind(), n.text_range())
                    }
                    rowan::NodeOrToken::Token(t) => writeln!(
                        out,
                        "{:indent$}{:?}@{:?} {:?}",
                        "",
                        t.kind(),
                        t.text_range(),
                        t.text()
                    ),
                };
                indent += 2;
            }
            rowan::WalkEvent::Leave(_) => indent -= 2,
        }
    }
    out
}

/// Whether `path` names a Verilog or SystemVerilog source by its
/// extension (`.v`, `.vh`, `.sv`, `.svh`), and if so which. Projects
/// configure their own extensions; this is the default.
pub fn default_standard_for_extension(extension: &str) -> Option<Standard> {
    match extension.to_ascii_lowercase().as_str() {
        "v" | "vh" | "vlg" | "verilog" => Some(Standard::Verilog2005),
        "sv" | "svh" | "svi" | "svp" => Some(Standard::Sv2017),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standard_round_trips_through_its_name() {
        for std in Standard::ALL {
            assert_eq!(std.as_str().parse::<Standard>(), Ok(std));
        }
    }

    #[test]
    fn standards_are_ordered_by_age() {
        assert!(Standard::Verilog2005 < Standard::Sv2005);
        assert!(!Standard::Verilog2005.is_systemverilog());
        assert!(Standard::Sv2005.is_systemverilog());
    }
}

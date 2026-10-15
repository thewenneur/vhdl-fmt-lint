//! Lossless, error-tolerant VHDL lexer and parser.
//!
//! [`parse`] turns source text into a concrete syntax tree (a rowan green
//! tree) that holds every byte of the input, whitespace and comments
//! included, so printing the tree gives back the input exactly. Parsing never
//! fails: malformed input yields `ERROR` nodes and [`SyntaxError`]s, and the
//! rest of the file is still parsed. See `docs/design.md`.
//!
//! ```
//! use vfl_syntax::{Standard, SyntaxKind, parse};
//!
//! let text = "entity e is end; -- done\n";
//! let parse = parse(text, Standard::Vhdl2008);
//! assert!(parse.errors().is_empty());
//! assert_eq!(parse.syntax().to_string(), text);
//! assert!(parse.syntax().descendants().any(|n| n.kind() == SyntaxKind::ENTITY_DECL));
//! ```

mod grammar;
pub mod green_walk;
mod lexer;
mod line_index;
mod parser;
mod syntax_kind;

use std::fmt;
use std::str::FromStr;

pub use lexer::{Token, lex};
pub use line_index::{LineCol, LineIndex};
pub use parser::TokenSet;
pub use rowan::{GreenNode, TextRange, TextSize};
pub use syntax_kind::{SyntaxElement, SyntaxKind, SyntaxNode, SyntaxToken, Vhdl};

/// A VHDL language revision.
///
/// The parser accepts constructs from every revision and reports the ones
/// that need a newer standard than the selected one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum Standard {
    /// IEEE 1076-1993 (and 2002, which only added protected types).
    Vhdl1993,
    /// IEEE 1076-2008.
    #[default]
    Vhdl2008,
    /// IEEE 1076-2019.
    Vhdl2019,
}

impl Standard {
    pub const ALL: [Standard; 3] = [Standard::Vhdl1993, Standard::Vhdl2008, Standard::Vhdl2019];

    /// The short name used in configuration files and on the command line.
    pub fn as_str(self) -> &'static str {
        match self {
            Standard::Vhdl1993 => "93",
            Standard::Vhdl2008 => "2008",
            Standard::Vhdl2019 => "2019",
        }
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
            "unknown VHDL standard `{}` (expected 93, 2008 or 2019)",
            self.0
        )
    }
}

impl std::error::Error for UnknownStandard {}

impl FromStr for Standard {
    type Err = UnknownStandard;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "93" | "1993" => Ok(Standard::Vhdl1993),
            "08" | "2008" => Ok(Standard::Vhdl2008),
            "19" | "2019" => Ok(Standard::Vhdl2019),
            _ => Err(UnknownStandard(s.to_owned())),
        }
    }
}

/// The key under which an identifier is looked up: basic identifiers are
/// case-insensitive and folded to lower case, extended identifiers
/// (`\Foo\`) are kept exactly.
pub fn identifier_key(text: &str) -> String {
    if text.starts_with('\\') {
        text.to_owned()
    } else {
        text.to_ascii_lowercase()
    }
}

/// A problem found while lexing or parsing. Never fatal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyntaxError {
    pub message: String,
    pub range: TextRange,
}

impl SyntaxError {
    pub fn new(message: impl Into<String>, range: TextRange) -> Self {
        SyntaxError {
            message: message.into(),
            range,
        }
    }
}

impl fmt::Display for SyntaxError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}: {}", self.range, self.message)
    }
}

/// The result of parsing a file: a syntax tree and the errors found.
#[derive(Debug, Clone)]
pub struct Parse {
    green: GreenNode,
    errors: Vec<SyntaxError>,
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

    /// An indented dump of the tree, for tests and debugging.
    pub fn debug_tree(&self) -> String {
        debug_tree(&self.syntax())
    }
}

/// Parses a whole VHDL file.
///
/// The text must be decoded already. VHDL sources are often Latin-1; decode
/// those byte by byte (`bytes.iter().map(|&b| b as char).collect()`) rather
/// than lossily, so that the tree still reproduces the file.
pub fn parse(text: &str, std: Standard) -> Parse {
    let (tokens, mut errors) = lex(text, std);
    let mut parser = parser::Parser::new(text, &tokens, std);
    parser.source_file();
    let (green, parse_errors) = parser.finish();
    errors.extend(parse_errors);
    errors.sort_by_key(|e| (e.range.start(), e.range.end()));
    Parse { green, errors }
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
    fn identifier_keys_fold_basic_identifiers_only() {
        assert_eq!(identifier_key("Clk_In"), "clk_in");
        assert_eq!(identifier_key("\\Clk\\"), "\\Clk\\");
    }

    #[test]
    fn standards_are_ordered_by_age() {
        assert!(Standard::Vhdl1993 < Standard::Vhdl2008);
        assert!(Standard::Vhdl2008 < Standard::Vhdl2019);
    }
}

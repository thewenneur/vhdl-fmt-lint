//! Configurable VHDL formatter with column alignment.
//!
//! [`format`] parses a file, lays it out again and returns the new text.
//! Every rule comes from [`FormatConfig`] (the `[format]` table of
//! `vfl.toml`). The result is:
//!
//! - **idempotent**: formatting the output again changes nothing;
//! - **safe**: the output is parsed again and must hold the same tokens
//!   (up to case, and the optional words after `end`) and the same comments,
//!   or [`format`] returns an error instead of a broken file.
//!
//! How it works:
//!
//! 1. The tree is flattened into tokens and comments; keyword and identifier
//!    case is applied, and the words after `end` are added or dropped.
//! 2. Line breaks: the ones in the source are kept (blank lines capped at
//!    `max_blank_lines`), and each statement, declaration, clause, `begin`,
//!    `end`, `else` and `elsif` starts a line. Lists (`port (...)`,
//!    `port map (...)`, calls, aggregates) are laid out one element per line
//!    when the config asks for it or when a line is wider than `max_width`.
//! 3. Indentation follows the constructs; a line continuing a statement gets
//!    `continuation_indent`, and a line inside parentheses one `indent` more
//!    than the line that opened them.
//! 4. Alignment pads consecutive lines so that `:`, `<=`, `=>`, `when`,
//!    trailing comments and the like line up (see [`vfl_config::AlignConfig`]).
//!
//! Regions between `-- vfl: off` and `-- vfl: on`, and the statement after
//! `-- vfl: skip`, are left exactly as written.
//!
//! ```
//! use vfl_config::FormatConfig;
//! use vfl_syntax::Standard;
//!
//! let text = "ENTITY e IS PORT(a:IN bit;bb:OUT bit);END;";
//! let out = vfl_fmt::format(text, &FormatConfig::default(), Standard::Vhdl2008).unwrap();
//! assert_eq!(
//!     out,
//!     "entity e is\n  port (\n    a  : in  bit;\n    bb : out bit\n  );\nend;\n"
//! );
//! ```

mod align;
mod elems;
mod escape;
mod kinds;
mod layout;
mod spacing;
mod verify;

use std::fmt;

use vfl_config::{FormatConfig, LineEnding};
use vfl_syntax::{Standard, SyntaxError, parse};

/// Why a file could not be formatted. The file should be left as it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FormatError {
    /// The file has syntax errors; only valid files are formatted.
    Syntax(Vec<SyntaxError>),
    /// The formatter produced output that does not match its input. This is
    /// a bug in the formatter.
    Internal(String),
}

impl fmt::Display for FormatError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FormatError::Syntax(errors) => write!(
                f,
                "{} syntax error{}, not formatted",
                errors.len(),
                if errors.len() == 1 { "" } else { "s" }
            ),
            FormatError::Internal(message) => write!(f, "formatter bug, not formatted: {message}"),
        }
    }
}

impl std::error::Error for FormatError {}

/// Formats a whole VHDL file.
pub fn format(text: &str, config: &FormatConfig, std: Standard) -> Result<String, FormatError> {
    let (bom, body) = match text.strip_prefix('\u{feff}') {
        Some(rest) => ("\u{feff}", rest),
        None => ("", text),
    };
    let crlf = body.contains("\r\n");
    let source = if crlf {
        body.replace("\r\n", "\n")
    } else {
        body.to_owned()
    };
    let parsed = parse(&source, std);
    if !parsed.errors().is_empty() {
        return Err(FormatError::Syntax(parsed.errors().to_vec()));
    }
    let root = parsed.syntax();

    let elems = elems::collect(&root, config);
    let elems = elems::edit_end_clauses(elems, config);
    if elems.is_empty() {
        return Ok(String::new());
    }
    let regions = escape::regions(&elems, source.len());
    let protected = escape::protected(&elems, &regions);
    let an = layout::analyze(&elems, config);
    let mut expanded = layout::initially_expanded(&an);
    let (mut lines, line_of) = loop {
        let nl = layout::breaks(&elems, &an, &expanded, config);
        let (lines, line_of) = layout::lines(&elems, &an, &nl, config);
        if !layout::expand_long_lines(&elems, &an, &lines, &mut expanded, config) {
            break (lines, line_of);
        }
    };
    for line in &mut lines {
        line.verbatim = protected[line.start..line.end].iter().any(|&p| p);
    }
    let pads = align::align(&elems, &an, &mut lines, &line_of, &config.align);
    let (out, positions) = layout::render(&elems, &an, &lines, &pads, config);
    let out = escape::restore_verbatim(&source, out, &elems, &positions, &regions);

    let reparsed = parse(&out, std);
    if let Some(error) = reparsed.errors().first() {
        return Err(FormatError::Internal(format!(
            "the output does not parse: {}",
            error.message
        )));
    }
    let before = verify::fingerprint(&root);
    let after = verify::fingerprint(&reparsed.syntax());
    if before != after {
        return Err(FormatError::Internal(verify::first_difference(
            &before, &after,
        )));
    }

    let use_crlf = match config.line_ending {
        LineEnding::Auto => crlf,
        LineEnding::Lf => false,
        LineEnding::Crlf => true,
    };
    let out = if use_crlf {
        out.replace('\n', "\r\n")
    } else {
        out
    };
    Ok(format!("{bom}{out}"))
}

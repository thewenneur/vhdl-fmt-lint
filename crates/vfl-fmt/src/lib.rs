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
//!    when the config asks for it or when a line is wider than `max_width`;
//!    a line still too wide is broken at the loosest operators of its
//!    expressions. Widths are display widths (CJK characters count two).
//! 3. Indentation follows the constructs; a line continuing a statement gets
//!    `continuation_indent`, and a line inside parentheses one `indent` more
//!    than the line that opened them.
//! 4. Alignment pads consecutive lines so that `:`, `<=`, `=>`, `when`,
//!    trailing comments and the like line up (see [`vfl_config::AlignConfig`]).
//!
//! Regions between `-- vfl: off` and `-- vfl: on`, and the statement after
//! `-- vfl: skip`, are left exactly as written.
//!
//! [`format_ranges`] formats only some lines, for format selection and
//! format on type.
//!
//! ```
//! use vfl_config::FormatConfig;
//! use vfl_syntax::Standard;
//!
//! let text = "ENTITY e IS PORT(a:IN bit;bb:OUT bit);END;";
//! let out = vfl_fmt::format(text, &FormatConfig::default(), Standard::Vhdl2008).unwrap();
//! assert_eq!(
//!     out,
//!     "entity e is\n    port (\n        a  : in  bit;\n        bb : out bit\n    );\nend;\n"
//! );
//! ```

mod align;
mod elems;
mod escape;
mod kinds;
mod lang;
mod layout;
mod names;
mod range;
mod separators;
mod spacing;
mod verify;
mod verilog;
mod vhdl;

use std::fmt;
use std::ops::Range;

use vfl_config::{FormatConfig, LineEnding};
use vfl_syntax::{Standard, SyntaxError, Vhdl};
use vfl_verilog_syntax::Verilog;

use crate::lang::{Lang, Node};

pub use range::{
    Edit, apply_edits, format_range, format_ranges, format_ranges_in, format_ranges_verilog,
    format_ranges_verilog_in, format_typed, format_typed_verilog, minimal_edit, minimal_edits,
};

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
    format_with::<Vhdl>(text, config, std)
}

/// Formats a whole Verilog or SystemVerilog file. `options` are those it
/// is parsed with (macros defined beforehand, included files), which
/// decide how macro uses parse.
///
/// Verilog is case sensitive: `keyword_case` and `identifier_case` do not
/// apply, nor do `end_label` and `end_keyword`.
pub fn format_verilog(
    text: &str,
    config: &FormatConfig,
    options: &vfl_verilog_syntax::Options,
) -> Result<String, FormatError> {
    format_with::<Verilog>(text, config, *options)
}

pub(crate) fn format_with<L: Lang>(
    text: &str,
    config: &FormatConfig,
    std: L::Std<'_>,
) -> Result<String, FormatError> {
    let source = Source::new(text);
    let root = parse_valid::<L>(&source.text, std)?;
    let Some(laid) = lay_out(&source.text, &root, config, std, None)? else {
        return Ok(String::new());
    };
    let use_crlf = match config.line_ending {
        LineEnding::Auto => source.crlf,
        LineEnding::Lf => false,
        LineEnding::Crlf => true,
    };
    let out = if use_crlf {
        laid.out.replace('\n', "\r\n")
    } else {
        laid.out
    };
    Ok(format!("{}{out}", source.bom))
}

/// A file split into its byte order mark and its text with `\n` line ends.
pub(crate) struct Source<'a> {
    pub bom: &'a str,
    pub crlf: bool,
    pub text: String,
}

impl<'a> Source<'a> {
    pub fn new(text: &'a str) -> Self {
        let (bom, body) = match text.strip_prefix('\u{feff}') {
            Some(rest) => ("\u{feff}", rest),
            None => ("", text),
        };
        let crlf = body.contains("\r\n");
        let mut text = if crlf {
            body.replace("\r\n", "\n")
        } else {
            body.to_owned()
        };
        // A lone CR is a line end too. Kept, it can end a token (a `\`
        // continuing a directive onto an empty line) and read as CR LF once
        // a newline follows it in the output.
        if text.contains('\r') {
            text = text.replace('\r', "\n");
        }
        Source { bom, crlf, text }
    }
}

/// The whole file laid out, and how the output relates to the source.
pub(crate) struct Laid<L: Lang> {
    pub elems: Vec<elems::Elem<L>>,
    pub lines: Vec<layout::Line>,
    pub line_of: Vec<usize>,
    /// Where each element landed in the output before the protected regions
    /// were put back.
    pub positions: Vec<(usize, usize)>,
    /// That output.
    pub raw: String,
    /// The lines of each run of aligned anchors.
    pub runs: Vec<align::Run>,
    /// The formatted file, checked. Empty unless asked for.
    pub out: String,
}

/// The syntax tree of `source`, which must have no syntax errors.
pub(crate) fn parse_valid<L: Lang>(source: &str, std: L::Std<'_>) -> Result<Node<L>, FormatError> {
    let (root, errors) = L::parse(source, std);
    if !errors.is_empty() {
        return Err(FormatError::Syntax(errors));
    }
    Ok(root)
}

/// Formats `source` (with `\n` line ends), whose tree is `root`. `None`
/// when there is nothing in it but whitespace.
///
/// Without `window`, the formatted file is put together and checked:
/// parsed again, it must hold the same tokens and comments. With it, only
/// the bytes `window` of the source are laid out, and the output is not
/// put together.
pub(crate) fn lay_out<L: Lang>(
    source: &str,
    root: &Node<L>,
    config: &FormatConfig,
    std: L::Std<'_>,
    window: Option<Range<usize>>,
) -> Result<Option<Laid<L>>, FormatError> {
    let whole = window.is_none();
    let elems = elems::collect(root, config, window);
    let mut elems = L::edit_end_clauses(elems, config);
    if !config.declaration_case.is_empty() {
        L::apply_declaration_case(&mut elems, root, config);
    }
    if elems.is_empty() {
        return Ok(None);
    }
    let regions = escape::regions(&elems, source.len());
    let protected = escape::protected(&elems, &regions);
    separators::place(&mut elems, config, &protected);
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
    layout::indent_before_verbatim(&elems, &an, &mut lines, &line_of, config);
    let (pads, runs) = align::align(&elems, &an, &mut lines, &line_of, &config.align);
    let (raw, positions) = layout::render(&elems, &an, &lines, &pads, config);
    let mut out = String::new();
    if whole {
        out = escape::restore_verbatim(source, raw.clone(), &elems, &positions, &regions);
        let (reparsed, errors) = L::parse(&out, std);
        if let Some(error) = errors.first() {
            return Err(FormatError::Internal(format!(
                "the output does not parse: {}",
                error.message
            )));
        }
        let before = verify::fingerprint(root);
        let after = verify::fingerprint(&reparsed);
        if before != after {
            return Err(FormatError::Internal(verify::first_difference(
                &before, &after,
            )));
        }
    }
    Ok(Some(Laid {
        elems,
        lines,
        line_of,
        positions,
        raw,
        runs,
        out,
    }))
}

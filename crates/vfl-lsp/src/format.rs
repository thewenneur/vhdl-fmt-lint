//! Formatting a document in its language: VHDL, or Verilog and
//! SystemVerilog by the file's extension. Shared by the language server and
//! the editors that link vfl directly.

use std::ops::Range;
use std::path::{Path, PathBuf};

use vfl_config::Config;
use vfl_fmt::{Edit, FormatError};
use vfl_project::{verilog_standard, with_verilog_options};

/// Runs `vhdl`, or `verilog` with the parse options of the Verilog file at
/// `path`. `root` gives the project directory include directories are
/// relative to; it is only asked for Verilog files.
fn in_language<R>(
    path: Option<&Path>,
    config: &Config,
    root: impl FnOnce() -> PathBuf,
    vhdl: impl FnOnce() -> R,
    verilog: impl FnOnce(&vfl_verilog_syntax::Options) -> R,
) -> R {
    match path.and_then(|p| Some((p, verilog_standard(config, p)?))) {
        Some((path, standard)) => with_verilog_options(path, config, &root(), standard, verilog),
        None => vhdl(),
    }
}

/// `text`, the content of the file at `path` (`None` for a buffer that is
/// not saved anywhere, formatted as VHDL), formatted.
pub fn document(
    path: Option<&Path>,
    text: &str,
    config: &Config,
    root: impl FnOnce() -> PathBuf,
) -> Result<String, FormatError> {
    in_language(
        path,
        config,
        root,
        || vfl_fmt::format(text, &config.format, config.standard()),
        |options| vfl_fmt::format_verilog(text, &config.format, options),
    )
}

/// The edits formatting the lines that bytes `range` of `text` touch.
pub fn range(
    path: Option<&Path>,
    text: &str,
    range: Range<usize>,
    config: &Config,
    root: impl FnOnce() -> PathBuf,
) -> Result<Vec<Edit>, FormatError> {
    in_language(
        path,
        config,
        root,
        || vfl_fmt::format_range(text, range.clone(), &config.format, config.standard()),
        |options| {
            vfl_fmt::format_ranges_verilog(
                text,
                std::slice::from_ref(&range),
                &config.format,
                options,
            )
        },
    )
}

/// The edits formatting the line just finished after `ch` was typed at
/// byte `cursor` of `text`.
pub fn typed(
    path: Option<&Path>,
    text: &str,
    cursor: usize,
    ch: char,
    config: &Config,
    root: impl FnOnce() -> PathBuf,
) -> Result<Vec<Edit>, FormatError> {
    in_language(
        path,
        config,
        root,
        || vfl_fmt::format_typed(text, cursor, ch, &config.format, config.standard()),
        |options| vfl_fmt::format_typed_verilog(text, cursor, ch, &config.format, options),
    )
}

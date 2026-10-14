//! Formats every VHDL file under the given paths and checks the formatter's
//! guarantees: no internal error (the output re-parses to the same tokens)
//! and idempotency (formatting the output again changes nothing). With
//! `--verilog`, formats the Verilog and SystemVerilog files instead, each
//! with the standard its extension implies.
//!
//! ```text
//! cargo run --release -p vfl-fmt --example fmt_corpus -- [--std 2008] [--verilog] [--config vfl.toml] [--quiet] [--show] [--ranges] [--exclude SUBSTRING]... PATH...
//! ```
//!
//! Files with syntax errors are skipped. `--show` prints the formatted text.
//! `--ranges` also formats single lines spread over each file and checks that
//! formatting them again changes nothing and that the file still formats to
//! the same text. Exits non-zero on any internal error or non-idempotent file.

use std::ops::Range;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use vfl_config::{Config, FormatConfig};
use vfl_fmt::{Edit, FormatError, apply_edits};
use vfl_syntax::Standard;

/// The language of a file, and how it is parsed.
#[derive(Clone, Copy)]
enum Lang {
    Vhdl(Standard),
    Verilog(vfl_verilog_syntax::Standard),
}

fn format(text: &str, config: &FormatConfig, lang: Lang) -> Result<String, FormatError> {
    match lang {
        Lang::Vhdl(std) => vfl_fmt::format(text, config, std),
        Lang::Verilog(standard) => vfl_fmt::format_verilog(text, config, &options(standard)),
    }
}

fn format_ranges_in(
    text: &str,
    ranges: &[Range<usize>],
    config: &FormatConfig,
    lang: Lang,
    incremental: bool,
) -> Result<Vec<Edit>, FormatError> {
    match lang {
        Lang::Vhdl(std) => vfl_fmt::format_ranges_in(text, ranges, config, std, incremental),
        Lang::Verilog(standard) => {
            vfl_fmt::format_ranges_verilog_in(text, ranges, config, &options(standard), incremental)
        }
    }
}

fn format_range(
    text: &str,
    range: Range<usize>,
    config: &FormatConfig,
    lang: Lang,
) -> Result<Vec<Edit>, FormatError> {
    format_ranges_in(text, &[range], config, lang, true)
}

fn options(standard: vfl_verilog_syntax::Standard) -> vfl_verilog_syntax::Options<'static> {
    vfl_verilog_syntax::Options {
        standard,
        ..Default::default()
    }
}

fn main() {
    let mut std = Standard::Vhdl2008;
    let mut verilog = false;
    let mut config = FormatConfig::default();
    let mut quiet = false;
    let mut show = false;
    let mut ranges = false;
    let mut roots = Vec::new();
    let mut excludes = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--std" => {
                std = args
                    .next()
                    .expect("--std needs a value")
                    .parse()
                    .expect("bad standard")
            }
            "--config" => {
                let path = args.next().expect("--config needs a path");
                config = Config::from_file(Path::new(&path))
                    .expect("bad config")
                    .format;
            }
            "--quiet" => quiet = true,
            "--verilog" => verilog = true,
            "--show" => show = true,
            "--ranges" => ranges = true,
            "--exclude" => excludes.push(args.next().expect("--exclude needs a value")),
            _ => roots.push(PathBuf::from(arg)),
        }
    }

    let mut files = Vec::new();
    for root in &roots {
        collect(root, verilog, &mut files);
    }
    files.sort();
    files.retain(|f| {
        !excludes
            .iter()
            .any(|e| f.to_string_lossy().contains(e.as_str()))
    });

    let mut bytes = 0usize;
    let mut elapsed = Duration::ZERO;
    let mut skipped = 0usize;
    let mut changed = 0usize;
    let mut failures = 0usize;
    for file in &files {
        let raw = std::fs::read(file).expect("read file");
        let text = match String::from_utf8(raw) {
            Ok(text) => text,
            Err(err) => err.into_bytes().iter().map(|&b| b as char).collect(),
        };
        bytes += text.len();
        let lang = match verilog_standard(file) {
            Some(standard) if verilog => Lang::Verilog(standard),
            _ => Lang::Vhdl(std),
        };
        let start = Instant::now();
        let result = format(&text, &config, lang);
        elapsed += start.elapsed();
        let out = match result {
            Ok(out) => out,
            Err(FormatError::Syntax(_)) => {
                skipped += 1;
                continue;
            }
            Err(err) => {
                failures += 1;
                println!("{}: {err}", file.display());
                continue;
            }
        };
        if show {
            print!("{out}");
        }
        if out != text {
            changed += 1;
        }
        if ranges && let Err(message) = check_ranges(&text, &out, &config, lang) {
            failures += 1;
            println!("{}: {message}", file.display());
        }
        match format(&out, &config, lang) {
            Ok(again) if again == out => {}
            Ok(again) => {
                failures += 1;
                println!("{}: not idempotent", file.display());
                if !quiet {
                    print_first_diff(&out, &again);
                }
            }
            Err(err) => {
                failures += 1;
                println!("{}: second pass failed: {err}", file.display());
            }
        }
    }
    let mb = bytes as f64 / 1e6;
    println!(
        "{} files, {:.1} MB, formatted in {:.2?} ({:.1} MB/s); {} changed, {} skipped (syntax errors), {} failures",
        files.len(),
        mb,
        elapsed,
        mb / elapsed.as_secs_f64(),
        changed,
        skipped,
        failures,
    );
    if failures > 0 {
        std::process::exit(1);
    }
}

/// Formats single lines spread over `text` (whose full format is `whole`).
fn check_ranges(text: &str, whole: &str, config: &FormatConfig, lang: Lang) -> Result<(), String> {
    let starts: Vec<usize> = std::iter::once(0)
        .chain(text.match_indices('\n').map(|(i, _)| i + 1))
        .filter(|&i| i < text.len())
        .collect();
    let step = (starts.len() / 8).max(1);
    for (n, &start) in starts.iter().enumerate().step_by(step) {
        let at = |what: &str| format!("line {}: {what}", n + 1);
        let edits =
            format_range(text, start..start, config, lang).map_err(|e| at(&e.to_string()))?;
        let full = format_ranges_in(
            text,
            std::slice::from_ref(&(start..start)),
            config,
            lang,
            false,
        )
        .map_err(|e| at(&e.to_string()))?;
        if full != edits {
            return Err(at(&format!(
                "laying out part of the file differs from laying out all of it: {edits:?} vs {full:?}"
            )));
        }
        let out = apply_edits(text, &edits);
        let mut shift = 0isize;
        for edit in &edits {
            let landed = (edit.range.start as isize + shift) as usize;
            shift += edit.text.len() as isize - edit.range.len() as isize;
            let again = format_range(&out, landed..landed + edit.text.len(), config, lang)
                .map_err(|e| at(&format!("second range pass: {e}")))?;
            if !again.is_empty() {
                return Err(at("range formatting is not idempotent"));
            }
        }
        match format(&out, config, lang) {
            Ok(full) if full == whole => {}
            Ok(full) => {
                print_first_diff(whole, &full);
                return Err(at("the file formats differently after range formatting"));
            }
            Err(e) => return Err(at(&format!("full format after range: {e}"))),
        }
    }
    Ok(())
}

fn print_first_diff(a: &str, b: &str) {
    for (i, (x, y)) in a.lines().zip(b.lines()).enumerate() {
        if x != y {
            println!("  line {}:\n    first:  {x:?}\n    second: {y:?}", i + 1);
            return;
        }
    }
    println!("  line counts differ");
}

fn verilog_standard(path: &Path) -> Option<vfl_verilog_syntax::Standard> {
    path.extension()
        .and_then(|e| e.to_str())
        .and_then(vfl_verilog_syntax::default_standard_for_extension)
}

fn collect(path: &Path, verilog: bool, files: &mut Vec<PathBuf>) {
    // Some Verilog projects hold symbolic link loops.
    if verilog && path.is_symlink() && path.is_dir() {
        return;
    }
    if path.is_dir() {
        let mut entries: Vec<_> = std::fs::read_dir(path)
            .expect("read dir")
            .map(|e| e.expect("dir entry").path())
            .collect();
        entries.sort();
        for entry in entries {
            collect(&entry, verilog, files);
        }
    } else if (verilog && verilog_standard(path).is_some())
        || (!verilog
            && path
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| e.eq_ignore_ascii_case("vhd") || e.eq_ignore_ascii_case("vhdl")))
    {
        files.push(path.to_owned());
    }
}

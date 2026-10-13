//! Formats every VHDL file under the given paths and checks the formatter's
//! guarantees: no internal error (the output re-parses to the same tokens)
//! and idempotency (formatting the output again changes nothing).
//!
//! ```text
//! cargo run --release -p vfl-fmt --example fmt_corpus -- [--std 2008] [--config vfl.toml] [--quiet] [--show] [--exclude SUBSTRING]... PATH...
//! ```
//!
//! Files with syntax errors are skipped. `--show` prints the formatted text.
//! Exits non-zero on any internal error or non-idempotent file.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use vfl_config::{Config, FormatConfig};
use vfl_fmt::{FormatError, format};
use vfl_syntax::Standard;

fn main() {
    let mut std = Standard::Vhdl2008;
    let mut config = FormatConfig::default();
    let mut quiet = false;
    let mut show = false;
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
            "--show" => show = true,
            "--exclude" => excludes.push(args.next().expect("--exclude needs a value")),
            _ => roots.push(PathBuf::from(arg)),
        }
    }

    let mut files = Vec::new();
    for root in &roots {
        collect(root, &mut files);
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
        let start = Instant::now();
        let result = format(&text, &config, std);
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
        match format(&out, &config, std) {
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

fn print_first_diff(a: &str, b: &str) {
    for (i, (x, y)) in a.lines().zip(b.lines()).enumerate() {
        if x != y {
            println!("  line {}:\n    first:  {x:?}\n    second: {y:?}", i + 1);
            return;
        }
    }
    println!("  line counts differ");
}

fn collect(path: &Path, files: &mut Vec<PathBuf>) {
    if path.is_dir() {
        let mut entries: Vec<_> = std::fs::read_dir(path)
            .expect("read dir")
            .map(|e| e.expect("dir entry").path())
            .collect();
        entries.sort();
        for entry in entries {
            collect(&entry, files);
        }
    } else if path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("vhd") || e.eq_ignore_ascii_case("vhdl"))
    {
        files.push(path.to_owned());
    }
}

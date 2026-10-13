//! Parses every VHDL file under the given directories and reports
//! round-trip failures and syntax errors.
//!
//! ```text
//! cargo run --release -p vhone-syntax --example parse_corpus -- [--std 2008] [--quiet] [--trace] [--allow-errors] [--exclude SUBSTRING]... PATH...
//! ```
//!
//! Exits non-zero if any file fails to round-trip, or has errors unless
//! `--allow-errors` is given.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use vhone_syntax::{Standard, parse};

fn main() {
    let mut std = Standard::Vhdl2008;
    let mut quiet = false;
    let mut trace = false;
    let mut allow_errors = false;
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
            "--quiet" => quiet = true,
            "--trace" => trace = true,
            "--allow-errors" => allow_errors = true,
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
    let mut with_errors = 0usize;
    let mut total_errors = 0usize;
    let mut round_trip_failures = 0usize;
    for file in &files {
        let raw = std::fs::read(file).expect("read file");
        // Sources are often Latin-1; decode byte by byte so the text round-trips.
        let text = match String::from_utf8(raw) {
            Ok(text) => text,
            Err(err) => err.into_bytes().iter().map(|&b| b as char).collect(),
        };
        bytes += text.len();
        if trace {
            eprintln!("{}", file.display());
        }
        let start = Instant::now();
        let parsed = parse(&text, std);
        elapsed += start.elapsed();
        if parsed.syntax().to_string() != text {
            round_trip_failures += 1;
            println!("ROUND-TRIP FAILURE {}", file.display());
        }
        if !parsed.errors().is_empty() {
            with_errors += 1;
            total_errors += parsed.errors().len();
            if !quiet {
                let e = &parsed.errors()[0];
                let offset = usize::from(e.range.start());
                let line = text[..offset].matches('\n').count() + 1;
                println!(
                    "{}:{}: {} ({} errors)",
                    file.display(),
                    line,
                    e.message,
                    parsed.errors().len()
                );
            }
        }
    }
    let mb = bytes as f64 / 1e6;
    println!(
        "{} files, {:.1} MB, parsed in {:.2?} ({:.0} MB/s); {} files with errors ({} errors), {} round-trip failures",
        files.len(),
        mb,
        elapsed,
        mb / elapsed.as_secs_f64(),
        with_errors,
        total_errors,
        round_trip_failures,
    );
    if (with_errors > 0 && !allow_errors) || round_trip_failures > 0 {
        std::process::exit(1);
    }
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

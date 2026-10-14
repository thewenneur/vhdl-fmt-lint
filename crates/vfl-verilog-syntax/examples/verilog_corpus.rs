//! Parses every Verilog and SystemVerilog file under the given directories
//! and reports round-trip failures and syntax errors.
//!
//! ```text
//! cargo run --release -p vfl-verilog-syntax --example verilog_corpus -- [--std 1800-2017] [--quiet] [--trace] [--allow-errors] [--include DIR]... [--exclude SUBSTRING]... PATH...
//! ```
//!
//! Without `--std`, `.v` files are parsed as Verilog-2005 and `.sv` files
//! as SystemVerilog-2017. Included files are looked up in the including
//! file's directory and in the `--include` directories.
//!
//! With `--sv-tests`, files are read as tests of the chipsalliance sv-tests
//! suite: the macros in their `:defines:` line are defined, and a file with
//! a `:should_fail_because:` line may have errors.
//!
//! With `--prefixes N`, N prefixes of each file are parsed too, as an
//! editor sees a file being typed: they must round-trip and not panic.
//!
//! Exits non-zero if any file fails to round-trip, or has errors unless
//! `--allow-errors` is given.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use vfl_verilog_syntax::{Options, Standard, default_standard_for_extension, parse_with};

fn main() {
    let mut std: Option<Standard> = None;
    let mut include_dirs: Vec<PathBuf> = Vec::new();
    let mut quiet = false;
    let mut trace = false;
    let mut allow_errors = false;
    let mut sv_tests = false;
    let mut prefixes = 0usize;
    let mut roots = Vec::new();
    let mut excludes = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--std" => {
                std = Some(
                    args.next()
                        .expect("--std needs a value")
                        .parse()
                        .expect("bad standard"),
                )
            }
            "--quiet" => quiet = true,
            "--trace" => trace = true,
            "--allow-errors" => allow_errors = true,
            "--sv-tests" => sv_tests = true,
            "--prefixes" => {
                prefixes = args
                    .next()
                    .expect("--prefixes needs a value")
                    .parse()
                    .expect("bad count")
            }
            "--include" => {
                include_dirs.push(PathBuf::from(args.next().expect("--include needs a value")))
            }
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
        let standard = std.unwrap_or_else(|| {
            file.extension()
                .and_then(|e| e.to_str())
                .and_then(default_standard_for_extension)
                .unwrap_or_default()
        });
        let dir = file.parent().map(Path::to_path_buf).unwrap_or_default();
        let read = |name: &str| -> Option<String> {
            std::iter::once(&dir)
                .chain(&include_dirs)
                .map(|d| d.join(name))
                .find(|p| p.is_file())
                .and_then(|p| std::fs::read(p).ok())
                .map(|b| String::from_utf8_lossy(&b).into_owned())
        };
        let mut defines = Vec::new();
        let mut may_fail = false;
        if sv_tests {
            for line in text.lines() {
                if let Some(rest) = line.strip_prefix(":defines:") {
                    defines.extend(rest.split_whitespace().map(str::to_owned));
                }
                may_fail |= line.starts_with(":should_fail_because:");
            }
        }
        let options = Options {
            standard,
            defines: &defines,
            include: Some(&read),
        };
        let parsed = parse_with(&text, &options);
        elapsed += start.elapsed();
        for i in 1..=prefixes {
            let mut cut = text.len() * i / (prefixes + 1);
            while !text.is_char_boundary(cut) {
                cut += 1;
            }
            let prefix = &text[..cut];
            if parse_with(prefix, &options).syntax().to_string() != prefix {
                round_trip_failures += 1;
                println!(
                    "ROUND-TRIP FAILURE {} (prefix of {cut} bytes)",
                    file.display()
                );
            }
        }
        if parsed.syntax().to_string() != text {
            round_trip_failures += 1;
            println!("ROUND-TRIP FAILURE {}", file.display());
        }
        if !parsed.errors().is_empty() && !may_fail {
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
        .is_some_and(|e| default_standard_for_extension(e).is_some())
    {
        files.push(path.to_owned());
    }
}

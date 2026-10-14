//! Snapshot tests for the parser, plus robustness checks.
//!
//! - `data/ok/*` must parse without errors (`.v` files as Verilog-2005,
//!   `.sv` files as SystemVerilog-2017).
//! - `data/err/*` are broken on purpose; their trees and errors are snapshotted.
//!
//! Every file must round-trip, and so must every prefix of it.

use std::path::Path;

use vfl_verilog_syntax::{Parse, Standard, default_standard_for_extension, parse};

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap()
}

fn render(parse: &Parse) -> String {
    let mut out = parse.debug_tree();
    if !parse.errors().is_empty() {
        out.push_str("\nerrors:\n");
        for error in parse.errors() {
            out.push_str(&format!("  {error}\n"));
        }
    }
    out
}

fn standard_for(path: &Path) -> Standard {
    path.extension()
        .and_then(|e| e.to_str())
        .and_then(default_standard_for_extension)
        .unwrap()
}

fn check_round_trip(text: &str, std: Standard) -> Parse {
    let parsed = parse(text, std);
    assert_eq!(
        parsed.syntax().to_string(),
        text,
        "tree must reproduce the input"
    );
    parsed
}

#[test]
fn valid_files() {
    insta::glob!("data/ok/*", |path| {
        let text = read(path);
        let parsed = check_round_trip(&text, standard_for(path));
        assert!(
            parsed.errors().is_empty(),
            "{}: unexpected errors:\n{}",
            path.display(),
            render(&parsed)
        );
        insta::assert_snapshot!(render(&parsed));
    });
}

#[test]
fn invalid_files() {
    insta::glob!("data/err/*", |path| {
        let text = read(path);
        let parsed = check_round_trip(&text, standard_for(path));
        assert!(
            !parsed.errors().is_empty(),
            "{}: expected errors",
            path.display()
        );
        insta::assert_snapshot!(render(&parsed));
    });
}

#[test]
fn every_prefix_round_trips() {
    for dir in ["tests/data/ok", "tests/data/err"] {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            let text = read(&path);
            let std = standard_for(&path);
            for (end, _) in text.char_indices() {
                check_round_trip(&text[..end], std);
            }
        }
    }
}

#[test]
fn verilog_2005_does_not_reserve_systemverilog_keywords() {
    let text = "module m; wire logic; reg bit; endmodule\n";
    assert!(parse(text, Standard::Verilog2005).errors().is_empty());
    assert!(!parse(text, Standard::Sv2017).errors().is_empty());
}

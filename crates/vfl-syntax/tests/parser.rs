//! Snapshot tests for the parser, plus robustness checks.
//!
//! - `data/ok/*.vhd` must parse without errors (as VHDL-2019 if the file name
//!   says so, else as VHDL-2008).
//! - `data/err/*.vhd` are broken on purpose; their trees and errors are snapshotted.
//! - `data/std93/*.vhd` are parsed as VHDL-93 to check standard gating.
//!
//! Every file must round-trip, and so must every prefix of it.

use std::path::Path;

use vfl_syntax::{Parse, Standard, parse};

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
    if path.to_string_lossy().contains("2019") {
        Standard::Vhdl2019
    } else {
        Standard::Vhdl2008
    }
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
    insta::glob!("data/ok/*.vhd", |path| {
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
    insta::glob!("data/err/*.vhd", |path| {
        let text = read(path);
        let parsed = check_round_trip(&text, Standard::Vhdl2008);
        assert!(
            !parsed.errors().is_empty(),
            "{}: expected errors",
            path.display()
        );
        insta::assert_snapshot!(render(&parsed));
    });
}

#[test]
fn standard_gating() {
    insta::glob!("data/std93/*.vhd", |path| {
        let text = read(path);
        let parsed = check_round_trip(&text, Standard::Vhdl1993);
        let errors: Vec<String> = parsed.errors().iter().map(ToString::to_string).collect();
        insta::assert_snapshot!(errors.join("\n"));
    });
}

/// Every prefix of every test file parses without panicking and round-trips,
/// which exercises error recovery at every possible cut point.
#[test]
fn every_prefix_round_trips() {
    insta::glob!("data/*/*.vhd", |path| {
        let text = read(path);
        let std = standard_for(path);
        for (end, _) in text.char_indices() {
            check_round_trip(&text[..end], std);
        }
    });
}

/// Deleting any single token still yields a tree that reproduces the input.
#[test]
fn every_token_deletion_round_trips() {
    insta::glob!("data/ok/*.vhd", |path| {
        let text = read(path);
        let std = standard_for(path);
        let (tokens, _) = vfl_syntax::lex(&text, std);
        let mut offset = 0usize;
        for token in tokens {
            let len = u32::from(token.len) as usize;
            if !token.kind.is_trivia() {
                let mutated = format!("{}{}", &text[..offset], &text[offset + len..]);
                check_round_trip(&mutated, std);
            }
            offset += len;
        }
    });
}

#[test]
fn deeply_nested_input_does_not_overflow() {
    let text = format!(
        "constant c : integer := {}1{};",
        "(".repeat(100_000),
        ")".repeat(100_000)
    );
    let parsed = check_round_trip(&text, Standard::Vhdl2008);
    assert!(!parsed.errors().is_empty());
}

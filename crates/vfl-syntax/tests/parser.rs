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

/// The green-tree walks find what rowan's own traversals find, on every
/// fixture, broken ones included.
#[test]
fn green_walks_match_rowan_traversals() {
    use vfl_syntax::SyntaxKind;
    use vfl_syntax::green_walk::{descendants_where, for_each_token};

    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data");
    for sub in ["ok", "err"] {
        for entry in std::fs::read_dir(dir.join(sub)).unwrap() {
            let path = entry.unwrap().path();
            let root = parse(&read(&path), standard_for(&path)).syntax();

            let mut tokens = Vec::new();
            for_each_token(&root, |kind, range, text| {
                tokens.push((kind, range, text.to_owned()))
            });
            let expected: Vec<_> = root
                .descendants_with_tokens()
                .filter_map(|e| e.into_token())
                .map(|t| (t.kind(), t.text_range(), t.text().to_owned()))
                .collect();
            assert_eq!(tokens, expected, "{}", path.display());

            let mut kinds: Vec<SyntaxKind> = root.descendants().map(|n| n.kind()).collect();
            kinds.sort_unstable_by_key(|k| *k as u16);
            kinds.dedup();
            // Each kind alone, every other kind, and all kinds at once.
            let mut sets: Vec<Vec<SyntaxKind>> = kinds.iter().map(|&k| vec![k]).collect();
            sets.push(kinds.iter().copied().step_by(2).collect());
            sets.push(kinds.clone());
            for set in sets {
                let found = descendants_where(&root, |k| set.contains(&k));
                let expected: Vec<_> = root
                    .descendants()
                    .filter(|n| set.contains(&n.kind()))
                    .collect();
                assert_eq!(found, expected, "{} {set:?}", path.display());
                // Under an inner node too.
                if let Some(inner) = root.descendants().nth(3) {
                    let found = descendants_where(&inner, |k| set.contains(&k));
                    let expected: Vec<_> = inner
                        .descendants()
                        .filter(|n| set.contains(&n.kind()))
                        .collect();
                    assert_eq!(found, expected, "{} {set:?}", path.display());
                }
            }
        }
    }
}

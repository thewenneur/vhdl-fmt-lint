//! Fixture values for selected object declarations.

use std::path::PathBuf;
use std::sync::Arc;

use vfl_sema::{Analysis, FileInput};
use vfl_syntax::{Standard, TextRange, TextSize, parse};

/// `text` with fixture values for the declarations `selected` touches.
fn fixtures(text: &str, selected: &str) -> String {
    let parsed = parse(text, Standard::Vhdl2008);
    let analysis = Analysis::new(vec![FileInput {
        path: PathBuf::from("a.vhd"),
        text: Arc::from(text),
        green: parsed.green().clone(),
        libraries: vec!["work".into()],
    }]);
    let file = analysis
        .user_files()
        .iter()
        .position(|f| f.path == *"a.vhd")
        .unwrap() as u32;
    let start = text.find(selected).unwrap();
    let range = TextRange::new(
        TextSize::from(start as u32),
        TextSize::from((start + selected.len()) as u32),
    );
    let mut edits = analysis.fixture_edits(file, range);
    edits.sort_by_key(|e| std::cmp::Reverse(e.range.start()));
    let mut out = text.to_owned();
    for e in edits {
        out.replace_range(std::ops::Range::<usize>::from(e.range), &e.text);
    }
    out
}

const PACKAGE: &str = "\
library ieee;
use ieee.std_logic_1164.all;
use ieee.numeric_std.all;
entity e is
  generic (W : natural := 12);
end entity;
architecture a of e is
  type state_t is (IDLE, RUN, DONE);
  type rec_t is record
    a : std_logic_vector(7 downto 0);
    s : state_t;
    n : integer range 0 to 9;
  end record;
  type mem_t is array (0 to 3) of unsigned(3 downto 0);
  subtype byte is std_logic_vector(7 downto 0);
  constant C_A : std_logic_vector(W - 1 downto 0) := x\"000\";
  constant C_R : rec_t;
  constant C_M : mem_t;
  constant C_B : byte;
  constant C_I : integer range -5 to 5;
  constant C_N : natural;
  constant C_S : string(1 to 4);
  constant C_T : time;
  constant C_L : std_logic;
  constant C_V : std_logic_vector(0 to 2);
  signal s_state : state_t;
  constant C_U : std_logic_vector;
begin
end architecture;
";

#[test]
fn fills_each_selected_declaration() {
    let out = fixtures(
        PACKAGE,
        &PACKAGE[PACKAGE.find("  constant C_A").unwrap()..PACKAGE.find("begin").unwrap()],
    );
    check(&out);
}

#[test]
fn caret_picks_the_declaration_around_it() {
    let out = fixtures(PACKAGE, "C_B");
    let changed: Vec<&str> = out
        .lines()
        .zip(PACKAGE.lines())
        .filter(|(a, b)| a != b)
        .map(|(a, _)| a)
        .collect();
    assert_eq!(changed.len(), 1);
    assert!(
        changed[0].starts_with("  constant C_B : byte := x\""),
        "{changed:?}"
    );
    // The same name gives the same value.
    assert_eq!(out, fixtures(PACKAGE, "C_B"));
}

fn check(out: &str) {
    let decls: Vec<&str> = out
        .lines()
        .filter(|l| l.contains("constant C_") || l.contains("signal s_"))
        .collect();
    type Shape = (&'static str, fn(&str) -> bool);
    let expected_shapes: &[Shape] = &[
        ("C_A", |v| v.starts_with("x\"") && v.len() == 6), // 12 bits at W's default
        ("C_R", |v| {
            v.starts_with("(a => x\"") && v.contains(", s => ") && v.contains(", n => ")
        }),
        ("C_M", |v| {
            v.starts_with("(x\"") && v.matches(", ").count() == 3
        }),
        ("C_B", |v| v.starts_with("x\"") && v.len() == 5),
        ("C_I", |v| (-5..=5).contains(&v.parse::<i64>().unwrap())),
        ("C_N", |v| v.parse::<i64>().unwrap() >= 0),
        ("C_S", |v| v.len() == 6 && v.starts_with('"')),
        ("C_T", |v| v.ends_with(" ns")),
        ("C_L", |v| v == "'0'" || v == "'1'"),
        ("C_V", |v| v.len() == 5 && v.starts_with('"')),
        ("s_state", |v| ["IDLE", "RUN", "DONE"].contains(&v)),
    ];
    for (name, ok) in expected_shapes {
        let line = decls
            .iter()
            .find(|l| l.contains(&format!(" {name} ")))
            .unwrap();
        let value = line
            .split_once(":= ")
            .unwrap_or_else(|| panic!("no value: {line}"))
            .1
            .trim_end_matches(';');
        assert!(ok(value), "{line}");
    }
    // Without static bounds, the declaration is left alone.
    assert!(
        decls
            .iter()
            .any(|l| l.trim() == "constant C_U : std_logic_vector;")
    );
}

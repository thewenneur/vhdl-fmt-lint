//! The VHDL rules of `vfl-lint` on small files: what each reports, what
//! it does not, its message and its fix.

use std::path::PathBuf;
use std::sync::Arc;

use vfl_config::Config;
use vfl_lint::{Diagnostic, FileSettings, apply_fixes, lint};
use vfl_sema::{Analysis, FileInput};
use vfl_syntax::{Standard, parse};

/// The findings of `files` (name, text) in `work`, with `config` (a
/// `vfl.toml`), as `file:line: rule: message`, and the diagnostics.
fn run(files: &[(&str, &str)], config: &str) -> (Vec<String>, Vec<Diagnostic>) {
    let config = Config::from_toml_str(config).unwrap();
    let parses: Vec<_> = files
        .iter()
        .map(|(_, text)| parse(text, Standard::Vhdl2008))
        .collect();
    let inputs = files
        .iter()
        .zip(&parses)
        .map(|((name, text), parse)| FileInput {
            path: PathBuf::from(name),
            text: Arc::from(*text),
            green: parse.green().clone(),
            libraries: vec!["work".into()],
        })
        .collect();
    let analysis = Analysis::new(inputs);
    let settings: Vec<FileSettings> = files
        .iter()
        .zip(&parses)
        .map(|((name, _), parse)| FileSettings {
            file: analysis.file_id(&PathBuf::from(name)).unwrap(),
            syntax_errors: parse.errors(),
            config: &config,
            verilog: None,
            root: None,
            library_conflict: None,
        })
        .collect();
    let result = lint(&analysis, &settings);
    assert!(
        result.config_errors.is_empty(),
        "{:?}",
        result.config_errors
    );
    let lines = result
        .diagnostics
        .iter()
        .map(|d| {
            let file = &analysis.files()[d.file as usize];
            let at = file.line_index.line_col(&file.text, d.range.start());
            format!(
                "{}:{}: {}: {}",
                file.path.display(),
                at.line + 1,
                d.rule,
                d.message
            )
        })
        .collect();
    (lines, result.diagnostics)
}

/// The findings of one file, `test.vhd`, with only `rules` on.
fn findings(text: &str, rules: &[&str]) -> Vec<String> {
    run(&[("test.vhd", text)], &only(rules)).0
}

/// A config with the `none` preset and `rules` at `warn`.
fn only(rules: &[&str]) -> String {
    let mut config = String::from("[lint]\npreset = \"none\"\n[lint.rules]\n");
    for rule in rules {
        config.push_str(&format!("\"{rule}\" = \"warn\"\n"));
    }
    config
}

/// `text` with the fix of its only finding of `rule` applied.
#[track_caller]
fn fixed(text: &str, rule: &str) -> String {
    let (lines, diagnostics) = run(&[("test.vhd", text)], &only(&[rule]));
    let [diagnostic] = diagnostics.as_slice() else {
        panic!("expected one finding: {lines:#?}");
    };
    apply_fixes(text, diagnostic.fix.as_ref()).0
}

const ENTITY: &str = "entity test is
  port (clk : in bit; d : in bit; q : out bit);
end entity;
";

#[test]
fn syntax_errors_are_reported_in_every_preset() {
    let text = "entity test is\n  port (clk : in bit\nend entity;\n";
    let (lines, _) = run(&[("test.vhd", text)], "[lint]\npreset = \"none\"\n");
    assert_eq!(lines.len(), 1, "{lines:#?}");
    assert!(
        lines[0].starts_with("test.vhd:2: syntax/error:"),
        "{lines:#?}"
    );
    let (lines, _) = run(
        &[("test.vhd", text)],
        "[lint.rules]\n\"syntax/error\" = \"off\"\n",
    );
    assert!(
        lines.iter().all(|l| !l.contains("syntax/error")),
        "{lines:#?}"
    );
}

#[test]
fn line_length() {
    let max = |max: usize| {
        format!(
            "[lint]\npreset = \"none\"\n[lint.rules]\n\"style/line-length\" = {{ max = {max} }}\n"
        )
    };
    let long = format!("{ENTITY}-- {}\n", "x".repeat(98));
    let (lines, _) = run(&[("test.vhd", &long)], &max(100));
    assert_eq!(
        lines,
        ["test.vhd:4: style/line-length: line is 101 characters long (max 100)"]
    );
    // Counted in characters, not bytes.
    let at_limit = format!("{ENTITY}-- {}\n", "é".repeat(97));
    assert!(run(&[("test.vhd", &at_limit)], &max(100)).0.is_empty());
    // The default limit is 120.
    assert!(findings(&long, &["style/line-length"]).is_empty());
    let longer = format!("{ENTITY}-- {}\n", "x".repeat(118));
    assert_eq!(
        findings(&longer, &["style/line-length"]),
        ["test.vhd:4: style/line-length: line is 121 characters long (max 120)"]
    );
}

#[test]
fn trailing_whitespace_and_tabs() {
    let text = "entity test is \r\n\tport (clk : in bit);\r\n  \t-- mixed\nend entity;\n";
    assert_eq!(
        findings(text, &["style/trailing-whitespace", "style/tabs"]),
        [
            "test.vhd:1: style/trailing-whitespace: trailing whitespace",
            "test.vhd:2: style/tabs: line is indented with tabs",
            "test.vhd:3: style/tabs: line is indented with tabs",
        ]
    );
    assert_eq!(
        fixed(text, "style/trailing-whitespace"),
        text.replace("is \r\n", "is\r\n")
    );
    let tabs = "entity test is\n  \tport (clk : in bit);\nend entity;\n";
    // Two spaces then a tab reach the next multiple of `indent` (4).
    assert_eq!(
        fixed(tabs, "style/tabs"),
        tabs.replace("  \tport", "    port")
    );
}

#[test]
fn case_others() {
    let text = format!(
        "{ENTITY}architecture rtl of test is
  type state_t is (a, b);
  signal s : state_t;
begin
  process (s) is
  begin
    case s is
      when a => null;
      when b => null;
    end case;
    case s is
      when a => null;
      when others => null;
    end case;
  end process;
end architecture;
"
    );
    assert_eq!(
        findings(&text, &["style/case-others"]),
        ["test.vhd:10: style/case-others: `case` without `when others`"]
    );
    assert_eq!(
        fixed(&text, "style/case-others"),
        text.replacen(
            "      when b => null;\n",
            "      when b => null;\n      when others => null;\n",
            1
        )
    );
}

#[test]
fn buffer_ports() {
    let text = "entity test is
  port (a : in bit; q : buffer bit; r : out bit);
end entity;
";
    assert_eq!(
        findings(text, &["style/no-buffer-ports"]),
        [
            "test.vhd:2: style/no-buffer-ports: port of mode `buffer`; use `out` (readable since VHDL-2008) or an internal signal"
        ]
    );
    assert_eq!(
        fixed(text, "style/no-buffer-ports"),
        text.replace("buffer", "out")
    );
}

#[test]
fn non_standard_arithmetic_packages() {
    let text = "library ieee;
use ieee.std_logic_1164.all;
use ieee.std_logic_unsigned.all;
use IEEE.STD_LOGIC_ARITH.all;
use ieee.numeric_std.all;
entity test is
end entity;
";
    assert_eq!(
        findings(text, &["style/no-std-logic-arith"]),
        [
            "test.vhd:3: style/no-std-logic-arith: `std_logic_unsigned` is not a standard package; use `ieee.numeric_std`",
            "test.vhd:4: style/no-std-logic-arith: `std_logic_arith` is not a standard package; use `ieee.numeric_std`",
        ]
    );
    let one = "library ieee;\nuse ieee.std_logic_unsigned.all;\nentity test is\nend entity;\n";
    assert_eq!(
        fixed(one, "style/no-std-logic-arith"),
        one.replace("std_logic_unsigned", "numeric_std_unsigned")
    );
}

#[test]
fn process_labels() {
    let text = format!(
        "{ENTITY}architecture rtl of test is
begin
  process (clk) is
  begin
  end process;
  p_named : process (clk) is
  begin
  end process;
end architecture;
"
    );
    assert_eq!(
        findings(&text, &["style/process-label"]),
        ["test.vhd:6: style/process-label: process without a label"]
    );
}

#[test]
fn positional_association() {
    let files = [
        (
            "leaf.vhd",
            "entity leaf is\n  generic (W : natural := 1);\n  port (a : in bit; y : out bit);\nend entity;\narchitecture rtl of leaf is\nbegin\n  y <= a;\nend architecture;\n",
        ),
        (
            "test.vhd",
            "entity test is\n  port (a : in bit; y : out bit; z : out bit);\nend entity;
architecture rtl of test is
begin
  u_pos : entity work.leaf generic map (2) port map (a, y);
  u_named : entity work.leaf port map (a => a, y => z);
end architecture;
",
        ),
    ];
    let (lines, diagnostics) = run(&files, &only(&["style/positional-association"]));
    assert_eq!(
        lines,
        [
            "test.vhd:6: style/positional-association: generic map associates by position; name the formals",
            "test.vhd:6: style/positional-association: port map associates by position; name the formals",
        ]
    );
    let port = diagnostics[1]
        .fix
        .as_ref()
        .expect("a fix naming the formals");
    assert_eq!(
        apply_fixes(files[1].1, [port]).0,
        files[1]
            .1
            .replace("port map (a, y)", "port map (a => a, y => y)")
    );
}

#[test]
fn clock_edges() {
    let text = format!(
        "{ENTITY}architecture rtl of test is
begin
  process (clk) is
  begin
    if clk'event and clk = '1' then
      q <= d;
    end if;
    if (CLK = '0') and Clk'EVENT then
      q <= d;
    end if;
    if clk'event and d = '1' then
      q <= d;
    end if;
    if rising_edge(clk) then
      q <= d;
    end if;
  end process;
end architecture;
"
    );
    assert_eq!(
        findings(&text, &["style/clock-edge"]),
        [
            "test.vhd:8: style/clock-edge: use `rising_edge(clk)` instead of testing `'event`",
            "test.vhd:11: style/clock-edge: use `falling_edge(Clk)` instead of testing `'event`",
        ]
    );
    let one = format!(
        "{ENTITY}architecture rtl of test is
begin
  process (clk) is
  begin
    if clk'event and clk = '1' then
      q <= d;
    end if;
  end process;
end architecture;
"
    );
    assert_eq!(
        fixed(&one, "style/clock-edge"),
        one.replace("clk'event and clk = '1'", "rising_edge(clk)")
    );
}

#[test]
fn file_names() {
    let rules = only(&["style/file-name"]);
    let files = [
        ("test.vhd", ENTITY),
        // Case is ignored, and any primary unit may match.
        (
            "Pkg.vhdl",
            "package helpers is\nend package;\npackage pkg is\nend package;\n",
        ),
        ("wrong.vhdl", "package right is\nend package;\n"),
        // Secondary units only: nothing to match.
        ("body.vhd", "package body right is\nend package body;\n"),
    ];
    let (lines, _) = run(&files, &rules);
    assert_eq!(
        lines,
        [
            "wrong.vhdl:1: style/file-name: file name does not match the unit it declares; expected `right.vhdl`"
        ]
    );
}

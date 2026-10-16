//! Snapshot tests: each `tests/data/*.vhd` holds several files separated by
//! `-- file: path` lines. Libraries are inferred as in a real project, and
//! the snapshot lists each file's libraries and every diagnostic.

use std::fmt::Write;
use std::path::PathBuf;
use std::sync::Arc;

use vfl_project::{infer_libraries, summarize};
use vfl_sema::{Analysis, CodeChecks, DriverChecks, FileInput, ResolvedDrivers};
use vfl_syntax::{Standard, parse};

fn split(text: &str) -> Vec<(String, String)> {
    let mut files: Vec<(String, String)> = Vec::new();
    for line in text.lines() {
        if let Some(name) = line.strip_prefix("-- file: ") {
            files.push((name.trim().to_owned(), String::new()));
        } else if let Some((_, body)) = files.last_mut() {
            body.push_str(line);
            body.push('\n');
        }
    }
    files
}

/// Every on-demand check of the code, with every option.
const ALL_CODE_CHECKS: CodeChecks = CodeChecks {
    index_range: true,
    range_generics: true,
    value_range: true,
    unreachable_code: true,
    constant_condition: true,
    condition_constants: true,
    unused_subprogram: true,
    public_subprograms: true,
    comparison_width: true,
    synthesis_delay: true,
    synthesis_file_io: true,
    synthesis_loop: true,
    synthesis_initial_value: true,
    synthesis_real_port: true,
    identifier_case: true,
    case_libraries: false,
};

pub fn analyze(text: &str) -> String {
    let files = split(text);
    let parses: Vec<_> = files
        .iter()
        .map(|(_, body)| parse(body, Standard::Vhdl2008))
        .collect();
    let paths: Vec<PathBuf> = files.iter().map(|(p, _)| PathBuf::from(p)).collect();
    let summaries: Vec<_> = parses.iter().map(|p| summarize(&p.syntax())).collect();
    let inference = infer_libraries(&paths, &summaries, &vec![None; files.len()], &[]);
    let inputs: Vec<FileInput> = files
        .iter()
        .zip(&parses)
        .zip(&inference.libraries)
        .map(|(((path, body), parse), libs)| FileInput {
            path: PathBuf::from(path),
            text: Arc::from(body.as_str()),
            green: parse.green().clone(),
            libraries: libs.iter().map(|(l, _)| l.clone()).collect(),
        })
        .collect();
    let analysis = Analysis::new(inputs);
    let mut out = String::new();
    for file in analysis.user_files() {
        writeln!(
            out,
            "{}: {}",
            file.path.display(),
            file.libraries.join(", ")
        )
        .unwrap();
    }
    // On demand: with clocked processes, and the driver checks.
    let branches: Vec<_> = (0..analysis.user_files().len() as u32)
        .flat_map(|file| {
            let mut found = analysis.missing_branch_assignments(file, true).diagnostics;
            let checks = DriverChecks {
                multiple: Some(ResolvedDrivers::Rtl),
                loops: true,
            };
            found.extend(analysis.driver_checks(file, checks));
            found.extend(analysis.code_checks(file, &ALL_CODE_CHECKS));
            found
        })
        .collect();
    for d in analysis.diagnostics().iter().chain(&branches) {
        let file = &analysis.files()[d.file as usize];
        let at = file.line_index.line_col(&file.text, d.range.start());
        write!(
            out,
            "{}:{}:{}: {}: {}",
            file.path.display(),
            at.line + 1,
            at.col + 1,
            d.code,
            d.message
        )
        .unwrap();
        if let Some(fix) = &d.fix {
            write!(out, " [fix: {}]", fix.title).unwrap();
        }
        out.push('\n');
    }
    out
}

#[test]
fn snapshots() {
    insta::glob!("data/*.vhd", |path| {
        let text = std::fs::read_to_string(path).unwrap();
        insta::assert_snapshot!(analyze(&text));
    });
}

#[test]
fn bundled_libraries_have_no_errors() {
    let analysis = Analysis::new(Vec::new());
    assert!(analysis.diagnostics().is_empty());
    assert!(analysis.units().len() > 20);
}

/// Found by fuzzing: an unterminated string ending in a multi-byte
/// character panicked the type checker.
#[test]
fn unterminated_string_ending_in_a_multi_byte_character() {
    let text = "package n constant:=\"qê";
    for std in [Standard::Vhdl1993, Standard::Vhdl2008, Standard::Vhdl2019] {
        let input = FileInput {
            path: PathBuf::from("p.vhd"),
            text: Arc::from(text),
            green: parse(text, std).green().clone(),
            libraries: vec!["work".into()],
        };
        Analysis::new(vec![input]);
    }
}

fn input(path: &str, library: &str, text: &str) -> FileInput {
    FileInput {
        path: PathBuf::from(path),
        text: Arc::from(text),
        green: parse(text, Standard::Vhdl2008).green().clone(),
        libraries: vec![library.into()],
    }
}

#[test]
fn vendor_stubs_check_cleanly() {
    for (library, name, text) in vfl_sema::VENDOR {
        let analysis = Analysis::new(vec![input(name, library, text)]);
        assert!(
            analysis.diagnostics().is_empty(),
            "{library}/{name}: {:?}",
            analysis.diagnostics()
        );
    }
}

#[test]
fn vendor_stubs_resolve_primitives() {
    let text = "\
library ieee;
use ieee.std_logic_1164.all;
library unisim;
use unisim.vcomponents.all;
library xpm;

entity top is
  port (clk_in, d : in std_logic; v : in std_logic_vector(3 downto 0); q : out std_logic);
end entity;

architecture rtl of top is
  signal clk : std_logic;
begin
  u0 : BUFG port map (I => clk_in, O => clk);
  u1 : FDRE generic map (INIT => '0') port map (C => clk, CE => '1', R => '0', D => d, Q => q);
  -- Not in the stubs: not reported.
  u2 : BUFGCE_DIV generic map (BUFGCE_DIVIDE => 2) port map (I => clk_in, O => open);
  u3 : entity unisim.MMCME4_ADV port map (CLKIN1 => clk_in);
  u4 : IBUFDS generic map (DIFF_TERM => FALSE, NEW_GENERIC => 1) port map (I => d, IB => d, O => open);
  -- Entities too.
  u5 : entity unisim.IBUF port map (I => d, O => open);
  u6 : entity xpm.xpm_cdc_single generic map (DEST_SYNC_FF => 2)
    port map (src_clk => clk, src_in => d, dest_clk => clk, dest_out => open);
  -- What they declare is checked.
  u7 : BUFG port map (I => v, O => open);
end architecture;
";
    let messages = |analysis: &Analysis| -> Vec<String> {
        analysis
            .diagnostics()
            .iter()
            .map(|d| format!("{}: {}", d.code, d.message))
            .collect()
    };
    let analysis = Analysis::with_stubs(
        vec![input("top.vhd", "work", text)],
        Vec::new(),
        &["unisim", "xpm"],
    );
    assert_eq!(
        messages(&analysis),
        ["sema/type-mismatch: expected `STD_ULOGIC`, found `STD_LOGIC_VECTOR`"]
    );
    let bufg = analysis
        .units()
        .iter()
        .filter(|u| u.library == "unisim" && u.name == "bufg")
        .count();
    assert_eq!(bufg, 1, "the BUFG entity");

    // Without them, the libraries are unknown and nothing is checked.
    let analysis = Analysis::new(vec![input("top.vhd", "work", text)]);
    assert_eq!(
        messages(&analysis),
        [
            "sema/unknown-library: library `unisim` is not part of the project; names from it are not checked",
            "sema/unknown-library: library `xpm` is not part of the project; names from it are not checked",
        ]
    );
}

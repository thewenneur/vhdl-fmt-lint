//! `Analysis::update` must give the same result as `Analysis::new` on the
//! edited project, while analyzing only what an edit can affect.

use std::path::PathBuf;
use std::sync::Arc;

use vfl_project::{infer_libraries, summarize};
use vfl_sema::{Analysis, FileInput};
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

fn inputs(files: &[(String, String)]) -> Vec<FileInput> {
    let parses: Vec<_> = files
        .iter()
        .map(|(_, body)| parse(body, Standard::Vhdl2008))
        .collect();
    let paths: Vec<PathBuf> = files.iter().map(|(p, _)| PathBuf::from(p)).collect();
    let summaries: Vec<_> = parses.iter().map(|p| summarize(&p.syntax())).collect();
    let inference = infer_libraries(&paths, &summaries, &vec![None; files.len()], &[]);
    files
        .iter()
        .zip(&parses)
        .zip(&inference.libraries)
        .map(|(((path, body), parse), libs)| FileInput {
            path: PathBuf::from(path),
            text: Arc::from(body.as_str()),
            green: parse.green().clone(),
            libraries: libs.iter().map(|(l, _)| l.clone()).collect(),
        })
        .collect()
}

/// Re-parses only the edited file, as the language server does.
fn edited(before: &[FileInput], files: &[(String, String)], file: usize) -> Vec<FileInput> {
    let mut after = inputs(files);
    for (i, input) in after.iter_mut().enumerate() {
        if i != file {
            *input = before[i].clone();
        }
    }
    after
}

/// Applies `edit` to file `file` and checks the incremental result.
/// Returns how many units were analyzed again.
fn check(fixture: &str, file: &str, edit: impl Fn(&str) -> String) -> usize {
    let mut files = split(fixture);
    let before = inputs(&files);
    let previous = Analysis::new(Vec::new()).update(before.clone());
    let index = files.iter().position(|(p, _)| p == file).unwrap();
    files[index].1 = edit(&files[index].1);
    let after = edited(&before, &files, index);
    let full = Analysis::new(after.clone());
    // The language server's way: the previous analysis is consumed.
    let owned = Analysis::new(Vec::new())
        .update(before.clone())
        .into_update_with_foreign(after.clone(), Vec::new());
    let incremental = previous.update(after);
    assert_eq!(incremental.dump(), full.dump(), "{file}");
    assert_eq!(owned.dump(), full.dump(), "{file}");
    assert_eq!(owned.reanalyzed_units(), incremental.reanalyzed_units());
    incremental.reanalyzed_units()
}

const TYPES: &str = include_str!("data/types.vhd");
const RESOLUTION: &str = include_str!("data/resolution.vhd");

#[test]
fn nothing_changed() {
    let files = split(TYPES);
    let before = inputs(&files);
    let previous = Analysis::new(Vec::new()).update(before.clone());
    let again = previous.update(before);
    assert_eq!(again.dump(), previous.dump());
    assert_eq!(again.reanalyzed_units(), 0);
}

#[test]
fn comment_in_an_architecture() {
    let n = check(TYPES, "valid.vhd", |t| format!("{t}-- a comment\n"));
    // The entity and its architecture.
    assert_eq!(n, 2);
}

#[test]
fn shifted_package_keeps_dependents() {
    // Every declaration of the package moves; nothing else changes.
    let n = check(TYPES, "types_pkg.vhd", |t| format!("\n\n{t}"));
    assert_eq!(n, 2);
}

#[test]
fn changed_type_in_a_package() {
    let n = check(TYPES, "types_pkg.vhd", |t| {
        t.replacen("count : natural;", "count : integer;", 1)
    });
    assert!(n > 2, "dependents are analyzed again: {n}");
}

#[test]
fn new_declaration_in_a_package() {
    check(TYPES, "types_pkg.vhd", |t| {
        t.replacen(
            "  subtype byte_t",
            "  constant C_NEW : integer := 1;\n  subtype byte_t",
            1,
        )
    });
}

#[test]
fn changed_function_signature() {
    check(TYPES, "types_pkg.vhd", |t| {
        t.replace(
            "function to_byte (n : natural)",
            "function to_byte (n : boolean)",
        )
    });
}

#[test]
fn renamed_unit() {
    check(TYPES, "invalid.vhd", |t| t.replace("invalid", "other"));
}

#[test]
fn every_file_of_the_fixtures() {
    for fixture in [TYPES, RESOLUTION] {
        for (file, _) in split(fixture) {
            check(fixture, &file, |t| format!("{t}-- edit\n"));
            check(fixture, &file, |t| format!("\n{t}"));
            check(fixture, &file, |t| t.replacen("integer", "natural", 1));
            check(fixture, &file, |t| t.replacen("std_logic", "bit", 1));
            check(fixture, &file, |t| {
                let lines: Vec<&str> = t.lines().collect();
                let mid = lines.len() / 2;
                lines
                    .iter()
                    .enumerate()
                    .filter(|&(i, _)| i != mid)
                    .map(|(_, l)| format!("{l}\n"))
                    .collect()
            });
        }
    }
}

const MIXED_TOP: &str = "-- file: top.vhd
entity top is end;
architecture rtl of top is
  signal a, b : bit;
begin
  u : entity work.leaf port map (a => a, y => b);
end;
-- file: other.vhd
entity other is end;
architecture rtl of other is begin end;
";

const LEAF: &str = "module leaf(input a, output y);\nendmodule\n";

fn verilog(text: &str) -> vfl_sema::ForeignInput {
    let config = vfl_config::Config::default();
    let file = vfl_project::VerilogFile::new(
        PathBuf::from("leaf.v"),
        text.to_owned(),
        vfl_project::Encoding::Utf8,
        &config,
        std::path::Path::new("."),
        vfl_config::VerilogStandard::Verilog2005,
    );
    vfl_sema::ForeignInput {
        path: file.path,
        text: file.text,
        libraries: vec!["work".into()],
        modules: file.foreign,
    }
}

/// Edits the Verilog file and checks the incremental result; returns how
/// many units were analyzed again.
fn check_verilog(edit: &str) -> (usize, String) {
    let files = split(MIXED_TOP);
    let vhdl = inputs(&files);
    let previous = Analysis::new(Vec::new()).update_with_foreign(vhdl.clone(), vec![verilog(LEAF)]);
    let full = Analysis::with_foreign(vhdl.clone(), vec![verilog(edit)]);
    let incremental = previous.update_with_foreign(vhdl, vec![verilog(edit)]);
    assert_eq!(incremental.dump(), full.dump());
    let codes = full
        .diagnostics()
        .iter()
        .map(|d| d.message.clone())
        .collect::<Vec<_>>()
        .join("; ");
    (incremental.reanalyzed_units(), codes)
}

#[test]
fn verilog_edits_reanalyze_only_what_sees_the_module() {
    // The same module: nothing to do.
    assert_eq!(check_verilog(LEAF).0, 0);
    // Moved, same ports: only the module itself.
    assert_eq!(check_verilog(&format!("// Leaf.\n{LEAF}")).0, 1);
    // A port renamed: the module and the architecture that maps it.
    let (count, messages) = check_verilog("module leaf(input a, output z);\nendmodule\n");
    assert_eq!(count, 2);
    assert!(
        messages.contains("module `leaf` has no port `y`"),
        "{messages}"
    );
}

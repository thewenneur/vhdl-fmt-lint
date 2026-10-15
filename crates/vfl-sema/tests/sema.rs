//! Snapshot tests: each `tests/data/*.vhd` holds several files separated by
//! `-- file: path` lines. Libraries are inferred as in a real project, and
//! the snapshot lists each file's libraries and every diagnostic.

use std::fmt::Write;
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
    // On demand: with clocked processes.
    let branches: Vec<_> = (0..analysis.user_files().len() as u32)
        .flat_map(|file| analysis.missing_branch_assignments(file, true).diagnostics)
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

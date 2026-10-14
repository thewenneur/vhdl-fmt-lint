//! Re-analysis after edits to VHDL and Verilog files, reusing the previous
//! analysis and lint results as the language server does, gives exactly
//! what analyzing and linting from scratch gives.
#![no_main]

use std::sync::Arc;

use libfuzzer_sys::fuzz_target;
use vfl_config::Config;
use vfl_fuzz::Source;
use vfl_lint::{FileSettings, LintCache, lint, lint_cached};
use vfl_sema::{Analysis, FileInput, ForeignInput};
use vfl_syntax::{Standard, SyntaxError};
use vfl_verilog_syntax::Parse;

/// The analysis inputs of the files, the syntax errors of the VHDL ones
/// and the trees of the Verilog ones.
fn inputs(
    files: &[Source],
    std: Standard,
    config: &Config,
) -> (
    Vec<FileInput>,
    Vec<ForeignInput>,
    Vec<Vec<SyntaxError>>,
    Vec<Parse>,
) {
    let (mut vhdl, mut foreign, mut errors, mut verilog) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for (i, file) in files.iter().enumerate() {
        if file.verilog {
            let (input, parse) = vfl_fuzz::foreign(file.path(i), &file.text, config);
            foreign.push(input);
            verilog.push(parse);
            continue;
        }
        let parse = vfl_syntax::parse(&file.text, std);
        errors.push(parse.errors().to_vec());
        vhdl.push(FileInput {
            path: file.path(i),
            text: Arc::from(file.text.as_str()),
            green: parse.green().clone(),
            libraries: vec!["work".into()],
        });
    }
    (vhdl, foreign, errors, verilog)
}

/// Verilog files follow the VHDL ones in the analysis.
fn settings<'a>(
    errors: &'a [Vec<SyntaxError>],
    verilog: &'a [Parse],
    config: &'a Config,
) -> Vec<FileSettings<'a>> {
    errors
        .iter()
        .map(|e| (e.as_slice(), None))
        .chain(verilog.iter().map(|p| (p.errors(), Some(p))))
        .enumerate()
        .map(|(i, (syntax_errors, verilog))| FileSettings {
            file: i as _,
            syntax_errors,
            config,
            verilog,
        })
        .collect()
}

fuzz_target!(|data: &[u8]| {
    let (mut files, script) = vfl_fuzz::split(data);
    let pick = script.first().copied().unwrap_or(0);
    let config = vfl_fuzz::config(pick);
    let std = vfl_fuzz::standard(pick / 4);
    let edits = vfl_fuzz::edits(script.get(1..).unwrap_or_default());
    if edits.is_empty() {
        return;
    }
    let (mut vhdl, mut foreign, errors, mut verilog) = inputs(&files, std, &config);
    // Only analyses built by `update` keep what reuse needs.
    let mut analysis = Analysis::new(Vec::new()).update_with_foreign(vhdl.clone(), foreign.clone());
    let mut cache = LintCache::default();
    lint_cached(&analysis, &settings(&errors, &verilog, &config), &mut cache);
    for edit in &edits {
        let i = edit.file % files.len();
        vfl_fuzz::apply(&mut files[i].text, edit);
        // Unchanged files keep their trees and modules, as in the server,
        // so that reuse is exercised.
        let (new_vhdl, new_foreign, errors, new_verilog) = inputs(&files, std, &config);
        let (mut v, mut f) = (0, 0);
        for (j, file) in files.iter().enumerate() {
            if file.verilog {
                if j == i {
                    foreign[f] = new_foreign[f].clone();
                    verilog[f] = new_verilog[f].clone();
                }
                f += 1;
            } else {
                if j == i {
                    vhdl[v] = new_vhdl[v].clone();
                }
                v += 1;
            }
        }
        analysis = analysis.update_with_foreign(vhdl.clone(), foreign.clone());
        let full = Analysis::with_foreign(vhdl.clone(), foreign.clone());
        assert_eq!(analysis.dump(), full.dump(), "analysis after {edit:?}");
        let cached = lint_cached(&analysis, &settings(&errors, &verilog, &config), &mut cache);
        let fresh = lint(&full, &settings(&errors, &verilog, &config));
        assert_eq!(
            format!("{:?}", cached.diagnostics),
            format!("{:?}", fresh.diagnostics),
            "lint after {edit:?}"
        );
    }
});

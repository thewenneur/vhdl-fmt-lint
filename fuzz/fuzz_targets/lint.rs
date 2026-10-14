//! Semantic analysis and every lint rule never panic, on up to four VHDL
//! and Verilog files of one library, and applying their fixes never breaks
//! the syntax of a file that parsed.
#![no_main]

use std::sync::Arc;

use libfuzzer_sys::fuzz_target;
use vfl_lint::{FileSettings, apply_fixes, lint};
use vfl_sema::{Analysis, FileInput};

fuzz_target!(|data: &[u8]| {
    let (files, script) = vfl_fuzz::split(data);
    let pick = script.first().copied().unwrap_or(0);
    let config = vfl_fuzz::config(pick);
    let std = vfl_fuzz::standard(pick / 4);
    let vhdl: Vec<(usize, &str)> = files
        .iter()
        .enumerate()
        .filter(|(_, f)| !f.verilog)
        .map(|(i, f)| (i, f.text.as_str()))
        .collect();
    let foreign = files
        .iter()
        .enumerate()
        .filter(|(_, f)| f.verilog)
        .map(|(i, f)| vfl_fuzz::foreign(f.path(i), &f.text, &config))
        .collect();
    let parses: Vec<_> = vhdl
        .iter()
        .map(|(_, t)| vfl_syntax::parse(t, std))
        .collect();
    let inputs = vhdl
        .iter()
        .zip(&parses)
        .map(|(&(i, text), parse)| FileInput {
            path: files[i].path(i),
            text: Arc::from(text),
            green: parse.green().clone(),
            libraries: vec!["work".into()],
        })
        .collect();
    let analysis = Analysis::with_foreign(inputs, foreign);
    let settings: Vec<FileSettings> = parses
        .iter()
        .enumerate()
        .map(|(i, parse)| FileSettings {
            file: i as _,
            syntax_errors: parse.errors(),
            config: &config,
        })
        .collect();
    let result = lint(&analysis, &settings);
    for (i, &(_, text)) in vhdl.iter().enumerate() {
        let fixes = result
            .diagnostics
            .iter()
            .filter(|d| d.file as usize == i)
            .filter_map(|d| d.fix.as_ref());
        let (fixed, _) = apply_fixes(text, fixes);
        if parses[i].errors().is_empty() {
            let errors = vfl_syntax::parse(&fixed, std).errors().to_vec();
            assert!(
                errors.is_empty(),
                "fixes broke the syntax: {errors:?}\n--- fixed ---\n{fixed}"
            );
        }
    }
});

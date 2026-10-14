//! Re-analysis after edits, reusing the previous analysis and lint
//! results as the language server does, gives exactly what analyzing and
//! linting from scratch gives.
#![no_main]

use std::path::PathBuf;
use std::sync::Arc;

use libfuzzer_sys::fuzz_target;
use vfl_lint::{FileSettings, LintCache, lint, lint_cached};
use vfl_sema::{Analysis, FileInput};
use vfl_syntax::Standard;

fn input(i: usize, text: &str, std: Standard) -> FileInput {
    FileInput {
        path: PathBuf::from(format!("/fuzz/f{i}.vhd")),
        text: Arc::from(text),
        green: vfl_syntax::parse(text, std).green().clone(),
        libraries: vec!["work".into()],
    }
}

fn settings<'a>(
    inputs: &[FileInput],
    errors: &'a [Vec<vfl_syntax::SyntaxError>],
    config: &'a vfl_config::Config,
) -> Vec<FileSettings<'a>> {
    (0..inputs.len())
        .map(|i| FileSettings {
            file: i as _,
            syntax_errors: &errors[i],
            config,
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
    let mut inputs: Vec<FileInput> = files
        .iter()
        .enumerate()
        .map(|(i, t)| input(i, t, std))
        .collect();
    // Only analyses built by `update` keep what reuse needs.
    let mut analysis = Analysis::new(Vec::new()).update(inputs.clone());
    let mut cache = LintCache::default();
    let errors = |files: &[String]| -> Vec<_> {
        files
            .iter()
            .map(|t| vfl_syntax::parse(t, std).errors().to_vec())
            .collect()
    };
    let errs = errors(&files);
    lint_cached(&analysis, &settings(&inputs, &errs, &config), &mut cache);
    for edit in &edits {
        let i = edit.file % files.len();
        vfl_fuzz::apply(&mut files[i], edit);
        inputs[i] = input(i, &files[i], std);
        analysis = analysis.update(inputs.clone());
        let full = Analysis::new(inputs.clone());
        assert_eq!(analysis.dump(), full.dump(), "analysis after {edit:?}");
        let errs = errors(&files);
        let cached = lint_cached(&analysis, &settings(&inputs, &errs, &config), &mut cache);
        let fresh = lint(&full, &settings(&inputs, &errs, &config));
        assert_eq!(
            format!("{:?}", cached.diagnostics),
            format!("{:?}", fresh.diagnostics),
            "lint after {edit:?}"
        );
    }
});

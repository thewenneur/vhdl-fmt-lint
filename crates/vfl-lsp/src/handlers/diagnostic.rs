//! Problems as LSP diagnostics.

use std::path::Path;

use lsp_types::{NumberOrString, Url};
use rustc_hash::FxHashMap;
use vfl_config::Level;
#[cfg(test)]
use vfl_sema::FileId;
use vfl_syntax::{LineIndex, TextRange, TextSize};

use super::uri;
use crate::world::{Doc, World};

/// A problem as an LSP diagnostic.
pub(crate) fn lsp_diagnostic(world: &World, d: &vfl_lint::Diagnostic) -> lsp_types::Diagnostic {
    let tags = d
        .rule
        .starts_with("sema/unused-")
        .then(|| vec![lsp_types::DiagnosticTag::UNNECESSARY]);
    lsp_types::Diagnostic {
        range: world.doc(d.file).range(d.range),
        severity: Some(match d.level {
            Level::Error => lsp_types::DiagnosticSeverity::ERROR,
            Level::Warn => lsp_types::DiagnosticSeverity::WARNING,
            _ => lsp_types::DiagnosticSeverity::INFORMATION,
        }),
        code: Some(NumberOrString::String(d.rule.into())),
        source: Some("vfl".into()),
        message: d.message.clone(),
        tags,
        ..Default::default()
    }
}

/// The diagnostics of every user file that has some, Verilog files
/// included.
#[cfg(test)]
pub fn diagnostics(world: &World) -> Vec<(Url, Vec<lsp_types::Diagnostic>)> {
    let mut by_file: FxHashMap<FileId, Vec<lsp_types::Diagnostic>> = FxHashMap::default();
    for d in &world.lint {
        by_file
            .entry(d.file)
            .or_default()
            .push(lsp_diagnostic(world, d));
    }
    let mut out: Vec<(Url, Vec<lsp_types::Diagnostic>)> = by_file
        .into_iter()
        .filter_map(|(file, diagnostics)| {
            Some((
                uri(&world.analysis.files()[file as usize].path)?,
                diagnostics,
            ))
        })
        .collect();
    out.extend(config_diagnostics(world));
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// The errors of invalid config files, on each file.
pub(crate) fn config_diagnostics(world: &World) -> Vec<(Url, Vec<lsp_types::Diagnostic>)> {
    let mut by_file: FxHashMap<&Path, Vec<&vfl_config::FileError>> = FxHashMap::default();
    for error in &world.config_file_errors {
        by_file.entry(&error.path).or_default().push(error);
    }
    by_file
        .into_iter()
        .filter_map(|(path, errors)| {
            let text = std::fs::read_to_string(path).unwrap_or_default();
            let index = LineIndex::new(&text);
            let doc = Doc {
                text: &text,
                index: &index,
            };
            let diagnostics = errors
                .into_iter()
                .map(|error| {
                    // Without a place, the problem is shown on the first line.
                    let span = error.span.clone().unwrap_or(0..0);
                    let offset = |o: usize| {
                        let mut o = o.min(text.len());
                        while !text.is_char_boundary(o) {
                            o -= 1;
                        }
                        TextSize::from(o as u32)
                    };
                    lsp_types::Diagnostic {
                        range: doc.range(TextRange::new(offset(span.start), offset(span.end))),
                        severity: Some(lsp_types::DiagnosticSeverity::ERROR),
                        code: Some(NumberOrString::String("config/invalid".into())),
                        source: Some("vfl".into()),
                        message: format!(
                            "{}\nThis file is ignored until it is fixed.",
                            error.message
                        ),
                        ..Default::default()
                    }
                })
                .collect();
            Some((uri(path)?, diagnostics))
        })
        .collect()
}

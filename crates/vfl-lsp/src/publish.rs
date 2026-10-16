//! The diagnostics shown in the editor, and what to send it after an
//! analysis to bring them up to date.
//!
//! An edit usually changes the findings of one file, if any: the findings
//! of each file are compared with those last sent, and only the files whose
//! findings or text changed are converted to LSP diagnostics again.

use std::path::PathBuf;
use std::sync::Arc;

use lsp_types::{PublishDiagnosticsParams, Url};
use rustc_hash::FxHashMap;
use vfl_config::Level;
use vfl_sema::FileId;
use vfl_syntax::TextRange;

use crate::World;
use crate::handlers::{config_diagnostics, lsp_diagnostic, uri};

/// The diagnostics the editor shows.
#[doc(hidden)]
#[derive(Default)]
pub struct Published {
    /// By the path of the file they are on.
    files: FxHashMap<PathBuf, Sent>,
    /// The errors of invalid config files, by file.
    config: FxHashMap<Url, Vec<lsp_types::Diagnostic>>,
}

/// The diagnostics last sent for a file, and what they were made from.
struct Sent {
    uri: Url,
    /// The text their positions are in.
    text: Arc<str>,
    findings: Vec<Finding>,
    diagnostics: Vec<lsp_types::Diagnostic>,
}

/// What of a lint finding its LSP diagnostic shows.
#[derive(PartialEq)]
struct Finding {
    rule: &'static str,
    level: Level,
    range: TextRange,
    message: String,
}

impl Finding {
    fn is(&self, d: &vfl_lint::Diagnostic) -> bool {
        self.rule == d.rule
            && self.level == d.level
            && self.range == d.range
            && self.message == d.message
    }

    fn of(d: &vfl_lint::Diagnostic) -> Finding {
        Finding {
            rule: d.rule,
            level: d.level,
            range: d.range,
            message: d.message.clone(),
        }
    }
}

impl Published {
    /// What to send after an analysis: the diagnostics of every file whose
    /// diagnostics changed, and none for the files that no longer have
    /// any.
    pub fn update(&mut self, world: &World) -> Vec<PublishDiagnosticsParams> {
        let mut out = Vec::new();
        let mut by_file: FxHashMap<FileId, Vec<&vfl_lint::Diagnostic>> = FxHashMap::default();
        for d in &world.lint {
            by_file.entry(d.file).or_default().push(d);
        }
        let mut previous = std::mem::take(&mut self.files);
        let files = world.analysis.files();
        for (file, findings) in by_file {
            let Some(analyzed) = files.get(file as usize) else {
                continue;
            };
            let sent = previous.remove(&analyzed.path);
            let unchanged = sent.as_ref().is_some_and(|sent| {
                Arc::ptr_eq(&sent.text, &analyzed.text)
                    && sent.findings.len() == findings.len()
                    && sent.findings.iter().zip(&findings).all(|(s, d)| s.is(d))
            });
            let sent = match sent {
                Some(sent) if unchanged => sent,
                sent => {
                    let Some(uri) = sent
                        .as_ref()
                        .map(|s| s.uri.clone())
                        .or_else(|| uri(&analyzed.path))
                    else {
                        continue;
                    };
                    let diagnostics: Vec<lsp_types::Diagnostic> =
                        findings.iter().map(|d| lsp_diagnostic(world, d)).collect();
                    // The same diagnostics at the same lines, after an
                    // edit that moved nothing, say: not sent again.
                    if sent.is_none_or(|sent| sent.diagnostics != diagnostics) {
                        out.push(params(uri.clone(), diagnostics.clone()));
                    }
                    Sent {
                        uri,
                        text: analyzed.text.clone(),
                        findings: findings.iter().map(|d| Finding::of(d)).collect(),
                        diagnostics,
                    }
                }
            };
            self.files.insert(analyzed.path.clone(), sent);
        }
        for sent in previous.into_values() {
            out.push(params(sent.uri, Vec::new()));
        }

        // Config files are few, and only have diagnostics while invalid.
        let mut previous = std::mem::take(&mut self.config);
        for (uri, diagnostics) in config_diagnostics(world) {
            if previous.remove(&uri).as_ref() != Some(&diagnostics) {
                out.push(params(uri.clone(), diagnostics.clone()));
            }
            self.config.insert(uri, diagnostics);
        }
        out.extend(previous.into_keys().map(|uri| params(uri, Vec::new())));
        out.sort_by(|a, b| a.uri.cmp(&b.uri));
        out
    }
}

fn params(uri: Url, diagnostics: Vec<lsp_types::Diagnostic>) -> PublishDiagnosticsParams {
    PublishDiagnosticsParams {
        uri,
        diagnostics,
        version: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Only the files whose diagnostics changed are sent again.
    #[test]
    fn sends_only_what_changed() {
        let dir = tempfile::tempdir().unwrap();
        let text = |name: &str| {
            format!(
                "entity {name} is\nend entity;\narchitecture rtl of {name} is\n  signal s : bit;\nbegin\nend architecture;\n"
            )
        };
        for name in ["a", "b"] {
            std::fs::write(dir.path().join(format!("{name}.vhd")), text(name)).unwrap();
        }
        let a = dir.path().join("a.vhd");
        let mut world = World::new(dir.path());
        let mut published = Published::default();
        let sent = published.update(&world);
        assert_eq!(sent.len(), 2, "{sent:?}");
        assert!(sent.iter().all(|p| !p.diagnostics.is_empty()));
        assert!(published.update(&world).is_empty());

        let mut edit = |new: String| {
            world.set_text(a.clone(), new);
            world.refresh();
            published.update(&world)
        };
        // A comment at the end moves nothing.
        assert!(edit(text("a") + "-- end\n").is_empty());
        // A line at the start moves the findings of `a` only.
        let sent = edit(format!("\n{}", text("a")));
        assert_eq!(sent.len(), 1);
        assert!(sent[0].uri.path().ends_with("/a.vhd"));
        assert_eq!(sent[0].diagnostics[0].range.start.line, 4);
        // Fixed: its diagnostics are cleared.
        let sent = edit(text("a").replace("  signal s : bit;\n", ""));
        assert_eq!(sent.len(), 1);
        assert!(sent[0].uri.path().ends_with("/a.vhd"));
        assert!(sent[0].diagnostics.is_empty());
    }
}

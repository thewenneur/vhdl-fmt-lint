//! Open FPGA vendor files (`.qsf`, `.qip`, `.sdc`, `.xdc`, `.tcl`): linted
//! on their own, from their text, as they are opened and edited. They are
//! not part of the project's analysis, so editing one never analyzes the
//! project, and editing HDL costs them nothing: the design's ports are
//! read once per analysis, and only while a vendor file is open.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use lsp_types::{NumberOrString, PublishDiagnosticsParams, TextDocumentContentChangeEvent, Url};
use rustc_hash::FxHashMap;
use vfl_config::Level;
use vfl_syntax::LineIndex;
use vfl_vendor::Ports;

use crate::world::{Doc, World};

/// The open vendor documents.
#[derive(Default)]
pub struct Documents {
    docs: FxHashMap<PathBuf, Document>,
    /// The design's ports, from the latest analysis; `None` until read.
    ports: Option<Arc<Ports>>,
}

struct Document {
    uri: Url,
    text: String,
    sent: Vec<lsp_types::Diagnostic>,
}

/// Whether the URI `uri` may name a vendor file, by its extension: cheap,
/// for every message.
pub fn may_be_vendor_uri(uri: &str) -> bool {
    uri.rsplit_once('.').is_some_and(|(_, extension)| {
        vfl_vendor::EXTENSIONS
            .iter()
            .any(|e| e.eq_ignore_ascii_case(extension))
    })
}

/// Whether `uri` is a vendor file.
pub fn is_vendor_uri(uri: &Url) -> bool {
    uri.to_file_path()
        .is_ok_and(|path| vfl_vendor::is_vendor_file(&path))
}

impl Documents {
    pub fn is_empty(&self) -> bool {
        self.docs.is_empty()
    }

    /// A document was opened.
    pub fn open(
        &mut self,
        world: &mut World,
        uri: Url,
        text: String,
    ) -> Option<PublishDiagnosticsParams> {
        let path = uri.to_file_path().ok()?;
        self.docs.insert(
            path.clone(),
            Document {
                uri,
                text,
                sent: Vec::new(),
            },
        );
        self.lint(world, &path)
    }

    /// A document was edited.
    pub fn change(
        &mut self,
        world: &mut World,
        uri: &Url,
        changes: Vec<TextDocumentContentChangeEvent>,
    ) -> Option<PublishDiagnosticsParams> {
        let path = uri.to_file_path().ok()?;
        let doc = self.docs.get_mut(&path)?;
        crate::apply_changes(&mut doc.text, changes);
        self.lint(world, &path)
    }

    /// A document was closed: its diagnostics go.
    pub fn close(&mut self, uri: &Url) -> Option<PublishDiagnosticsParams> {
        let path = uri.to_file_path().ok()?;
        let doc = self.docs.remove(&path)?;
        if self.docs.is_empty() {
            self.ports = None;
        }
        (!doc.sent.is_empty()).then(|| params(doc.uri, Vec::new()))
    }

    /// After an analysis: reads the design's ports again, and lints the
    /// open documents again if they changed.
    pub fn analyzed(&mut self, world: &mut World) -> Vec<PublishDiagnosticsParams> {
        if self.docs.is_empty() {
            return Vec::new();
        }
        let ports = vfl_lint::vendor::ports(&world.analysis);
        if self.ports.as_deref() == Some(&ports) {
            return Vec::new();
        }
        self.ports = Some(Arc::new(ports));
        let mut paths: Vec<PathBuf> = self.docs.keys().cloned().collect();
        paths.sort();
        paths
            .iter()
            .filter_map(|path| self.lint(world, path))
            .collect()
    }

    /// Lints the document at `path`; what to send if its diagnostics
    /// changed.
    fn lint(&mut self, world: &mut World, path: &Path) -> Option<PublishDiagnosticsParams> {
        if self.ports.is_none() {
            self.ports = Some(Arc::new(vfl_lint::vendor::ports(&world.analysis)));
        }
        let ports = self.ports.clone();
        let root = world.root.dir.clone();
        let root = (!root.as_os_str().is_empty()).then_some(root);
        let config = world.config(path).clone();
        let doc = self.docs.get_mut(path)?;
        let (findings, _) = vfl_lint::vendor::lint(
            path,
            &doc.text,
            &config,
            root.as_deref(),
            ports.as_deref(),
            0,
        );
        let index = LineIndex::new(&doc.text);
        let at = Doc {
            text: &doc.text,
            index: &index,
        };
        let diagnostics: Vec<lsp_types::Diagnostic> = findings
            .iter()
            .map(|d| lsp_types::Diagnostic {
                range: at.range(d.range),
                severity: Some(match d.level {
                    Level::Error => lsp_types::DiagnosticSeverity::ERROR,
                    Level::Warn => lsp_types::DiagnosticSeverity::WARNING,
                    _ => lsp_types::DiagnosticSeverity::INFORMATION,
                }),
                code: Some(NumberOrString::String(d.rule.into())),
                source: Some("vfl".into()),
                message: d.message.clone(),
                ..Default::default()
            })
            .collect();
        if diagnostics == doc.sent {
            return None;
        }
        doc.sent = diagnostics.clone();
        Some(params(doc.uri.clone(), diagnostics))
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

    #[test]
    fn vendor_uris_by_extension() {
        assert!(may_be_vendor_uri("file:///p/top.QSF"));
        assert!(may_be_vendor_uri("file:///p/pins.xdc"));
        assert!(may_be_vendor_uri("file:///p/pins.cst"));
        assert!(may_be_vendor_uri("file:///p/board.lpf"));
        assert!(!may_be_vendor_uri("file:///p/top.vhd"));
        assert!(!may_be_vendor_uri("file:///p.qsf/top"));
    }
}

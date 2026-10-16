//! Formatting: whole documents, ranges and as you type.

use std::path::Path;

use lsp_types::{
    DocumentFormattingParams, DocumentOnTypeFormattingParams, DocumentRangeFormattingParams,
    TextEdit,
};
use vfl_syntax::{LineIndex, TextRange, TextSize};

use crate::world::{Doc, World};

/// The latest text of the document at `path` and its config. Formatting
/// reads it instead of the analysis, so it never waits for one.
fn latest(world: &mut World, path: &Path) -> Option<(vfl_config::Config, String)> {
    let config = world.config(path).clone();
    let text = match world.overlays.get(path) {
        Some(text) => text.clone(),
        None => match crate::verilog::file(world, path) {
            Some(file) => file.text.to_string(),
            // The project's text: the analysis has only the outline of
            // a file over `max-file-size`.
            None => world.project.files[world.project.file_index(path)?]
                .text
                .to_string(),
        },
    };
    Some((config, text))
}

pub(super) fn formatting(
    world: &mut World,
    params: DocumentFormattingParams,
) -> Option<Vec<TextEdit>> {
    let path = params.text_document.uri.to_file_path().ok()?;
    let (config, text) = latest(world, &path)?;
    let index = LineIndex::new(&text);
    let doc = Doc {
        text: &text,
        index: &index,
    };
    let root = || world.root.dir.clone();
    let formatted = crate::format::document(Some(&path), doc.text, &config, root).ok()?;
    if formatted == doc.text {
        return Some(Vec::new());
    }
    // One edit covering what changed, so the cursor keeps its place.
    let edit = vfl_fmt::minimal_edit(doc.text, &formatted);
    let range = TextRange::new(
        TextSize::from(edit.range.start as u32),
        TextSize::from(edit.range.end as u32),
    );
    Some(vec![TextEdit {
        range: doc.range(range),
        new_text: edit.text,
    }])
}

/// Formats the line just finished: the one holding a `;` just typed, or
/// the one above a new line. Nothing when the file does not parse yet.
pub(super) fn on_type_formatting(
    world: &mut World,
    params: DocumentOnTypeFormattingParams,
) -> Option<Vec<TextEdit>> {
    let position = params.text_document_position;
    let path = position.text_document.uri.to_file_path().ok()?;
    let (config, text) = latest(world, &path)?;
    let index = LineIndex::new(&text);
    let doc = Doc {
        text: &text,
        index: &index,
    };
    let cursor = usize::from(doc.offset(position.position));
    let ch = params.ch.chars().next()?;
    let root = || world.root.dir.clone();
    let edits = crate::format::typed(Some(&path), &text, cursor, ch, &config, root).ok()?;
    Some(text_edits(doc, edits))
}

pub(super) fn range_formatting(
    world: &mut World,
    params: DocumentRangeFormattingParams,
) -> Option<Vec<TextEdit>> {
    let path = params.text_document.uri.to_file_path().ok()?;
    let (config, text) = latest(world, &path)?;
    let index = LineIndex::new(&text);
    let doc = Doc {
        text: &text,
        index: &index,
    };
    let range =
        usize::from(doc.offset(params.range.start))..usize::from(doc.offset(params.range.end));
    let root = || world.root.dir.clone();
    let edits = crate::format::range(Some(&path), &text, range, &config, root).ok()?;
    Some(text_edits(doc, edits))
}

fn text_edits(doc: Doc, edits: Vec<vfl_fmt::Edit>) -> Vec<TextEdit> {
    edits
        .into_iter()
        .map(|e| TextEdit {
            range: doc.range(TextRange::new(
                TextSize::from(e.range.start as u32),
                TextSize::from(e.range.end as u32),
            )),
            new_text: e.text,
        })
        .collect()
}

//! The language server never panics while VHDL and Verilog documents are
//! edited the way editors send changes, positions out of range included,
//! and every request is answered at arbitrary positions. After the edits, its
//! diagnostics are those of a server that opened the final text.
#![no_main]

use libfuzzer_sys::fuzz_target;
use lsp_types::{Position, Range, TextDocumentContentChangeEvent, Url};
use serde_json::{Value, json};
use vfl_lsp::World;

/// A position from raw edit bytes: often in range, sometimes past the end
/// of the line or of the text, as a buggy or racing client may send.
fn position(text: &str, at: usize) -> Position {
    let lines = text.lines().count() as u32 + 2;
    Position::new((at >> 8) as u32 % lines, (at & 0xFF) as u32 % 100)
}

fn requests(world: &mut World, uri: &Url, at: Position) {
    let doc = json!({ "uri": uri });
    let pos = json!({ "textDocument": doc, "position": at });
    let end = Position::new(at.line + 1, 0);
    let range = json!({ "start": at, "end": end });
    let options = json!({ "tabSize": 2, "insertSpaces": true });
    let calls: [(&str, Value); 15] = [
        ("textDocument/hover", pos.clone()),
        ("textDocument/completion", pos.clone()),
        ("textDocument/signatureHelp", pos.clone()),
        ("textDocument/definition", pos.clone()),
        ("textDocument/documentHighlight", pos.clone()),
        ("textDocument/prepareRename", pos.clone()),
        (
            "textDocument/references",
            json!({ "textDocument": doc, "position": at, "context": { "includeDeclaration": true } }),
        ),
        (
            "textDocument/rename",
            json!({ "textDocument": doc, "position": at, "newName": "renamed" }),
        ),
        (
            "textDocument/codeAction",
            json!({ "textDocument": doc, "range": range, "context": { "diagnostics": [] } }),
        ),
        (
            "textDocument/documentSymbol",
            json!({ "textDocument": doc }),
        ),
        (
            "textDocument/semanticTokens/full",
            json!({ "textDocument": doc }),
        ),
        (
            "textDocument/semanticTokens/range",
            json!({ "textDocument": doc, "range": range }),
        ),
        (
            "textDocument/formatting",
            json!({ "textDocument": doc, "options": options }),
        ),
        (
            "textDocument/rangeFormatting",
            json!({ "textDocument": doc, "range": range, "options": options }),
        ),
        (
            "textDocument/onTypeFormatting",
            json!({ "textDocument": doc, "position": at, "ch": ";", "options": options }),
        ),
    ];
    for (method, params) in calls {
        vfl_lsp::handle(world, method, params);
    }
}

fuzz_target!(|data: &[u8]| {
    let (mut files, script) = vfl_fuzz::split(data);
    let edits = vfl_fuzz::edits(script);
    let mut world = World::detached();
    for (i, file) in files.iter().enumerate() {
        world.set_text(file.path(i), file.text.clone());
    }
    world.refresh();
    for edit in &edits {
        let i = edit.file % files.len();
        let path = files[i].path(i);
        let uri = Url::from_file_path(&path).unwrap();
        let start = position(&files[i].text, edit.start);
        let end = Position::new(
            start.line + (edit.len as u32 >> 3),
            (edit.len as u32 & 7) * 9,
        );
        // Some clients send the whole text instead of a range.
        let range = (edit.len != 31).then_some(Range::new(start, end));
        let change = TextDocumentContentChangeEvent {
            range,
            range_length: None,
            text: edit.text.clone(),
        };
        vfl_lsp::apply_changes(&mut files[i].text, vec![change]);
        world.set_text(path, files[i].text.clone());
        world.refresh();
        requests(&mut world, &uri, start);
    }
    if edits.is_empty() {
        for (i, file) in files.iter().enumerate() {
            let uri = Url::from_file_path(file.path(i)).unwrap();
            requests(&mut world, &uri, Position::new(0, 0));
        }
        return;
    }
    let mut fresh = World::detached();
    for (i, file) in files.iter().enumerate() {
        fresh.set_text(file.path(i), file.text.clone());
    }
    fresh.refresh();
    assert_eq!(
        format!("{:?}", world.lint),
        format!("{:?}", fresh.lint),
        "diagnostics after edits differ from opening the final text"
    );
});

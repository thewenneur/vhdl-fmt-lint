//! Measures how long hover, signature help, completion, code actions,
//! highlights and references take: opens a project, then sends each
//! request at a spread of positions in a spread of files, and prints the
//! median and slowest times. Range formatting and format on type (a new
//! line) are timed the same way, and so are go to implementation, type
//! definition and declaration, linked editing, selection ranges, inlay
//! hints (for the 50 lines from each position, a screen) and the preparing
//! of a call hierarchy with its incoming and outgoing calls. Then the code
//! lenses of every file, each resolved, the inlay hints of every whole
//! file, and the semantic tokens of every file, whole and as a delta after
//! the first.
//! Usage: request_latency DIR

use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde_json::json;
use vfl_lsp::World;

fn main() {
    let dir = PathBuf::from(std::env::args().nth(1).expect("usage: request_latency DIR"));
    let dir = std::fs::canonicalize(dir).unwrap();
    let mut world = World::new(&dir);
    let files: Vec<(PathBuf, String)> = {
        let all = &world.project.files;
        let step = (all.len() / 16).max(1);
        all.iter()
            .step_by(step)
            .take(16)
            .map(|f| (f.path.clone(), f.text.to_string()))
            .collect()
    };
    for method in [
        "textDocument/hover",
        "textDocument/signatureHelp",
        "textDocument/completion",
        "textDocument/codeAction",
        "textDocument/documentHighlight",
        "textDocument/references",
        "textDocument/rangeFormatting",
        "textDocument/onTypeFormatting",
        "textDocument/implementation",
        "textDocument/typeDefinition",
        "textDocument/declaration",
        "textDocument/linkedEditingRange",
        "textDocument/selectionRange",
        "textDocument/inlayHint",
        "callHierarchy",
    ] {
        let mut times: Vec<Duration> = Vec::new();
        let mut answered = 0;
        for (path, text) in &files {
            let uri = lsp_types::Url::from_file_path(path).unwrap();
            let lines: Vec<&str> = text.lines().collect();
            for (line, content) in lines.iter().enumerate().step_by(7) {
                let character = content.encode_utf16().count() as u32 / 2;
                let position = json!({ "line": line, "character": character });
                let options = json!({ "tabSize": 2, "insertSpaces": true });
                let params = match method {
                    "textDocument/codeAction" => json!({
                        "textDocument": { "uri": uri },
                        "range": { "start": position, "end": position },
                        "context": { "diagnostics": [] },
                    }),
                    "textDocument/references" => json!({
                        "textDocument": { "uri": uri },
                        "position": position,
                        "context": { "includeDeclaration": true },
                    }),
                    "textDocument/rangeFormatting" => json!({
                        "textDocument": { "uri": uri },
                        "range": { "start": { "line": line, "character": 0 }, "end": position },
                        "options": options,
                    }),
                    "textDocument/onTypeFormatting" => json!({
                        "textDocument": { "uri": uri },
                        "position": { "line": line + 1, "character": 0 },
                        "ch": "\n",
                        "options": options,
                    }),
                    "textDocument/selectionRange" => json!({
                        "textDocument": { "uri": uri },
                        "positions": [position],
                    }),
                    "textDocument/inlayHint" => json!({
                        "textDocument": { "uri": uri },
                        "range": {
                            "start": { "line": line, "character": 0 },
                            "end": { "line": line + 50, "character": 0 },
                        },
                    }),
                    _ => json!({ "textDocument": { "uri": uri }, "position": position }),
                };
                let start = Instant::now();
                let result = if method == "callHierarchy" {
                    call_hierarchy(&mut world, params)
                } else {
                    vfl_lsp::handle(&mut world, method, params)
                };
                times.push(start.elapsed());
                if result.is_some_and(|r| !r.is_null()) {
                    answered += 1;
                }
            }
        }
        times.sort();
        if let Some(max) = times.last() {
            println!(
                "{method}: {} requests, {answered} answered, median {:?}, max {:?}",
                times.len(),
                times[times.len() / 2],
                max
            );
        }
    }
    let paths: Vec<PathBuf> = world.project.files.iter().map(|f| f.path.clone()).collect();
    let mut lenses: Vec<Duration> = Vec::new();
    let mut resolves: Vec<(Duration, String)> = Vec::new();
    let mut hints: Vec<(Duration, usize, PathBuf)> = Vec::new();
    for path in &paths {
        let uri = lsp_types::Url::from_file_path(path).unwrap();
        let document = json!({ "textDocument": { "uri": uri } });
        let start = Instant::now();
        let found = vfl_lsp::handle(&mut world, "textDocument/codeLens", document.clone());
        lenses.push(start.elapsed());
        for lens in found
            .as_ref()
            .and_then(|l| l.as_array())
            .into_iter()
            .flatten()
        {
            let start = Instant::now();
            let resolved = vfl_lsp::handle(&mut world, "codeLens/resolve", lens.clone());
            let title = resolved.map_or(String::new(), |r| r["command"]["title"].to_string());
            resolves.push((start.elapsed(), title));
        }
        let mut whole = document;
        whole["range"] = json!({
            "start": { "line": 0, "character": 0 },
            "end": { "line": u32::MAX, "character": 0 },
        });
        let start = Instant::now();
        let found = vfl_lsp::handle(&mut world, "textDocument/inlayHint", whole);
        let count = found.and_then(|h| h.as_array().map(Vec::len)).unwrap_or(0);
        hints.push((start.elapsed(), count, path.clone()));
    }
    lenses.sort();
    resolves.sort();
    hints.sort_by_key(|h| h.0);
    if let (Some(max), Some((resolve_max, title))) = (lenses.last(), resolves.last()) {
        println!(
            "textDocument/codeLens: {} files, median {:?}, max {max:?}; \
             codeLens/resolve: {} lenses, median {:?}, max {resolve_max:?} ({title})",
            lenses.len(),
            lenses[lenses.len() / 2],
            resolves.len(),
            resolves[resolves.len() / 2].0,
        );
    }
    if let Some((max, count, path)) = hints.last() {
        println!(
            "textDocument/inlayHint (whole file): {} files, median {:?}, max {max:?} \
             ({count} hints, {})",
            hints.len(),
            hints[hints.len() / 2].0,
            path.display()
        );
    }
    let mut times: Vec<(Duration, Duration, usize, PathBuf)> = Vec::new();
    for path in paths {
        let uri = lsp_types::Url::from_file_path(&path).unwrap();
        let start = Instant::now();
        let full = vfl_lsp::handle(
            &mut world,
            "textDocument/semanticTokens/full",
            json!({ "textDocument": { "uri": uri } }),
        )
        .unwrap_or_default();
        let full_time = start.elapsed();
        let start = Instant::now();
        vfl_lsp::handle(
            &mut world,
            "textDocument/semanticTokens/full/delta",
            json!({ "textDocument": { "uri": uri }, "previousResultId": full["resultId"] }),
        );
        let tokens = full["data"].as_array().map_or(0, |d| d.len() / 5);
        times.push((full_time, start.elapsed(), tokens, path));
    }
    times.sort_by_key(|t| t.0);
    if let Some((max, delta, tokens, path)) = times.last() {
        let median = &times[times.len() / 2];
        println!(
            "textDocument/semanticTokens/full: {} files, median {:?}, max {max:?} \
             ({tokens} tokens, delta {delta:?}, {})",
            times.len(),
            median.0,
            path.display()
        );
    }
}

/// Prepares a call hierarchy at a position, and asks for the incoming and
/// outgoing calls of each item, as an editor showing it would.
fn call_hierarchy(world: &mut World, params: serde_json::Value) -> Option<serde_json::Value> {
    let items = vfl_lsp::handle(world, "textDocument/prepareCallHierarchy", params)?;
    for item in items.as_array()? {
        let item = json!({ "item": item });
        vfl_lsp::handle(world, "callHierarchy/incomingCalls", item.clone());
        vfl_lsp::handle(world, "callHierarchy/outgoingCalls", item);
    }
    Some(items)
}

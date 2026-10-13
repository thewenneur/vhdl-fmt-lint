//! Measures how long hover, signature help, completion, code actions,
//! highlights and references take: opens a project, then sends each
//! request at a spread of positions in a spread of files, and prints the
//! median and slowest times. Range formatting and format on type (a new
//! line) are timed the same way. Then the semantic tokens of every file, whole
//! and as a delta after the first.
//! Usage: request_latency DIR

use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde_json::json;
use vhone_lsp::World;

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
                    _ => json!({ "textDocument": { "uri": uri }, "position": position }),
                };
                let start = Instant::now();
                let result = vhone_lsp::handle(&mut world, method, params);
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
    let mut times: Vec<(Duration, Duration, usize, PathBuf)> = Vec::new();
    let paths: Vec<PathBuf> = world.project.files.iter().map(|f| f.path.clone()).collect();
    for path in paths {
        let uri = lsp_types::Url::from_file_path(&path).unwrap();
        let start = Instant::now();
        let full = vhone_lsp::handle(
            &mut world,
            "textDocument/semanticTokens/full",
            json!({ "textDocument": { "uri": uri } }),
        )
        .unwrap_or_default();
        let full_time = start.elapsed();
        let start = Instant::now();
        vhone_lsp::handle(
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

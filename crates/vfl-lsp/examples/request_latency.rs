//! Measures how long hover, signature help, completion, range formatting
//! and format on type (a new line) take: opens a
//! project, then sends each request at a spread of positions in a spread
//! of files, and prints the median and slowest times.
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
                let options = json!({ "tabSize": 2, "insertSpaces": true });
                let params = match method {
                    "textDocument/rangeFormatting" => json!({
                        "textDocument": { "uri": uri },
                        "range": {
                            "start": { "line": line, "character": 0 },
                            "end": { "line": line, "character": character },
                        },
                        "options": options,
                    }),
                    "textDocument/onTypeFormatting" => json!({
                        "textDocument": { "uri": uri },
                        "position": { "line": line + 1, "character": 0 },
                        "ch": "\n",
                        "options": options,
                    }),
                    _ => json!({
                        "textDocument": { "uri": uri },
                        "position": { "line": line, "character": character },
                    }),
                };
                let start = Instant::now();
                let result = vfl_lsp::handle(&mut world, method, params);
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
}

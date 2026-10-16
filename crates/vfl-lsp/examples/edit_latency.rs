//! Measures how long the language server takes from an edit to fresh
//! diagnostics: opens a project, then for a spread of files appends a
//! comment, and separately changes a type (`integer` to `natural`), and
//! times each refresh. Each edit comes as the editor sends it (an
//! incremental `didChange`); the time to apply it and the time to work out
//! which diagnostics to send after the refresh are printed too.
//! Usage: edit_latency DIR [FILE...]

use std::path::PathBuf;
use std::time::{Duration, Instant};

use vfl_lsp::{Published, World};

/// An edit of a file's text, for the given round.
type Edit = fn(&str, usize) -> String;

/// The LSP position of byte `offset` in `text` (UTF-16 columns).
fn position(text: &str, offset: usize) -> lsp_types::Position {
    let before = &text[..offset];
    let line = before.matches('\n').count();
    let start = before.rfind('\n').map_or(0, |i| i + 1);
    let character = before[start..].encode_utf16().count();
    lsp_types::Position::new(line as u32, character as u32)
}

/// The change an editor sends to turn `old` into `new`.
fn change(old: &str, new: &str) -> lsp_types::TextDocumentContentChangeEvent {
    let edit = vfl_fmt::minimal_edit(old, new);
    lsp_types::TextDocumentContentChangeEvent {
        range: Some(lsp_types::Range::new(
            position(old, edit.range.start),
            position(old, edit.range.end),
        )),
        range_length: None,
        text: edit.text,
    }
}

fn median(times: &mut [Duration]) -> Duration {
    times.sort();
    times.get(times.len() / 2).copied().unwrap_or_default()
}

fn main() {
    let mut args = std::env::args().skip(1);
    let dir = PathBuf::from(args.next().expect("usage: edit_latency DIR [FILE...]"));
    let dir = std::fs::canonicalize(dir).unwrap();
    let start = Instant::now();
    let mut world = World::new(&dir);
    println!("{}: open {:?}", dir.display(), start.elapsed());
    let mut published = Published::default();
    published.update(&world);
    let mut files: Vec<PathBuf> = args.map(|f| dir.join(f)).collect();
    if files.is_empty() {
        // A spread of files: every n-th one of the project.
        let all: Vec<PathBuf> = world.project.files.iter().map(|f| f.path.clone()).collect();
        let step = (all.len() / 8).max(1);
        files = all.into_iter().step_by(step).take(8).collect();
    }
    let edits: [(&str, Edit); 2] = [
        ("comment", |t, round| format!("{t}\n-- edit {round}\n")),
        ("type", |t, round| {
            let to = if round % 2 == 0 {
                "natural"
            } else {
                "positive"
            };
            t.replacen("integer", to, 1)
        }),
    ];
    for (name, edit) in edits {
        let mut times: Vec<Duration> = Vec::new();
        let (mut changes, mut publishes) = (Vec::new(), Vec::new());
        for path in &files {
            let Some(i) = world.project.file_index(path) else {
                continue;
            };
            let original = world.project.files[i].text.to_string();
            world.set_text(path.clone(), original.clone());
            let mut text = original.clone();
            for round in 0..3 {
                let new = edit(&original, round);
                let event = change(&text, &new);
                let start = Instant::now();
                vfl_lsp::did_change(&mut world, path.clone(), vec![event]);
                changes.push(start.elapsed());
                let start = Instant::now();
                world.refresh();
                times.push(start.elapsed());
                let start = Instant::now();
                for params in published.update(&world) {
                    // Sending serializes them.
                    std::hint::black_box(serde_json::to_value(params).unwrap());
                }
                publishes.push(start.elapsed());
                text = new;
            }
            vfl_lsp::did_change(&mut world, path.clone(), vec![change(&text, &original)]);
            world.refresh();
            published.update(&world);
        }
        times.sort();
        if let (Some(min), Some(max)) = (times.first(), times.last()) {
            println!(
                "  {name}: {} edits, median {:?}, min {:?}, max {:?}; change median {:?}, \
                 publish median {:?}, max {:?}",
                times.len(),
                times[times.len() / 2],
                min,
                max,
                median(&mut changes),
                median(&mut publishes),
                publishes.last().copied().unwrap_or_default(),
            );
        }
    }
}

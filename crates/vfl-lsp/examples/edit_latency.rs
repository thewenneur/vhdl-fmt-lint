//! Measures how long the language server takes from an edit to fresh
//! diagnostics: opens a project, then for a spread of files appends a
//! comment, and separately changes a type (`integer` to `natural`), and
//! times each refresh.
//! Usage: edit_latency DIR [FILE...]

use std::path::PathBuf;
use std::time::{Duration, Instant};

use vfl_lsp::World;

/// An edit of a file's text, for the given round.
type Edit = fn(&str, usize) -> String;

fn main() {
    let mut args = std::env::args().skip(1);
    let dir = PathBuf::from(args.next().expect("usage: edit_latency DIR [FILE...]"));
    let dir = std::fs::canonicalize(dir).unwrap();
    let start = Instant::now();
    let mut world = World::new(&dir);
    println!("{}: open {:?}", dir.display(), start.elapsed());
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
        for path in &files {
            let Some(i) = world.project.file_index(path) else {
                continue;
            };
            let original = world.project.files[i].text.to_string();
            for round in 0..3 {
                world.set_text(path.clone(), edit(&original, round));
                let start = Instant::now();
                world.refresh();
                times.push(start.elapsed());
            }
            world.set_text(path.clone(), original);
            world.refresh();
        }
        times.sort();
        if let (Some(min), Some(max)) = (times.first(), times.last()) {
            println!(
                "  {name}: {} edits, median {:?}, min {:?}, max {:?}",
                times.len(),
                times[times.len() / 2],
                min,
                max
            );
        }
    }
}

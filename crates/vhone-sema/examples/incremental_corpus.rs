//! Checks `Analysis::update` against `Analysis::new` on real projects: for
//! a spread of files, applies a few edits and compares the results.
//! Usage: incremental_corpus [--std 93|2008|2019] [--files N] DIR...

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use vhone_project::{Project, ProjectRoot, RootKind};
use vhone_sema::{Analysis, FileInput};

/// An edit of a file's text.
type Edit = fn(&str) -> String;

fn main() {
    let mut args = std::env::args().skip(1);
    let mut dirs = Vec::new();
    let mut std = None;
    let mut count = 8;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--std" => std = args.next(),
            "--files" => count = args.next().unwrap().parse().unwrap(),
            _ => dirs.push(PathBuf::from(arg)),
        }
    }
    let mut failed = false;
    for dir in dirs {
        let root = ProjectRoot {
            dir: dir.clone(),
            kind: RootKind::Fallback,
        };
        let mut config = vhone_config::load_for_dir(&dir).unwrap();
        if let Some(std) = &std {
            config.project.standard = toml::Value::String(std.clone()).try_into().unwrap();
        }
        let standard = config.standard();
        let paths = vhone_project::collect_sources(&dir, &config.project).unwrap();
        let project = Project::from_paths(root, config, paths).unwrap();
        let inputs: Vec<FileInput> = project
            .files
            .iter()
            .zip(&project.libraries)
            .map(|(f, libs)| FileInput {
                path: f.path.clone(),
                text: Arc::clone(&f.text),
                green: f.parse.green().clone(),
                libraries: libs.iter().map(|(l, _)| l.clone()).collect(),
            })
            .collect();
        // Built by `update`, as the language server does, so it can be reused.
        let base = Analysis::new(Vec::new()).update(inputs.clone());
        let step = (inputs.len() / count.max(1)).max(1);
        let edits: [(&str, Edit); 4] = [
            ("comment", |t| format!("{t}\n-- edit\n")),
            ("shift", |t| format!("\n{t}")),
            ("type", |t| {
                if t.contains("integer") {
                    t.replacen("integer", "natural", 1)
                } else {
                    t.replacen("std_logic", "bit", 1)
                }
            }),
            ("line", |t| {
                let lines: Vec<&str> = t.lines().collect();
                let mid = lines.len() / 2;
                lines
                    .iter()
                    .enumerate()
                    .filter(|&(i, _)| i != mid)
                    .map(|(_, l)| format!("{l}\n"))
                    .collect()
            }),
        ];
        let (mut full_time, mut incremental_time) = (Duration::ZERO, Duration::ZERO);
        let mut checked = 0;
        for index in (0..inputs.len()).step_by(step).take(count) {
            for (name, edit) in &edits {
                let mut after = inputs.clone();
                let text = edit(&after[index].text);
                let parse = vhone_syntax::parse(&text, standard);
                after[index].text = Arc::from(text.as_str());
                after[index].green = parse.green().clone();
                let start = Instant::now();
                let full = Analysis::new(after.clone());
                full_time += start.elapsed();
                let start = Instant::now();
                let incremental = base.update(after);
                incremental_time += start.elapsed();
                checked += 1;
                let (a, b) = (incremental.dump(), full.dump());
                if a != b {
                    failed = true;
                    println!("MISMATCH {} ({name})", inputs[index].path.display());
                    let diff = a.lines().zip(b.lines()).filter(|(x, y)| x != y).take(3);
                    for (x, y) in diff {
                        println!("  incremental: {x}\n  full:        {y}");
                    }
                }
            }
        }
        println!(
            "{}: {checked} edits, full {:?}, incremental {:?} on average",
            dir.display(),
            full_time / checked.max(1) as u32,
            incremental_time / checked.max(1) as u32,
        );
    }
    if failed {
        std::process::exit(1);
    }
}

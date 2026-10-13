//! Analyzes directories as projects and reports diagnostics by code.
//! Usage: sema_corpus [--std 93|2008|2019] [--show CODE]... [--quiet] DIR...

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use vfl_project::{Project, ProjectRoot, RootKind};
use vfl_sema::{Analysis, FileInput};

fn main() {
    let mut args = std::env::args().skip(1);
    let mut dirs = Vec::new();
    let mut show = Vec::new();
    let mut std = None;
    let mut all = false;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--std" => std = args.next(),
            "--show" => show.push(args.next().unwrap()),
            "--all" => all = true,
            _ => dirs.push(PathBuf::from(arg)),
        }
    }
    for dir in dirs {
        let start = Instant::now();
        let root = ProjectRoot {
            dir: dir.clone(),
            kind: RootKind::Fallback,
        };
        let mut config = vfl_config::load_for_dir(&dir).unwrap();
        if let Some(std) = &std {
            config.project.standard = toml::Value::String(std.clone()).try_into().unwrap();
        }
        let paths = vfl_project::collect_sources(&dir, &config.project).unwrap();
        let project = Project::from_paths(root, config, paths).unwrap();
        let loaded = start.elapsed();
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
        let analysis = Analysis::new(inputs);
        let total = start.elapsed();
        let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
        for d in analysis.diagnostics() {
            *counts.entry(d.code).or_default() += 1;
            if all || show.iter().any(|s| s == d.code) {
                let file = &analysis.files()[d.file as usize];
                let at = file.line_index.line_col(&file.text, d.range.start());
                println!(
                    "{}:{}:{}: {}: {}",
                    file.path.display(),
                    at.line + 1,
                    at.col + 1,
                    d.code,
                    d.message
                );
            }
        }
        println!(
            "{}: {} files, {} units, load {:?}, total {:?}",
            dir.display(),
            project.files.len(),
            analysis.units().len(),
            loaded,
            total
        );
        for (code, n) in counts {
            println!("  {n:6} {code}");
        }
    }
}

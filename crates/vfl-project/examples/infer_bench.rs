//! Times library inference on a project: once with the library lists just
//! read (as on load), then again with them kept (as on an edit).
//! The load time includes reading the lists.
//! Usage: infer_bench DIR [ROUNDS]
use std::time::Instant;

fn main() {
    let mut args = std::env::args().skip(1);
    let dir = std::path::PathBuf::from(args.next().expect("a project directory"));
    let rounds: u32 = args.next().map_or(50, |r| r.parse().unwrap());
    let mut project = vfl_project::Project::load(vfl_project::find_root(&dir)).unwrap();
    let mut read = f64::MAX;
    let mut load = f64::MAX;
    let mut edit = f64::MAX;
    for _ in 0..rounds {
        let t = Instant::now();
        let sources = project.library_sources().unwrap();
        read = read.min(t.elapsed().as_secs_f64());
        project.infer_with(&sources);
        load = load.min(t.elapsed().as_secs_f64());
        let t = Instant::now();
        project.infer_with(&sources);
        edit = edit.min(t.elapsed().as_secs_f64());
    }
    println!(
        "lists read in {:.2} ms, load {:.2} ms, edit {:.2} ms",
        read * 1e3,
        load * 1e3,
        edit * 1e3
    );
}

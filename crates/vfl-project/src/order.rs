//! Compile order: every file of a project, in each of its libraries, after
//! the files declaring the units it uses, as simulators and synthesis tools
//! need them. See `vfl order` in `docs/cli.md`.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};
use std::path::PathBuf;

use crate::load::Project;
use crate::summary::{FileSummary, UnitKind};

/// The language of a file to compile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    Vhdl,
    Verilog,
}

/// One file compiled into one library.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompileStep {
    pub path: PathBuf,
    pub library: String,
    pub language: Language,
}

/// The files of a project in compile order.
#[derive(Debug, Clone, Default)]
pub struct CompileOrder {
    pub steps: Vec<CompileStep>,
    /// Groups of steps that use each other's units, so that no order
    /// works; each is listed in the order it was compiled in.
    pub cycles: Vec<Vec<CompileStep>>,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum OrderError {
    #[error("no entity or configuration `{0}` in the project")]
    NoTop(String),
    #[error("`{name}` is declared in several libraries ({}); write it as library.{name}", .libraries.join(", "))]
    AmbiguousTop {
        name: String,
        libraries: Vec<String>,
    },
}

/// A source file: its path, summary, libraries and language.
type FileEntry<'a> = (
    &'a PathBuf,
    &'a FileSummary,
    &'a [(String, crate::Reason)],
    Language,
);

/// A dependency between steps: `hard` ones must be respected (a use
/// clause, `entity lib.e`, an architecture's entity), soft ones only when
/// they can be (a component's default binding, a Verilog instance), since
/// the tools bind those at elaboration.
struct Edge {
    to: usize,
    hard: bool,
}

/// Orders the files of `project`. With `top` (`name` or `library.name`),
/// only the steps the top entity or configuration needs are kept: the
/// files of the units it uses, with their architectures and package
/// bodies, transitively.
pub fn compile_order(project: &Project, top: Option<&str>) -> Result<CompileOrder, OrderError> {
    let files: Vec<FileEntry> = project
        .files
        .iter()
        .zip(&project.libraries)
        .map(|(f, l)| (&f.path, &f.summary, l.as_slice(), Language::Vhdl))
        .chain(
            project
                .verilog
                .iter()
                .zip(&project.verilog_libraries)
                .map(|(f, l)| (&f.path, &f.summary, l.as_slice(), Language::Verilog)),
        )
        .collect();
    let mut steps = Vec::new();
    let mut file_of = Vec::new();
    for (file, (path, _, libraries, language)) in files.iter().enumerate() {
        for (library, _) in libraries.iter() {
            steps.push(CompileStep {
                path: (*path).clone(),
                library: library.clone(),
                language: *language,
            });
            file_of.push(file);
        }
    }
    // Files are sorted by path within each language: a stable tie-break.
    let mut order: Vec<usize> = (0..steps.len()).collect();
    order.sort_by(|&a, &b| {
        (&steps[a].path, &steps[a].library).cmp(&(&steps[b].path, &steps[b].library))
    });
    let mut rank = vec![0; steps.len()];
    for (r, &s) in order.iter().enumerate() {
        rank[s] = r;
    }

    // (library, unit) -> steps declaring it as a primary unit; (library,
    // primary) -> steps holding its architectures or body.
    let mut declared: HashMap<(&str, &str), Vec<usize>> = HashMap::new();
    let mut secondary: HashMap<(&str, &str), Vec<usize>> = HashMap::new();
    for (step, &file) in file_of.iter().enumerate() {
        let library = steps[step].library.as_str();
        for unit in &files[file].1.units {
            let (map, name) = match (&unit.primary, unit.kind) {
                (Some(primary), UnitKind::Architecture | UnitKind::PackageBody) => {
                    (&mut secondary, primary.as_str())
                }
                _ => (&mut declared, unit.name.as_str()),
            };
            let entry = map.entry((library, name)).or_default();
            if !entry.contains(&step) {
                entry.push(step);
            }
        }
    }

    let mut edges: Vec<Vec<Edge>> = (0..steps.len()).map(|_| Vec::new()).collect();
    for (step, &file) in file_of.iter().enumerate() {
        let (_, summary, _, language) = files[file];
        for r in &summary.refs {
            let library = if r.library == "work" {
                steps[step].library.as_str()
            } else {
                r.library.as_str()
            };
            let Some(targets) = declared.get(&(library, r.unit.as_str())) else {
                continue;
            };
            // A unit of the same file: the file's own order.
            if targets.iter().any(|&t| file_of[t] == file) {
                continue;
            }
            let hard = !r.implicit && language == Language::Vhdl;
            for &to in targets {
                match edges[step].iter_mut().find(|e| e.to == to) {
                    Some(edge) => edge.hard |= hard,
                    None => edges[step].push(Edge { to, hard }),
                }
            }
        }
    }

    let keep: Vec<bool> = match top {
        None => vec![true; steps.len()],
        Some(top) => {
            let roots = top_steps(top, &steps, &file_of, &files)?;
            let mut keep = vec![false; steps.len()];
            let mut stack = roots;
            while let Some(step) = stack.pop() {
                if std::mem::replace(&mut keep[step], true) {
                    continue;
                }
                stack.extend(edges[step].iter().map(|e| e.to));
                let library = steps[step].library.as_str();
                for unit in files[file_of[step]]
                    .1
                    .units
                    .iter()
                    .filter(|u| u.kind.is_primary())
                {
                    if let Some(bodies) = secondary.get(&(library, unit.name.as_str())) {
                        stack.extend(bodies.iter().copied());
                    }
                }
            }
            keep
        }
    };

    let cycles = cycles(&edges, &keep, &rank)
        .into_iter()
        .map(|cycle| cycle.into_iter().map(|s| steps[s].clone()).collect())
        .collect();
    // All dependencies if they leave no cycle, else the hard ones, then
    // breaking what cycles remain.
    let sorted = topological(&edges, &keep, &rank, false)
        .unwrap_or_else(|| topological(&edges, &keep, &rank, true).expect("breaks cycles"));
    Ok(CompileOrder {
        steps: sorted.into_iter().map(|s| steps[s].clone()).collect(),
        cycles,
    })
}

/// The steps declaring the top unit: an entity or configuration, or else
/// any primary unit of that name.
fn top_steps(
    top: &str,
    steps: &[CompileStep],
    file_of: &[usize],
    files: &[FileEntry],
) -> Result<Vec<usize>, OrderError> {
    let key = |s: &str| vfl_syntax::identifier_key(s.trim());
    let (library, name) = match top.split_once('.') {
        Some((library, name)) => (Some(key(library)), key(name)),
        None => (None, key(top)),
    };
    let declares = |step: usize, kinds: &[UnitKind]| {
        files[file_of[step]]
            .1
            .units
            .iter()
            .any(|u| u.name == name && kinds.contains(&u.kind))
    };
    let mut found: Vec<usize> = Vec::new();
    for kinds in [
        &[UnitKind::Entity, UnitKind::Configuration][..],
        &[
            UnitKind::Package,
            UnitKind::PackageInstance,
            UnitKind::Context,
        ],
    ] {
        found = (0..steps.len())
            .filter(|&s| library.as_ref().is_none_or(|l| *l == steps[s].library))
            .filter(|&s| declares(s, kinds))
            .collect();
        if !found.is_empty() {
            break;
        }
    }
    if found.is_empty() {
        return Err(OrderError::NoTop(top.to_owned()));
    }
    let mut libraries: Vec<String> = found.iter().map(|&s| steps[s].library.clone()).collect();
    libraries.sort();
    libraries.dedup();
    if libraries.len() > 1 {
        return Err(OrderError::AmbiguousTop {
            name: top.to_owned(),
            libraries,
        });
    }
    Ok(found)
}

/// Kahn's algorithm over the kept steps, the lowest rank first among those
/// ready. With `hard_only`, soft edges are ignored and a cycle is broken at
/// its lowest-ranked step; otherwise a cycle gives `None`.
fn topological(
    edges: &[Vec<Edge>],
    keep: &[bool],
    rank: &[usize],
    hard_only: bool,
) -> Option<Vec<usize>> {
    let n = edges.len();
    let counts = |e: &Edge| keep[e.to] && (e.hard || !hard_only);
    let mut waiting = vec![0usize; n];
    let mut users: Vec<Vec<usize>> = vec![Vec::new(); n];
    for step in (0..n).filter(|&s| keep[s]) {
        for edge in edges[step].iter().filter(|e| counts(e)) {
            waiting[step] += 1;
            users[edge.to].push(step);
        }
    }
    let mut ready: BinaryHeap<Reverse<(usize, usize)>> = (0..n)
        .filter(|&s| keep[s] && waiting[s] == 0)
        .map(|s| Reverse((rank[s], s)))
        .collect();
    let mut done = vec![false; n];
    let mut out = Vec::new();
    let total = keep.iter().filter(|&&k| k).count();
    while out.len() < total {
        let step = match ready.pop() {
            Some(Reverse((_, step))) => step,
            None if hard_only => (0..n)
                .filter(|&s| keep[s] && !done[s])
                .min_by_key(|&s| rank[s])
                .expect("steps remain"),
            None => return None,
        };
        if std::mem::replace(&mut done[step], true) {
            continue;
        }
        out.push(step);
        for &user in &users[step] {
            waiting[user] = waiting[user].saturating_sub(1);
            if waiting[user] == 0 && !done[user] {
                ready.push(Reverse((rank[user], user)));
            }
        }
    }
    Some(out)
}

/// The strongly connected components of more than one step along hard
/// edges (Tarjan's algorithm, iterative), each sorted by rank.
fn cycles(edges: &[Vec<Edge>], keep: &[bool], rank: &[usize]) -> Vec<Vec<usize>> {
    let n = edges.len();
    let hard: Vec<Vec<usize>> = edges
        .iter()
        .map(|e| {
            e.iter()
                .filter(|e| e.hard && keep[e.to])
                .map(|e| e.to)
                .collect()
        })
        .collect();
    let mut index = vec![usize::MAX; n];
    let mut low = vec![0; n];
    let mut on_stack = vec![false; n];
    let mut stack = Vec::new();
    let mut next = 0;
    let mut out = Vec::new();
    for start in (0..n).filter(|&s| keep[s]) {
        if index[start] != usize::MAX {
            continue;
        }
        // (step, next edge to visit)
        let mut call = vec![(start, 0usize)];
        index[start] = next;
        low[start] = next;
        next += 1;
        stack.push(start);
        on_stack[start] = true;
        while let Some(&(v, i)) = call.last() {
            if let Some(&w) = hard[v].get(i) {
                call.last_mut().expect("in a call").1 += 1;
                if index[w] == usize::MAX {
                    index[w] = next;
                    low[w] = next;
                    next += 1;
                    stack.push(w);
                    on_stack[w] = true;
                    call.push((w, 0));
                } else if on_stack[w] {
                    low[v] = low[v].min(index[w]);
                }
                continue;
            }
            call.pop();
            if let Some(&(parent, _)) = call.last() {
                low[parent] = low[parent].min(low[v]);
            }
            if low[v] == index[v] {
                let mut component = Vec::new();
                while let Some(w) = stack.pop() {
                    on_stack[w] = false;
                    component.push(w);
                    if w == v {
                        break;
                    }
                }
                if component.len() > 1 {
                    component.sort_by_key(|&s| rank[s]);
                    out.push(component);
                }
            }
        }
    }
    out.sort_by_key(|c| rank[c[0]]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ProjectRoot, RootKind};
    use std::fs;
    use std::path::Path;

    fn project(files: &[(&str, &str)]) -> (tempfile::TempDir, Project) {
        let dir = tempfile::tempdir().unwrap();
        for (path, text) in files {
            let path = dir.path().join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, text).unwrap();
        }
        let project = Project::load(ProjectRoot {
            dir: dir.path().to_owned(),
            kind: RootKind::Fallback,
        })
        .unwrap();
        (dir, project)
    }

    fn names(root: &Path, steps: &[CompileStep]) -> Vec<String> {
        steps
            .iter()
            .map(|s| {
                let path = s
                    .path
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                format!("{}:{path}", s.library)
            })
            .collect()
    }

    const FILES: &[(&str, &str)] = &[
        (
            "a_top.vhd",
            "library util; use util.pkg.all;\nentity top is end;\narchitecture rtl of top is begin\n  u : entity work.core;\n  c : comp;\nend;",
        ),
        ("b_core.vhd", "entity core is end;"),
        (
            "c_core_arch.vhd",
            "library util; use util.pkg.all;\narchitecture rtl of core is begin end;",
        ),
        ("comp.vhd", "entity comp is end;"),
        ("util/pkg.vhd", "package pkg is end;"),
        ("util/pkg_body.vhd", "package body pkg is end package body;"),
        ("unused.vhd", "entity unused is end;"),
    ];

    #[test]
    fn units_come_after_what_they_use() {
        let (dir, project) = project(FILES);
        let order = compile_order(&project, None).unwrap();
        assert!(order.cycles.is_empty());
        assert_eq!(
            names(dir.path(), &order.steps),
            [
                "work:b_core.vhd",
                "work:comp.vhd",
                "work:unused.vhd",
                "util:util/pkg.vhd",
                "work:a_top.vhd",
                "work:c_core_arch.vhd",
                "util:util/pkg_body.vhd",
            ]
        );
    }

    #[test]
    fn a_top_keeps_only_what_it_needs() {
        let (dir, project) = project(FILES);
        let order = compile_order(&project, Some("top")).unwrap();
        // The core's architecture and the package's body come along;
        // `unused` does not.
        assert_eq!(
            names(dir.path(), &order.steps),
            [
                "work:b_core.vhd",
                "work:comp.vhd",
                "util:util/pkg.vhd",
                "work:a_top.vhd",
                "work:c_core_arch.vhd",
                "util:util/pkg_body.vhd",
            ]
        );
        let order = compile_order(&project, Some("work.Core")).unwrap();
        assert_eq!(
            names(dir.path(), &order.steps),
            [
                "work:b_core.vhd",
                "util:util/pkg.vhd",
                "work:c_core_arch.vhd",
                "util:util/pkg_body.vhd"
            ]
        );
        assert_eq!(
            compile_order(&project, Some("nope")).unwrap_err(),
            OrderError::NoTop("nope".into())
        );
        assert!(matches!(
            compile_order(&project, Some("util.top")),
            Err(OrderError::NoTop(_))
        ));
    }

    #[test]
    fn cycles_are_reported_and_broken() {
        let (dir, project) = project(&[
            ("a.vhd", "use work.b.all; package a is end;"),
            ("b.vhd", "use work.a.all; package b is end;"),
            ("c.vhd", "use work.a.all; entity c is end;"),
        ]);
        let order = compile_order(&project, None).unwrap();
        assert_eq!(order.cycles.len(), 1);
        assert_eq!(
            names(dir.path(), &order.cycles[0]),
            ["work:a.vhd", "work:b.vhd"]
        );
        assert_eq!(
            names(dir.path(), &order.steps),
            ["work:a.vhd", "work:b.vhd", "work:c.vhd"]
        );
    }

    #[test]
    fn component_bindings_give_way_to_cycles() {
        // `a` instantiates component `b`, whose entity uses `a`'s package:
        // legal, since components bind at elaboration.
        let (dir, project) = project(&[
            (
                "a.vhd",
                "package p is end;\nentity a is end;\narchitecture r of a is begin u : b; end;",
            ),
            ("b.vhd", "use work.p.all; entity b is end;"),
        ]);
        let order = compile_order(&project, None).unwrap();
        assert!(order.cycles.is_empty());
        assert_eq!(
            names(dir.path(), &order.steps),
            ["work:a.vhd", "work:b.vhd"]
        );
    }
}

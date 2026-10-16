//! Library inference: which library each file belongs to, worked out from
//! the units the files declare and the `lib.unit` references between them.
//! See `docs/design.md`, "Project autodiscovery", step 4.

use std::collections::{BTreeSet, HashMap};
use std::fmt;
use std::path::PathBuf;

use crate::summary::FileSummary;

/// Libraries that are bundled with the tool, so references to them do not
/// pull project files in unless the project declares the unit itself.
pub const BUNDLED_LIBRARIES: [&str; 2] = ["std", "ieee"];

/// Why a file was put in a library.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reason {
    /// An explicit entry: `[project.libraries]` in `vfl.toml`, or an
    /// imported tool file such as `vhdl_ls.toml`.
    Explicit { source: String },
    /// Another file refers to one of its units as `library.unit`.
    Referenced { by: PathBuf, unit: String },
    /// A file of that library refers to one of its units through `work`.
    Work { by: PathBuf, unit: String },
    /// It refers through `work` to units of that library and nothing names it.
    Follows { unit: String },
    /// Nothing places it anywhere else.
    Default,
}

impl fmt::Display for Reason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Reason::Explicit { source } => write!(f, "listed in {source}"),
            Reason::Referenced { by, unit } => {
                write!(f, "{} refers to {unit} in this library", by.display())
            }
            Reason::Work { by, unit } => write!(f, "{} uses work.{unit}", by.display()),
            Reason::Follows { unit } => write!(f, "uses work.{unit} from this library"),
            Reason::Default => f.write_str("default library"),
        }
    }
}

/// The libraries of each file, in input order, with the reasons.
#[derive(Debug, Clone, Default)]
pub struct Inference {
    pub libraries: Vec<Vec<(String, Reason)>>,
    /// Ambiguities worth showing to the user.
    pub notes: Vec<String>,
}

/// Infers the library of every file.
///
/// `explicit[i]`, when set, fixes file `i`'s libraries; inference never
/// changes those. Everything else starts empty, gains libraries from
/// `lib.unit` references and along `work` references until nothing changes.
/// A file still in no library then takes `fallback[i]` when set, and those
/// propagate the same way; what is left ends in `work`.
pub fn infer_libraries(
    paths: &[PathBuf],
    summaries: &[FileSummary],
    explicit: &[Option<Vec<(String, Reason)>>],
    fallback: &[Option<Vec<(String, Reason)>>],
) -> Inference {
    infer_in(paths, &Dirs::new(paths), summaries, explicit, fallback, &[])
}

/// [`infer_libraries`], with the files' directories worked out.
pub(crate) fn infer_in(
    paths: &[PathBuf],
    dirs: &Dirs,
    summaries: &[FileSummary],
    explicit: &[Option<Vec<(String, Reason)>>],
    fallback: &[Option<Vec<(String, Reason)>>],
    seeds: &[Option<Vec<(String, Reason)>>],
) -> Inference {
    let n = paths.len();
    let mut libs: Vec<Vec<(String, Reason)>> = explicit
        .iter()
        .map(|e| e.clone().unwrap_or_default())
        .collect();
    libs.resize(n, Vec::new());
    let mut fixed: Vec<bool> = (0..n)
        .map(|i| explicit.get(i).is_some_and(Option::is_some))
        .collect();
    // Seeded files start in those libraries and may gain more.
    for (i, seed) in seeds.iter().enumerate() {
        if let Some(seed) = seed
            && !fixed[i]
        {
            libs[i] = seed.clone();
        }
    }
    let mut notes = Vec::new();

    // Primary unit name -> files declaring it.
    let mut declared: HashMap<&str, Vec<usize>> = HashMap::new();
    for (file, summary) in summaries.iter().enumerate() {
        for unit in summary.units.iter().filter(|u| u.kind.is_primary()) {
            let files = declared.entry(unit.name.as_str()).or_default();
            if !files.contains(&file) {
                files.push(file);
            }
        }
    }

    let has = |libs: &[(String, Reason)], lib: &str| libs.iter().any(|(l, _)| l == lib);

    let closest = |from: usize, candidates: &[usize]| dirs.closest(from, candidates);

    // 1. Named libraries: `lib.unit` puts unit's file in lib.
    for (file, summary) in summaries.iter().enumerate() {
        for r in summary.refs.iter().filter(|r| r.library != "work") {
            let Some(candidates) = declared.get(r.unit.as_str()) else {
                continue;
            };
            if candidates.iter().any(|&c| has(&libs[c], &r.library)) {
                continue;
            }
            let open: Vec<usize> = candidates.iter().copied().filter(|&c| !fixed[c]).collect();
            if open.is_empty() {
                continue;
            }
            let picked = closest(file, &open);
            if picked.len() > 1 {
                notes.push(ambiguity(paths, &r.library, &r.unit, &picked));
            }
            for c in picked {
                libs[c].push((
                    r.library.clone(),
                    Reason::Referenced {
                        by: paths[file].clone(),
                        unit: r.unit.clone(),
                    },
                ));
            }
        }
    }

    // 2. Propagate along `work` references until nothing changes.
    let mut memo = Memo::new(summaries);
    propagate(
        paths, dirs, &mut memo, summaries, &declared, &fixed, &mut libs,
    );

    // 3. Files the code placed nowhere take their fallback libraries
    // (`vhdl_ls.toml`), which then propagate in turn.
    let mut fell_back = false;
    for file in 0..n {
        if let Some(Some(fallback)) = fallback.get(file)
            && libs[file].is_empty()
        {
            libs[file] = fallback.clone();
            fixed[file] = true;
            fell_back = true;
        }
    }
    if fell_back {
        // What is open changed: forget which files were.
        let mut memo = Memo::new(summaries);
        propagate(
            paths, dirs, &mut memo, summaries, &declared, &fixed, &mut libs,
        );
    }

    for file_libs in &mut libs {
        if file_libs.is_empty() {
            file_libs.push(("work".into(), Reason::Default));
        }
    }
    Inference {
        libraries: libs,
        notes,
    }
}

/// Spreads libraries along `work` references until nothing changes: a unit
/// used through `work` joins its user's libraries, and a file nothing
/// placed joins those of the units it uses.
fn propagate(
    paths: &[PathBuf],
    dirs: &Dirs,
    memo: &mut Memo,
    summaries: &[FileSummary],
    declared: &HashMap<&str, Vec<usize>>,
    fixed: &[bool],
    libs: &mut [Vec<(String, Reason)>],
) {
    let n = paths.len();
    let has = |libs: &[(String, Reason)], lib: &str| libs.iter().any(|(l, _)| l == lib);
    loop {
        let mut changed = false;
        for file in 0..n {
            for (k, r) in summaries[file].refs.iter().enumerate() {
                if r.library != "work" {
                    continue;
                }
                let Some(candidates) = declared.get(r.unit.as_str()) else {
                    continue;
                };
                // A unit declared in the same file is the one meant (a
                // package body's own package), whatever else declares it.
                if candidates.contains(&file) {
                    continue;
                }
                // Forward: the referenced unit is in each of my libraries.
                let mine: Vec<String> = libs[file].iter().map(|(l, _)| l.clone()).collect();
                for lib in &mine {
                    if candidates.iter().any(|&c| has(&libs[c], lib)) {
                        continue;
                    }
                    let picked = memo.forward[file][k].get_or_insert_with(|| {
                        let open: Vec<usize> =
                            candidates.iter().copied().filter(|&c| !fixed[c]).collect();
                        dirs.closest(file, &open)
                    });
                    for &c in picked.iter() {
                        libs[c].push((
                            lib.clone(),
                            Reason::Work {
                                by: paths[file].clone(),
                                unit: r.unit.clone(),
                            },
                        ));
                        changed = true;
                    }
                }
            }
        }
        // Backward: a file nothing placed joins the libraries of the units
        // it uses through `work` (an architecture joins its entity's).
        for file in 0..n {
            if !libs[file].is_empty() {
                continue;
            }
            let mut joined: BTreeSet<String> = BTreeSet::new();
            let mut via = String::new();
            for (k, r) in summaries[file].refs.iter().enumerate() {
                if r.library != "work" || r.implicit {
                    continue;
                }
                let Some(candidates) = declared.get(r.unit.as_str()) else {
                    continue;
                };
                if candidates.contains(&file) {
                    continue;
                }
                let picked =
                    memo.backward[file][k].get_or_insert_with(|| dirs.closest(file, candidates));
                for &c in picked.iter() {
                    for (lib, _) in &libs[c] {
                        if joined.insert(lib.clone()) && via.is_empty() {
                            via = r.unit.clone();
                        }
                    }
                }
            }
            for lib in joined {
                libs[file].push((lib, Reason::Follows { unit: via.clone() }));
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
}

/// How many library conflicts these libraries leave: units declared more
/// than once in a library, and `lib.unit` and `work.unit` references that
/// do not resolve, the unit being declared in the project but in none of
/// the files of the library the reference means (for `work`, each library
/// of the referencing file). References to units the project does not
/// declare at all, or to bundled libraries, are not counted.
pub fn library_conflicts(summaries: &[FileSummary], libs: &[Vec<(String, Reason)>]) -> usize {
    let mut declared: HashMap<&str, Vec<usize>> = HashMap::new();
    for (file, summary) in summaries.iter().enumerate() {
        for unit in summary.units.iter().filter(|u| u.kind.is_primary()) {
            let files = declared.entry(unit.name.as_str()).or_default();
            if !files.contains(&file) {
                files.push(file);
            }
        }
    }
    let in_library = |candidates: &[usize], lib: &str| {
        candidates
            .iter()
            .any(|&c| libs[c].iter().any(|(l, _)| l == lib))
    };
    // A unit declared twice in one library: one of them is not the unit
    // its users mean.
    let mut missing: usize = declared
        .values()
        .map(|files| {
            let mut seen: Vec<&str> = files
                .iter()
                .flat_map(|&f| libs[f].iter().map(|(l, _)| l.as_str()))
                .collect();
            let all = seen.len();
            seen.sort_unstable();
            seen.dedup();
            all - seen.len()
        })
        .sum();
    for (file, summary) in summaries.iter().enumerate() {
        for r in summary.refs.iter().filter(|r| !r.implicit) {
            if BUNDLED_LIBRARIES.contains(&r.library.as_str()) {
                continue;
            }
            let Some(candidates) = declared.get(r.unit.as_str()) else {
                continue;
            };
            if r.library == "work" {
                missing += libs[file]
                    .iter()
                    .filter(|(lib, _)| !in_library(candidates, lib))
                    .count();
            } else if !in_library(candidates, &r.library) {
                missing += 1;
            }
        }
    }
    missing
}

/// Which files each `work` reference means, worked out once per
/// propagation: by file, by index in its references.
struct Memo {
    /// Among the declaring files inference may still place.
    forward: Vec<Vec<Option<Vec<usize>>>>,
    /// Among all the declaring files.
    backward: Vec<Vec<Option<Vec<usize>>>>,
}

impl Memo {
    fn new(summaries: &[FileSummary]) -> Self {
        let empty = || summaries.iter().map(|s| vec![None; s.refs.len()]).collect();
        Memo {
            forward: empty(),
            backward: empty(),
        }
    }
}

/// Each file's directory, as numbers standing for its components, to
/// compare directories without parsing paths again.
pub(crate) struct Dirs(Vec<Vec<u32>>);

impl Dirs {
    pub(crate) fn new(paths: &[PathBuf]) -> Self {
        let mut ids: HashMap<&std::ffi::OsStr, u32> = HashMap::new();
        Dirs(
            paths
                .iter()
                .map(|path| {
                    path.parent()
                        .unwrap_or(path)
                        .components()
                        .map(|c| {
                            let next = ids.len() as u32;
                            *ids.entry(c.as_os_str()).or_insert(next)
                        })
                        .collect()
                })
                .collect(),
        )
    }

    /// How many leading components two files' directories share.
    fn common_depth(&self, a: usize, b: usize) -> usize {
        (self.0[a].iter())
            .zip(&self.0[b])
            .take_while(|(x, y)| x == y)
            .count()
    }

    /// Picks the files a reference from `from` means among the
    /// `candidates` declaring the unit: those closest to `from` in the
    /// directory tree.
    pub(crate) fn closest(&self, from: usize, candidates: &[usize]) -> Vec<usize> {
        let depth = |c: usize| self.common_depth(from, c);
        let best = candidates.iter().map(|&c| depth(c)).max().unwrap_or(0);
        candidates
            .iter()
            .copied()
            .filter(|&c| depth(c) == best)
            .collect()
    }
}

fn ambiguity(paths: &[PathBuf], library: &str, unit: &str, files: &[usize]) -> String {
    let list: Vec<String> = files
        .iter()
        .map(|&f| paths[f].display().to_string())
        .collect();
    format!(
        "{library}.{unit} is declared in several files ({}); all were put in {library}. \
         Add a [project.libraries] entry to pick one.",
        list.join(", ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::summary::summarize;
    use vfl_syntax::{Standard, parse};

    fn infer(files: &[(&str, &str)]) -> Vec<(String, Vec<String>)> {
        let paths: Vec<PathBuf> = files.iter().map(|(p, _)| PathBuf::from(p)).collect();
        let summaries: Vec<_> = files
            .iter()
            .map(|(_, text)| summarize(&parse(text, Standard::Vhdl2008).syntax()))
            .collect();
        let inference = infer_libraries(&paths, &summaries, &vec![None; files.len()], &[]);
        files
            .iter()
            .zip(inference.libraries)
            .map(|((p, _), libs)| (p.to_string(), libs.into_iter().map(|(l, _)| l).collect()))
            .collect()
    }

    #[test]
    fn named_references_and_work_propagation() {
        let result = infer(&[
            ("osvvm/alert.vhd", "package alertlogpkg is end;"),
            (
                "osvvm/random.vhd",
                "use work.alertlogpkg.all; package randompkg is end;",
            ),
            (
                "osvvm/random_body.vhd",
                "package body randompkg is end package body;",
            ),
            ("osvvm/unused.vhd", "package unusedpkg is end;"),
            (
                "tb/tb.vhd",
                "library osvvm; use osvvm.randompkg.all; entity tb is end;",
            ),
            ("tb/tb_arch.vhd", "architecture sim of tb is begin end;"),
        ]);
        let libs = |p: &str| result.iter().find(|(f, _)| f == p).unwrap().1.clone();
        assert_eq!(libs("osvvm/random.vhd"), ["osvvm"]);
        // Reached only through `work.alertlogpkg` from an osvvm file.
        assert_eq!(libs("osvvm/alert.vhd"), ["osvvm"]);
        // A package body joins its package's library.
        assert_eq!(libs("osvvm/random_body.vhd"), ["osvvm"]);
        assert_eq!(libs("osvvm/unused.vhd"), ["work"]);
        assert_eq!(libs("tb/tb.vhd"), ["work"]);
        assert_eq!(libs("tb/tb_arch.vhd"), ["work"]);
    }

    #[test]
    fn closest_declaration_wins() {
        let result = infer(&[
            ("a/pkg.vhd", "package pkg is end;"),
            ("b/pkg.vhd", "package pkg is end;"),
            (
                "b/user.vhd",
                "library lib; use lib.pkg.all; entity user is end;",
            ),
        ]);
        assert_eq!(result[0].1, ["work"]);
        assert_eq!(result[1].1, ["lib"]);
    }

    #[test]
    fn explicit_libraries_are_kept() {
        let files = [
            ("x/pkg.vhd", "package pkg is end;"),
            (
                "y/user.vhd",
                "library lib; use lib.pkg.all; entity user is end;",
            ),
        ];
        let paths: Vec<PathBuf> = files.iter().map(|(p, _)| PathBuf::from(p)).collect();
        let summaries: Vec<_> = files
            .iter()
            .map(|(_, text)| summarize(&parse(text, Standard::Vhdl2008).syntax()))
            .collect();
        let explicit = vec![
            Some(vec![(
                "mylib".to_string(),
                Reason::Explicit {
                    source: "vfl.toml".into(),
                },
            )]),
            None,
        ];
        let inference = infer_libraries(&paths, &summaries, &explicit, &[]);
        assert_eq!(inference.libraries[0].len(), 1);
        assert_eq!(inference.libraries[0][0].0, "mylib");
    }

    #[test]
    fn a_package_body_means_its_own_package() {
        let result = infer(&[
            (
                "src/pkg.vhd",
                "package pkg is end; package body pkg is end;",
            ),
            (
                "docs/pkg.vhd",
                "package pkg is end; package body pkg is end;",
            ),
            (
                "docs/tb.vhd",
                "library lib; use lib.pkg.all; entity tb is end;",
            ),
        ]);
        // `lib.pkg` from docs/ picks docs/pkg.vhd; its body's implicit
        // `work.pkg` must not drag src/pkg.vhd into `lib` too.
        assert_eq!(result[1].1, ["lib"]);
        assert_eq!(result[0].1, ["work"]);
    }
}

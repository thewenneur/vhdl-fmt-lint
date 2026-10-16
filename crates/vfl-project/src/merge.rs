//! Merging the library lists of a project with what its code says: each
//! file's library is chosen among the opinions of `vfl.toml`, the files
//! of other tools, `vhdl_ls.toml` and inference, by the references of the
//! code. See `docs/design.md`, "Merging library sources".

use std::collections::HashMap;
use std::path::PathBuf;

use crate::infer::{BUNDLED_LIBRARIES, Dirs, Inference, Reason, infer_in, library_conflicts};
use crate::summary::FileSummary;

/// The libraries of one file, with why.
pub type Libs = Vec<(String, Reason)>;

/// What kind of list an opinion comes from, which ranks it on a tie.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Kind {
    /// A file of another tool (`run.py`, `.core`, `.qsf`...).
    Tool,
    /// What inference makes of the code alone.
    Code,
    /// `vhdl_ls.toml`, by default the lowest: written by hand, it often
    /// lags behind the code.
    VhdlLs,
}

/// The opinions of the library lists about one file: those that list it.
pub(crate) type Opinions = Vec<(Kind, Libs)>;

/// The library sources of a project disagreed about a file: what each
/// said and what vfl chose.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibraryConflict {
    pub path: PathBuf,
    /// The libraries each opinion puts the file in, and the sources
    /// holding it (a list, or the code).
    pub opinions: Vec<(Vec<String>, Vec<String>)>,
    /// The libraries vfl put the file in.
    pub chosen: Vec<String>,
    /// Whether those are all the opinions' libraries together.
    pub union: bool,
    /// Unresolved references (and units declared twice in a library) the
    /// chosen libraries leave around this file, and the fewest another
    /// list's would.
    pub left: usize,
    pub rejected_left: usize,
}

impl LibraryConflict {
    /// The message shown on the file.
    pub fn message(&self) -> String {
        let libs = |libs: &[String]| {
            libs.iter()
                .map(|l| format!("`{l}`"))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let opinions: Vec<String> = self
            .opinions
            .iter()
            .map(|(l, sources)| {
                let verb = if sources.len() > 1 { "put" } else { "puts" };
                format!("{} {verb} it in {}", sources.join(" and "), libs(l))
            })
            .collect();
        let why = if self.left < self.rejected_left {
            format!(
                "it leaves {} unresolved library references around the file, the other \
                 choices at least {}",
                self.left, self.rejected_left
            )
        } else {
            "the references of the code do not tell them apart, and it ranks first".to_owned()
        };
        format!(
            "The library sources disagree about this file: {}. vfl uses {}{}: {why}. \
             Add the file to [project.libraries] in vfl.toml to choose.",
            opinions.join("; "),
            if self.union { "all of them, " } else { "" },
            libs(&self.chosen),
        )
    }
}

/// How each file's libraries are to be found, as decided once per set of
/// library lists.
#[derive(Debug, Clone)]
pub(crate) enum Pick {
    /// Placed by a list: inference does not change it.
    Listed(Libs),
    /// Left to inference, with libraries to take if it places the file
    /// nowhere.
    Inferred(Option<Libs>),
    /// In these libraries, and in more if inference places it there.
    Seeded(Libs),
}

/// How the lists are taken, from one starting point of [`decide`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// A list only `vhdl_ls.toml` holds and the code does not dispute
    /// fills in where inference places the file nowhere.
    Ranked,
    /// Every list is kept as is.
    Fixed,
    /// Every list is kept, and inference may add libraries to what
    /// `vhdl_ls.toml` alone says.
    Seeded,
}

/// What [`decide`] found.
#[derive(Debug, Clone, Default)]
pub(crate) struct Decision {
    /// By path relative to the root.
    pub picks: HashMap<String, Pick>,
    pub conflicts: Vec<LibraryConflict>,
    pub notes: Vec<String>,
}

/// A distinct opinion about a file's libraries.
#[derive(Debug, Clone)]
struct Candidate {
    libs: Libs,
    /// The kinds of the sources holding it, and their names.
    kinds: Vec<Kind>,
    sources: Vec<String>,
    union: bool,
}

impl Candidate {
    fn names(&self) -> Vec<String> {
        names(&self.libs)
    }

    fn listed(&self) -> bool {
        self.kinds.iter().any(|k| *k != Kind::Code)
    }

    /// The order on a tie: more sources agreeing first, then by the best
    /// of their kinds; the union of the others last.
    fn rank(&self) -> (bool, usize, Kind) {
        (
            self.union,
            usize::MAX - self.kinds.len(),
            self.kinds.iter().copied().min().unwrap_or(Kind::VhdlLs),
        )
    }
}

fn names(libs: &Libs) -> Vec<String> {
    let mut names: Vec<String> = libs.iter().map(|(l, _)| l.clone()).collect();
    names.sort();
    names.dedup();
    names
}

fn source_name(kind: Kind, libs: &Libs) -> String {
    match (kind, libs.first()) {
        (Kind::Code, _) => "the code".to_owned(),
        (_, Some((_, Reason::Explicit { source }))) => source.clone(),
        _ => "a library list".to_owned(),
    }
}

/// The distinct opinions about a file, best ranked first: those of the
/// lists, that of the code (its [`firm`] libraries), and when they differ
/// their union.
fn candidates(opinions: &[(Kind, Libs)], code: &Libs) -> Vec<Candidate> {
    let mut out: Vec<Candidate> = Vec::new();
    let code = (!code.is_empty()).then_some(code);
    let all = opinions
        .iter()
        .map(|(kind, libs)| (*kind, libs))
        .chain(code.map(|c| (Kind::Code, c)));
    for (kind, libs) in all {
        let key = names(libs);
        let name = source_name(kind, libs);
        match out.iter_mut().find(|c| c.names() == key) {
            Some(c) => {
                c.kinds.push(kind);
                // The code comes last: the reasons kept are a list's.
                if !c.sources.contains(&name) {
                    c.sources.push(name);
                }
            }
            None => out.push(Candidate {
                libs: libs.clone(),
                kinds: vec![kind],
                sources: vec![name],
                union: false,
            }),
        }
    }
    if out.len() > 1 {
        let mut libs: Libs = Vec::new();
        for c in &out {
            for (l, r) in &c.libs {
                if !libs.iter().any(|(x, _)| x == l) {
                    libs.push((l.clone(), r.clone()));
                }
            }
        }
        if !out.iter().any(|c| names(&c.libs) == names(&libs)) {
            out.push(Candidate {
                libs,
                kinds: Vec::new(),
                sources: Vec::new(),
                union: true,
            });
        }
    }
    out.sort_by_key(Candidate::rank);
    out
}

/// The pick for a candidate: a list's opinion is kept as is when the
/// lists disagree with each other or with the code, or when a tool file
/// holds it (as tool files always were); otherwise inference runs, with
/// the list to fall back on.
fn pick(candidate: &Candidate, contested: bool, fallback: Option<&Libs>, mode: Mode) -> Pick {
    let tool = candidate.kinds.contains(&Kind::Tool);
    let shared = candidate.libs.len() > 1;
    if candidate.listed() && mode == Mode::Seeded && shared && !tool && !contested {
        Pick::Seeded(candidate.libs.clone())
    } else if candidate.union || (candidate.listed() && (tool || contested || mode != Mode::Ranked))
    {
        Pick::Listed(candidate.libs.clone())
    } else {
        Pick::Inferred(fallback.cloned())
    }
}

/// The pick for a file the decision has not seen (one created since): the
/// best ranked opinion of the lists, without weighing the code.
pub(crate) fn default_pick(opinions: &[(Kind, Libs)]) -> Pick {
    let all = candidates(opinions, &Vec::new());
    match all.first() {
        Some(c) if c.kinds.contains(&Kind::Tool) => Pick::Listed(c.libs.clone()),
        Some(c) => Pick::Inferred(Some(c.libs.clone())),
        None => Pick::Inferred(None),
    }
}

/// The explicit, fallback and seed libraries of each file for these picks.
pub(crate) fn apply(fixed: &[Option<Libs>], picks: &[Pick]) -> Assigned {
    let mut out = Assigned::default();
    for (fixed, pick) in fixed.iter().zip(picks) {
        let (explicit, fallback, seed) = match (fixed, pick) {
            (Some(f), _) => (Some(f.clone()), None, None),
            (None, Pick::Listed(l)) => (Some(l.clone()), None, None),
            (None, Pick::Inferred(fallback)) => (None, fallback.clone(), None),
            (None, Pick::Seeded(l)) => (None, None, Some(l.clone())),
        };
        out.explicit.push(explicit);
        out.fallback.push(fallback);
        out.seeds.push(seed);
    }
    out
}

/// The libraries [`infer_in`] starts from.
#[derive(Default)]
pub(crate) struct Assigned {
    pub explicit: Vec<Option<Libs>>,
    pub fallback: Vec<Option<Libs>>,
    pub seeds: Vec<Option<Libs>>,
}

/// Chooses each file's libraries among the opinions of the lists and the
/// code. `fixed` (`vfl.toml`) is never questioned. Returns the decision
/// and the libraries it gives.
///
/// 1. The code's opinion of each file is what its `lib.unit` references
///    demand ([`firm`]).
/// 2. From each starting point ([`Mode`]): the best ranked opinions (tool
///    files, then the code, then `vhdl_ls.toml`), the lists above the
///    code, and the same with files `vhdl_ls.toml` shares between
///    libraries open to more, the project is inferred; then each file the
///    opinions disagree about takes the one leaving the fewest library
///    conflicts around it (unresolved references to and from it, its units
///    declared twice in a library), for a few rounds, and the project is
///    inferred again.
/// 3. The result leaving the fewest conflicts in the whole project wins.
pub(crate) fn decide(
    paths: &[PathBuf],
    relative: &[String],
    summaries: &[FileSummary],
    fixed: &[Option<Libs>],
    opinions: &[Opinions],
) -> (Decision, Inference) {
    let n = paths.len();
    let dirs = Dirs::new(paths);
    let firm = firm(paths, &dirs, summaries, fixed);
    let all: Vec<Vec<Candidate>> = (0..n)
        .map(|i| {
            if fixed[i].is_some() {
                Vec::new()
            } else {
                candidates(&opinions[i], &firm[i])
            }
        })
        .collect();
    fn fallback_of(cands: &[Candidate]) -> Option<&Libs> {
        cands.iter().find(|c| c.listed()).map(|c| &c.libs)
    }
    let picks_for = |chosen: &[usize], mode: Mode| -> Vec<Pick> {
        (0..n)
            .map(|i| match all[i].get(chosen[i]) {
                Some(c) => pick(c, all[i].len() > 1, fallback_of(&all[i]), mode),
                None => Pick::Inferred(None),
            })
            .collect()
    };
    let run = |picks: &[Pick]| {
        let a = apply(fixed, picks);
        infer_in(paths, &dirs, summaries, &a.explicit, &a.fallback, &a.seeds)
    };

    let contested: Vec<usize> = (0..n).filter(|&i| all[i].len() > 1).collect();
    let index = (!contested.is_empty()).then(|| Index::new(summaries));
    let search = |mut chosen: Vec<usize>, mode: Mode| -> Searched {
        let picks = picks_for(&chosen, mode);
        let result = run(&picks);
        let left = library_conflicts(summaries, &result.libraries);
        let unscored = Searched {
            scores: vec![(0, 0); n],
            chosen: chosen.clone(),
            picks,
            result,
            left,
        };
        let Some(index) = index.as_ref().filter(|_| left > 0) else {
            return unscored;
        };
        let mut scores = vec![(0, 0); n];
        let mut libs = unscored.result.libraries.clone();
        for _round in 0..4 {
            let mut changed = false;
            for &i in &contested {
                let current = chosen[i];
                let around: Vec<usize> = all[i]
                    .iter()
                    .map(|c| {
                        let saved = std::mem::replace(&mut libs[i], c.libs.clone());
                        let left = index.around(i, &libs);
                        libs[i] = saved;
                        left
                    })
                    .collect();
                // On a tie, the current opinion stays.
                let best = (0..all[i].len())
                    .min_by_key(|&c| (around[c], c != current, c))
                    .unwrap_or(current);
                let others = (0..all[i].len())
                    .filter(|&c| c != best && all[i][c].listed())
                    .map(|c| around[c])
                    .min()
                    .unwrap_or(usize::MAX);
                scores[i] = (around[best], others);
                if best != current {
                    chosen[i] = best;
                    libs[i] = all[i][best].libs.clone();
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
        if chosen == unscored.chosen {
            return Searched { scores, ..unscored };
        }
        let picks = picks_for(&chosen, mode);
        let result = run(&picks);
        let left = library_conflicts(summaries, &result.libraries);
        if left <= unscored.left {
            Searched {
                chosen,
                picks,
                result,
                left,
                scores,
            }
        } else {
            unscored
        }
    };

    let ranked = search(vec![0; n], Mode::Ranked);
    let mut notes = Vec::new();
    // Lists first: `vhdl_ls.toml` above the code, as is.
    let vhdl_ls = all
        .iter()
        .any(|cands| cands.iter().any(|c| c.kinds.contains(&Kind::VhdlLs)));
    let best = if ranked.left > 0 && vhdl_ls {
        let lists_first: Vec<usize> = all
            .iter()
            .map(|cands| {
                let listed = |kind: Option<Kind>| {
                    cands.iter().position(|c| {
                        !c.union && c.listed() && kind.is_none_or(|k| c.kinds.contains(&k))
                    })
                };
                listed(Some(Kind::VhdlLs)).or(listed(None)).unwrap_or(0)
            })
            .collect();
        // Files the list puts in several libraries (shared by the VIPs of
        // UVVM) may be needed in libraries it does not list: open those
        // to more. That changes nothing when the list is right.
        let shared = all.iter().any(|cands| {
            cands
                .iter()
                .any(|c| c.libs.len() > 1 && c.kinds.iter().all(|k| *k == Kind::VhdlLs))
        });
        let listed = search(lists_first, if shared { Mode::Seeded } else { Mode::Fixed });
        if listed.left < ranked.left {
            let source = opinions
                .iter()
                .flatten()
                .find(|(k, _)| *k == Kind::VhdlLs)
                .map_or_else(|| "vhdl_ls.toml".to_owned(), |(k, l)| source_name(*k, l));
            notes.push(format!(
                "{source} ranks above library inference: it leaves {} library conflicts \
                 (unresolved references, units declared twice in a library), inference {}. \
                 Set [project] vhdl-ls = \"prefer\" to always rank it so.",
                listed.left, ranked.left
            ));
            listed
        } else {
            ranked
        }
    } else {
        ranked
    };

    // Report the files where an opinion of a list was set aside.
    let mut conflicts = Vec::new();
    for &i in &contested {
        let chosen = &all[i][best.chosen[i]];
        let set_aside = all[i]
            .iter()
            .enumerate()
            .any(|(k, o)| k != best.chosen[i] && o.listed() && !o.union);
        if !set_aside {
            continue;
        }
        let (left, rejected_left) = best.scores[i];
        conflicts.push(LibraryConflict {
            path: paths[i].clone(),
            opinions: all[i]
                .iter()
                .filter(|o| !o.union)
                .map(|o| (o.names(), o.sources.clone()))
                .collect(),
            chosen: chosen.names(),
            union: chosen.union,
            left,
            rejected_left,
        });
    }
    if !conflicts.is_empty() {
        notes.push(format!(
            "The library sources disagree about {} {}; vfl chose by the references of \
             the code (see project/library-conflict).",
            conflicts.len(),
            if conflicts.len() == 1 {
                "file"
            } else {
                "files"
            },
        ));
    }
    let decision = Decision {
        picks: relative.iter().cloned().zip(best.picks).collect(),
        conflicts,
        notes,
    };
    (decision, best.result)
}

/// What a search from one starting point found.
struct Searched {
    /// The opinion taken for each file, by index in its candidates.
    chosen: Vec<usize>,
    picks: Vec<Pick>,
    result: Inference,
    /// The library conflicts it leaves.
    left: usize,
    /// For each contested file, the conflicts around it with the opinion
    /// taken and the fewest with another list's.
    scores: Vec<(usize, usize)>,
}

/// The libraries the code demands of each file, as inference would put
/// it there first: a `lib.unit` reference meaning this file's unit, it
/// being the only one, or the only one closest to the reference. What
/// follows from `work` references, or from a unit several files as close
/// declare, depends on where the other files are, and does not stand
/// against a list.
fn firm(
    paths: &[PathBuf],
    dirs: &Dirs,
    summaries: &[FileSummary],
    fixed: &[Option<Libs>],
) -> Vec<Libs> {
    let mut declared: HashMap<&str, Vec<usize>> = HashMap::new();
    for (file, summary) in summaries.iter().enumerate() {
        for unit in summary.units.iter().filter(|u| u.kind.is_primary()) {
            let files = declared.entry(&unit.name).or_default();
            if !files.contains(&file) {
                files.push(file);
            }
        }
    }
    let has = |file: usize, lib: &str| {
        fixed[file]
            .as_ref()
            .is_some_and(|libs| libs.iter().any(|(l, _)| l == lib))
    };
    let mut out: Vec<Libs> = vec![Vec::new(); paths.len()];
    for (file, summary) in summaries.iter().enumerate() {
        for r in summary.refs.iter().filter(|r| r.library != "work") {
            let Some(files) = declared.get(r.unit.as_str()) else {
                continue;
            };
            if files.iter().any(|&c| has(c, &r.library)) {
                continue;
            }
            let open: Vec<usize> = files
                .iter()
                .copied()
                .filter(|&c| fixed[c].is_none())
                .collect();
            if let [only] = dirs.closest(file, &open)[..]
                && !out[only].iter().any(|(l, _)| *l == r.library)
            {
                out[only].push((
                    r.library.clone(),
                    Reason::Referenced {
                        by: paths[file].clone(),
                        unit: r.unit.clone(),
                    },
                ));
            }
        }
    }
    out
}

/// Who refers to what, to count the library conflicts around one file.
struct Index<'a> {
    summaries: &'a [FileSummary],
    /// Primary unit -> files declaring it.
    declared: HashMap<&'a str, Vec<usize>>,
    /// Primary unit -> references to it: file and index in its refs.
    users: HashMap<&'a str, Vec<(usize, usize)>>,
}

impl<'a> Index<'a> {
    fn new(summaries: &'a [FileSummary]) -> Self {
        let mut declared: HashMap<&str, Vec<usize>> = HashMap::new();
        for (file, summary) in summaries.iter().enumerate() {
            for unit in summary.units.iter().filter(|u| u.kind.is_primary()) {
                let files = declared.entry(unit.name.as_str()).or_default();
                if !files.contains(&file) {
                    files.push(file);
                }
            }
        }
        let mut users: HashMap<&str, Vec<(usize, usize)>> = HashMap::new();
        for (file, summary) in summaries.iter().enumerate() {
            for (k, r) in summary.refs.iter().enumerate() {
                if r.implicit
                    || BUNDLED_LIBRARIES.contains(&r.library.as_str())
                    || !declared.contains_key(r.unit.as_str())
                {
                    continue;
                }
                users.entry(r.unit.as_str()).or_default().push((file, k));
            }
        }
        Index {
            summaries,
            declared,
            users,
        }
    }

    /// The library conflicts involving file `i`, counted as
    /// [`library_conflicts`] does.
    fn around(&self, i: usize, libs: &[Libs]) -> usize {
        let in_library = |files: &[usize], lib: &str| {
            files.iter().any(|&f| libs[f].iter().any(|(l, _)| l == lib))
        };
        let mut refs: Vec<(usize, usize)> = Vec::new();
        for (k, _) in self.summaries[i].refs.iter().enumerate() {
            refs.push((i, k));
        }
        let mut total = 0;
        for unit in self.summaries[i]
            .units
            .iter()
            .filter(|u| u.kind.is_primary())
        {
            if let Some(users) = self.users.get(unit.name.as_str()) {
                refs.extend(users.iter().copied());
            }
            if let Some(files) = self.declared.get(unit.name.as_str()) {
                let mut seen: Vec<&str> = files
                    .iter()
                    .flat_map(|&f| libs[f].iter().map(|(l, _)| l.as_str()))
                    .collect();
                let all = seen.len();
                seen.sort_unstable();
                seen.dedup();
                total += all - seen.len();
            }
        }
        refs.sort_unstable();
        refs.dedup();
        for (file, k) in refs {
            let r = &self.summaries[file].refs[k];
            if r.implicit || BUNDLED_LIBRARIES.contains(&r.library.as_str()) {
                continue;
            }
            let Some(files) = self.declared.get(r.unit.as_str()) else {
                continue;
            };
            if r.library == "work" {
                total += libs[file]
                    .iter()
                    .filter(|(lib, _)| !in_library(files, lib))
                    .count();
            } else if !in_library(files, &r.library) {
                total += 1;
            }
        }
        total
    }
}

//! Whole-project analysis: indexes the design units of every file by
//! library, orders them so each unit is analyzed after the units it uses,
//! resolves names unit by unit (in parallel within a level), then runs the
//! checks that need several units at once.
//!
//! [`Analysis::update`] analyzes a new version of the same project and
//! reuses the results of every unit an edit cannot have affected.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use rayon::prelude::*;
use vfl_syntax::{GreenNode, LineIndex, Standard, SyntaxKind, SyntaxNode, TextRange, TextSize};

use crate::checks;
use crate::decl::{Decl, DeclId, DeclKind, FileId, Reference, Region, UnitId};
use crate::diagnostic::Diagnostic;
use crate::resolver::{ImportId, Resolver, UseItem};
use crate::stdlib;
use crate::syntax::{designator, simple_name_token};
use crate::typeck;

/// A file to analyze.
#[derive(Debug, Clone)]
pub struct FileInput {
    pub path: PathBuf,
    pub text: Arc<str>,
    pub green: GreenNode,
    /// The libraries the file belongs to (usually one).
    pub libraries: Vec<String>,
}

/// A file of an analysis.
#[derive(Debug, Clone)]
pub struct AnalyzedFile {
    pub path: PathBuf,
    pub text: Arc<str>,
    pub green: GreenNode,
    pub libraries: Vec<String>,
    /// A bundled `std` or `ieee` file.
    pub builtin: bool,
    pub line_index: LineIndex,
}

impl AnalyzedFile {
    pub fn syntax(&self) -> SyntaxNode {
        SyntaxNode::new_root(self.green.clone())
    }
}

/// A design unit, in one library.
#[derive(Debug, Clone)]
pub struct UnitInfo {
    pub file: FileId,
    /// Which `DESIGN_UNIT` of the file.
    pub index: usize,
    pub kind: DeclKind,
    pub name: String,
    /// An architecture's entity or a package body's package.
    pub primary: Option<String>,
    pub library: String,
    pub name_range: TextRange,
}

/// The analysis of one unit.
#[derive(Debug, Default, Clone)]
pub(crate) struct UnitResult {
    pub decls: Vec<Decl>,
    pub region: Arc<Region>,
    pub imports: Vec<crate::resolver::Import>,
    pub libraries: Vec<String>,
    pub refs: Vec<Reference>,
    pub diagnostics: Vec<Diagnostic>,
    pub use_items: Vec<UseItem>,
    pub used_imports: Vec<ImportId>,
    /// Operators whose visible declarations are unknown, in source order.
    pub opaque_ops: Vec<TextRange>,
}

/// Library name -> unit name -> primary units.
type LibraryIndex = HashMap<String, HashMap<String, Vec<UnitId>>>;

pub(crate) struct Ctx<'a> {
    pub files: &'a [AnalyzedFile],
    pub units: &'a [UnitInfo],
    pub libraries: &'a LibraryIndex,
    pub architectures: &'a HashMap<(String, String, String), UnitId>,
    pub results: &'a [OnceLock<Arc<UnitResult>>],
    pub standard: Option<UnitId>,
}

impl Ctx<'_> {
    pub fn standard_region(&self) -> Option<Arc<Region>> {
        self.standard
            .and_then(|u| self.result(u))
            .map(|r| r.region.clone())
    }

    pub fn result(&self, unit: UnitId) -> Option<&UnitResult> {
        self.results.get(unit as usize)?.get().map(|r| &**r)
    }

    pub fn decl(&self, id: DeclId) -> Option<&Decl> {
        self.result(id.unit)?.decls.get(id.index as usize)
    }

    pub fn library_known(&self, library: &str) -> bool {
        self.libraries.contains_key(library)
    }

    pub fn opaque_unit(&self, library: &str, unit: &str) -> bool {
        stdlib::OPAQUE_UNITS.contains(&(library, unit))
    }

    pub fn unit_decl_kind(&self, unit: UnitId) -> Option<DeclKind> {
        self.units.get(unit as usize).map(|u| u.kind)
    }

    /// The primary unit `name` of `library`; with several, the one whose
    /// file is closest to `from`.
    pub fn unit_in_library(&self, library: &str, name: &str, from: FileId) -> Option<UnitId> {
        let candidates = self.libraries.get(library)?.get(name)?;
        closest(self.files, self.units, candidates, from)
    }

    pub fn architecture(&self, library: &str, entity: &str, name: &str) -> Option<UnitId> {
        self.architectures
            .get(&(library.to_owned(), entity.to_owned(), name.to_owned()))
            .copied()
    }

    /// The one-based line of `offset` in `file`.
    pub fn line_of(&self, file: FileId, offset: TextSize) -> u32 {
        let file = &self.files[file as usize];
        file.line_index.line_col(&file.text, offset).line + 1
    }
}

fn closest(
    files: &[AnalyzedFile],
    units: &[UnitInfo],
    candidates: &[UnitId],
    from: FileId,
) -> Option<UnitId> {
    if candidates.len() <= 1 {
        return candidates.first().copied();
    }
    let here = files[from as usize].path.parent().unwrap_or(Path::new(""));
    candidates.iter().copied().max_by_key(|&u| {
        let there = files[units[u as usize].file as usize]
            .path
            .parent()
            .unwrap_or(Path::new(""));
        let shared = here
            .components()
            .zip(there.components())
            .take_while(|(a, b)| a == b)
            .count();
        // Prefer the same file, then the closest directory, then the most
        // similar file name, then the first declared.
        let same = units[u as usize].file == from;
        let name = |f: FileId| {
            files[f as usize]
                .path
                .file_stem()
                .map(|s| s.to_string_lossy().to_ascii_lowercase())
                .unwrap_or_default()
        };
        let (a, b) = (name(from), name(units[u as usize].file));
        let similar = a.bytes().zip(b.bytes()).take_while(|(x, y)| x == y).count();
        (same, shared, similar, std::cmp::Reverse(u))
    })
}

/// The result of analyzing a set of files.
pub struct Analysis {
    files: Vec<AnalyzedFile>,
    units: Vec<UnitInfo>,
    results: Vec<Arc<UnitResult>>,
    /// Per unit: a hash of what other units can see of it (its
    /// declarations without their positions, its region and imports).
    interfaces: Vec<u64>,
    /// Per unit: the other units its results refer to.
    uses: Vec<Vec<UnitId>>,
    /// Per file: its units with the names they mention.
    per_file: Vec<Arc<Vec<UnitWithDeps>>>,
    diagnostics: Vec<Diagnostic>,
    user_files: usize,
    /// Per unit: whether it was analyzed rather than reused.
    fresh: Vec<bool>,
    /// Whether `interfaces` and `uses` were computed, so that a later
    /// [`Analysis::update`] can reuse this analysis.
    incremental: bool,
}

impl std::fmt::Debug for Analysis {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Analysis")
            .field("files", &self.files.len())
            .field("units", &self.units.len())
            .field("diagnostics", &self.diagnostics.len())
            .finish()
    }
}

/// A bundled source: library, file name, text and tree.
type Bundled = (String, String, Arc<str>, GreenNode);

/// A unit with the `(library, unit)` names it mentions.
type UnitWithDeps = (UnitInfo, Vec<(String, String)>);

/// The bundled libraries, parsed once.
fn bundled() -> &'static [Bundled] {
    static BUNDLED: OnceLock<Vec<Bundled>> = OnceLock::new();
    BUNDLED.get_or_init(|| {
        stdlib::SOURCES
            .par_iter()
            .map(|(library, name, text)| {
                let parse = vfl_syntax::parse(text, Standard::Vhdl2019);
                (
                    library.to_string(),
                    name.to_string(),
                    Arc::<str>::from(*text),
                    parse.green().clone(),
                )
            })
            .collect()
    })
}

impl Analysis {
    /// Analyzes `files` together with the bundled `std` and `ieee` libraries.
    pub fn new(inputs: Vec<FileInput>) -> Analysis {
        Analysis::build(inputs, None)
    }

    /// Analyzes a new version of the project, reusing the results of the
    /// units that `self` analyzed and that the changes cannot affect: a unit
    /// is analyzed again when its file changed, or when a unit it uses was
    /// affected and what that unit shows to others (its declarations, apart
    /// from their positions) changed. The result is the same as
    /// [`Analysis::new`]'s. When files or design units were added, removed
    /// or renamed, or when `self` was built by [`Analysis::new`] (which
    /// skips the bookkeeping reuse needs), everything is analyzed again.
    pub fn update(&self, inputs: Vec<FileInput>) -> Analysis {
        Analysis::build(inputs, Some(self))
    }

    /// How many units were analyzed (not reused) to build this analysis.
    pub fn reanalyzed_units(&self) -> usize {
        self.fresh.iter().filter(|&&f| f).count()
    }

    /// Whether a unit of `file` was analyzed (not reused) to build this
    /// analysis: if not, its declarations are those of the previous one.
    pub fn file_reanalyzed(&self, file: FileId) -> bool {
        self.units
            .iter()
            .zip(&self.fresh)
            .any(|(u, &fresh)| u.file == file && fresh)
    }

    fn build(inputs: Vec<FileInput>, previous: Option<&Analysis>) -> Analysis {
        let user_files = inputs.len();
        // A file unchanged since the previous analysis (same tree) keeps
        // its line index and its units.
        let unchanged = |id: usize, path: &Path, green: &GreenNode| -> Option<usize> {
            let old = previous?.files.get(id)?;
            (old.path == path && std::ptr::eq(&*old.green, &**green)).then_some(id)
        };
        let mut files: Vec<AnalyzedFile> = inputs
            .into_par_iter()
            .enumerate()
            .map(|(id, input)| {
                let line_index = match unchanged(id, &input.path, &input.green) {
                    Some(id) => previous.expect("unchanged").files[id].line_index.clone(),
                    None => LineIndex::new(&input.text),
                };
                AnalyzedFile {
                    line_index,
                    path: input.path,
                    text: input.text,
                    green: input.green,
                    libraries: input.libraries,
                    builtin: false,
                }
            })
            .collect();
        for (library, name, text, green) in bundled() {
            let path = PathBuf::from(format!("<bundled>/{library}/{name}"));
            let line_index = match unchanged(files.len(), &path, green) {
                Some(id) => previous.expect("unchanged").files[id].line_index.clone(),
                None => LineIndex::new(text),
            };
            files.push(AnalyzedFile {
                path,
                line_index,
                text: text.clone(),
                green: green.clone(),
                libraries: vec![library.clone()],
                builtin: true,
            });
        }

        // Units, per file and library.
        let per_file: Vec<Arc<Vec<UnitWithDeps>>> = files
            .par_iter()
            .enumerate()
            .map(|(id, file)| {
                if let Some(p) = previous
                    && unchanged(id, &file.path, &file.green).is_some()
                    && p.files[id].libraries == file.libraries
                {
                    return p.per_file[id].clone();
                }
                Arc::new(file_units(id as FileId, file))
            })
            .collect();
        let mut units = Vec::new();
        let mut raw_deps = Vec::new();
        for file_units in &per_file {
            for (unit, deps) in file_units.iter() {
                units.push(unit.clone());
                raw_deps.push(deps);
            }
        }

        let mut libraries: LibraryIndex = HashMap::new();
        let mut architectures = HashMap::new();
        for (id, unit) in units.iter().enumerate() {
            match unit.kind {
                DeclKind::Architecture | DeclKind::PackageBody => {
                    if unit.kind == DeclKind::Architecture
                        && let Some(entity) = &unit.primary
                    {
                        architectures
                            .entry((unit.library.clone(), entity.clone(), unit.name.clone()))
                            .or_insert(id as UnitId);
                    }
                }
                _ => libraries
                    .entry(unit.library.clone())
                    .or_default()
                    .entry(unit.name.clone())
                    .or_default()
                    .push(id as UnitId),
            }
        }
        // Libraries named in the project but empty still exist.
        for file in &files {
            for library in &file.libraries {
                libraries.entry(library.clone()).or_default();
            }
        }

        let standard = units.iter().position(|u| {
            files[u.file as usize].builtin && u.library == "std" && u.name == "standard"
        });
        let standard = standard.map(|u| u as UnitId);

        let results: Vec<OnceLock<Arc<UnitResult>>> =
            (0..units.len()).map(|_| OnceLock::new()).collect();
        let ctx = Ctx {
            files: &files,
            units: &units,
            libraries: &libraries,
            architectures: &architectures,
            results: &results,
            standard,
        };

        // Dependencies between units.
        let deps: Vec<Vec<UnitId>> = raw_deps
            .iter()
            .enumerate()
            .map(|(id, deps)| {
                let unit = &units[id];
                let mut out: Vec<UnitId> = deps
                    .iter()
                    .filter_map(|(lib, name)| {
                        let lib = if lib == "work" { &unit.library } else { lib };
                        ctx.unit_in_library(lib, name, unit.file)
                    })
                    .filter(|&d| d != id as UnitId)
                    .collect();
                if let Some(primary) = &unit.primary
                    && let Some(p) = ctx.unit_in_library(&unit.library, primary, unit.file)
                {
                    out.push(p);
                }
                out.sort_unstable();
                out.dedup();
                out
            })
            .collect();

        let design_unit = |unit: UnitId| -> SyntaxNode {
            let info = &units[unit as usize];
            let root = files[info.file as usize].syntax();
            root.children()
                .filter(|n| n.kind() == SyntaxKind::DESIGN_UNIT)
                .nth(info.index)
                .expect("unit index is valid")
        };
        let analyze = |unit: UnitId| -> UnitResult {
            let result = Resolver::new(&ctx, unit).run(&design_unit(unit));
            checks::unit_checks(&ctx, unit, result)
        };

        // What can be reused: only with the same files and units, in the
        // same order, since results refer to units and files by index.
        // Only an analysis that may be updated later (the language server's)
        // keeps what reuse needs; a one-off analysis (the CLI's) skips it.
        let incremental = previous.is_some();
        let previous = previous.filter(|p| p.incremental && same_structure(p, &files, &units));
        let changed: Vec<bool> = match previous {
            Some(p) => files
                .iter()
                .zip(&p.files)
                .map(|(new, old)| !std::ptr::eq(&*new.green, &*old.green) && new.green != old.green)
                .collect(),
            None => vec![true; files.len()],
        };
        // A unit is affected when it was analyzed again and what it shows
        // changed, or when a unit it uses was affected.
        let mut affected = vec![false; units.len()];
        let mut fresh = vec![false; units.len()];
        let mut interfaces = vec![0u64; units.len()];
        // Decides whether `unit` must be analyzed again; reuses it if not.
        // Units are analyzed in the same order as in the previous analysis
        // (the order depends only on the units and their dependencies), so
        // a unit whose inputs did not change would get the same result, even
        // where a dependency cycle was broken.
        let reuse = |unit: UnitId, affected: &[bool]| -> bool {
            let Some(p) = previous else { return false };
            let u = unit as usize;
            if changed[units[u].file as usize] {
                return false;
            }
            let dirty = deps[u]
                .iter()
                .chain(&p.uses[u])
                .any(|&d| affected[d as usize]);
            if dirty {
                return false;
            }
            let _ = results[u].set(p.results[u].clone());
            true
        };

        let mut done = vec![false; units.len()];
        let mut remaining: Vec<UnitId> = (0..units.len() as UnitId).collect();
        if let Some(standard) = standard {
            let s = standard as usize;
            if reuse(standard, &affected) {
                interfaces[s] = previous.map_or(0, |p| p.interfaces[s]);
            } else {
                let result = analyze(standard);
                interfaces[s] = if incremental {
                    interface_hash(&result)
                } else {
                    0
                };
                affected[s] = previous.is_some_and(|p| p.interfaces[s] != interfaces[s]);
                fresh[s] = true;
                let _ = results[s].set(Arc::new(result));
            }
            done[s] = true;
            remaining.retain(|&u| u != standard);
        }
        while !remaining.is_empty() {
            let mut ready: Vec<UnitId> = remaining
                .iter()
                .copied()
                .filter(|&u| deps[u as usize].iter().all(|&d| done[d as usize]))
                .collect();
            if ready.is_empty() {
                // A dependency cycle: break it at the unit missing the fewest.
                let pick = remaining
                    .iter()
                    .copied()
                    .min_by_key(|&u| {
                        deps[u as usize]
                            .iter()
                            .filter(|&&d| !done[d as usize])
                            .count()
                    })
                    .expect("remaining is not empty");
                ready.push(pick);
            }
            let mut todo = Vec::new();
            for &u in &ready {
                if reuse(u, &affected) {
                    let p = previous.expect("reused from a previous analysis");
                    interfaces[u as usize] = p.interfaces[u as usize];
                } else {
                    todo.push(u);
                }
            }
            let analyzed: Vec<(UnitId, UnitResult, u64)> = todo
                .par_iter()
                .map(|&u| {
                    let result = analyze(u);
                    let interface = if incremental {
                        interface_hash(&result)
                    } else {
                        0
                    };
                    (u, result, interface)
                })
                .collect();
            for (unit, result, interface) in analyzed {
                let u = unit as usize;
                interfaces[u] = interface;
                // Without a previous analysis, nothing is reused, so
                // whether a unit is affected does not matter.
                affected[u] = previous.is_some_and(|p| p.interfaces[u] != interface);
                fresh[u] = true;
                let _ = results[u].set(Arc::new(result));
            }
            for &u in &ready {
                let u = u as usize;
                done[u] = true;
                // Units that use an affected unit are affected too, whatever
                // they show: their own dependents may read through them.
                if let Some(p) = previous
                    && fresh[u]
                    && deps[u]
                        .iter()
                        .chain(&p.uses[u])
                        .any(|&d| affected[d as usize])
                {
                    affected[u] = true;
                }
            }
            let ready: HashSet<UnitId> = ready.into_iter().collect();
            remaining.retain(|u| !ready.contains(u));
        }

        let mut diagnostics = checks::project_checks(&ctx);
        // Type checking needs only the declarations of the units a unit
        // depends on, and nothing depends on its outcome: it runs for every
        // unit at once, off the dependency order's critical path.
        let outcomes: Vec<Option<typeck::Outcome>> = (0..units.len() as UnitId)
            .into_par_iter()
            .map(|unit| {
                if !fresh[unit as usize] {
                    return None;
                }
                let result = ctx.result(unit)?;
                Some(typeck::check_unit(&ctx, unit, &design_unit(unit), result))
            })
            .collect();
        let previous_uses = previous.map(|p| &p.uses);
        let (results, uses): (Vec<Arc<UnitResult>>, Vec<Vec<UnitId>>) = results
            .into_iter()
            .zip(outcomes)
            .enumerate()
            .map(|(u, (result, outcome))| {
                let result = result.into_inner().unwrap_or_default();
                if !fresh[u] {
                    let uses = previous_uses.map(|p| p[u].clone()).unwrap_or_default();
                    return (result, uses);
                }
                let mut result = Arc::try_unwrap(result).unwrap_or_else(|r| (*r).clone());
                if let Some(outcome) = outcome {
                    outcome.apply(&mut result);
                }
                let uses = if incremental {
                    used_units(u as UnitId, &result)
                } else {
                    Vec::new()
                };
                (Arc::new(result), uses)
            })
            .unzip();
        for result in &results {
            diagnostics.extend(result.diagnostics.iter().cloned());
        }
        diagnostics.retain(|d| (d.file as usize) < user_files);
        // The message breaks ties, so the order does not depend on hash
        // map iteration (a unit in two libraries is reported twice).
        diagnostics.sort_by(|a, b| {
            (a.file, a.range.start(), a.code, &a.message).cmp(&(
                b.file,
                b.range.start(),
                b.code,
                &b.message,
            ))
        });
        diagnostics.dedup();

        Analysis {
            files,
            units,
            results,
            interfaces,
            uses,
            per_file,
            diagnostics,
            user_files,
            fresh,
            incremental,
        }
    }

    /// Every analyzed file: the inputs, in order, then the bundled ones.
    pub fn files(&self) -> &[AnalyzedFile] {
        &self.files
    }

    /// The inputs only.
    pub fn user_files(&self) -> &[AnalyzedFile] {
        &self.files[..self.user_files]
    }

    pub fn file_id(&self, path: &Path) -> Option<FileId> {
        self.files
            .iter()
            .position(|f| f.path == path)
            .map(|i| i as FileId)
    }

    pub fn units(&self) -> &[UnitInfo] {
        &self.units
    }

    /// All diagnostics of the input files, sorted by file and position.
    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    pub fn decl(&self, id: DeclId) -> Option<&Decl> {
        self.results
            .get(id.unit as usize)?
            .decls
            .get(id.index as usize)
    }

    /// The type a type or subtype mark denotes, through subtypes and
    /// incomplete type declarations.
    pub fn base_type(&self, mut mark: DeclId) -> Option<DeclId> {
        for _ in 0..32 {
            let decl = self.decl(mark)?;
            match decl.kind {
                DeclKind::Type | DeclKind::GenericType => return Some(mark),
                DeclKind::Subtype | DeclKind::IncompleteType => mark = decl.subtype?,
                _ => return None,
            }
        }
        None
    }

    /// The declarations of a unit; index 0 is the unit itself.
    pub fn unit_decls(&self, unit: UnitId) -> &[Decl] {
        &self.results[unit as usize].decls
    }

    /// The resolved references of a unit, in source order.
    pub fn unit_references(&self, unit: UnitId) -> &[Reference] {
        &self.results[unit as usize].refs
    }

    /// The units of a file.
    pub fn file_units(&self, file: FileId) -> impl Iterator<Item = UnitId> + '_ {
        self.units
            .iter()
            .enumerate()
            .filter(move |(_, u)| u.file == file)
            .map(|(i, _)| i as UnitId)
    }

    /// Every declaration, with its id.
    pub fn decls(&self) -> impl Iterator<Item = (DeclId, &Decl)> {
        self.results.iter().enumerate().flat_map(|(unit, result)| {
            result.decls.iter().enumerate().map(move |(index, decl)| {
                (
                    DeclId {
                        unit: unit as UnitId,
                        index: index as u32,
                    },
                    decl,
                )
            })
        })
    }

    /// A textual dump of everything the analysis found (declarations,
    /// references, diagnostics), in a stable order: for checking that an
    /// [`Analysis::update`] gives the same result as [`Analysis::new`].
    #[doc(hidden)]
    pub fn dump(&self) -> String {
        use std::fmt::Write;
        let mut out = String::new();
        for (id, decl) in self.decls() {
            let _ = writeln!(
                out,
                "decl {}:{} {} {:?} {}:{:?} {:?} {:?} {:?} {:?} {:?} {:?} {:?}",
                id.unit,
                id.index,
                decl.name,
                decl.kind,
                decl.file,
                decl.range,
                decl.full_range,
                decl.parent,
                decl.subtype,
                decl.type_def,
                decl.params,
                decl.width,
                decl.value,
            );
            if let Some(members) = &decl.members {
                let _ = writeln!(out, "  members {:?}", members.iter().collect::<Vec<_>>());
            }
        }
        for (file, r) in self.references() {
            let _ = writeln!(
                out,
                "ref {file}:{:?} {:?} {:?}",
                r.range, r.targets, r.access
            );
        }
        for d in &self.diagnostics {
            let _ = writeln!(
                out,
                "diag {}:{:?} {} {}",
                d.file, d.range, d.code, d.message
            );
        }
        out
    }

    /// Every reference, with the file it is in.
    pub fn references(&self) -> impl Iterator<Item = (FileId, &Reference)> {
        self.units
            .iter()
            .zip(&self.results)
            .flat_map(|(unit, result)| result.refs.iter().map(move |r| (unit.file, r)))
    }
}

/// The design units of a file, one per library it is in, with the
/// `(library, unit)` pairs each refers to.
fn file_units(file_id: FileId, file: &AnalyzedFile) -> Vec<UnitWithDeps> {
    let root = file.syntax();
    let mut out = Vec::new();
    for (index, design_unit) in root
        .children()
        .filter(|n| n.kind() == SyntaxKind::DESIGN_UNIT)
        .enumerate()
    {
        let Some(node) = design_unit
            .children()
            .find(|n| n.kind() != SyntaxKind::CONTEXT_CLAUSE)
        else {
            continue;
        };
        let kind = match node.kind() {
            SyntaxKind::ENTITY_DECL => DeclKind::Entity,
            SyntaxKind::ARCHITECTURE_BODY => DeclKind::Architecture,
            SyntaxKind::PACKAGE_DECL => DeclKind::Package,
            SyntaxKind::PACKAGE_BODY => DeclKind::PackageBody,
            SyntaxKind::PACKAGE_INSTANTIATION_DECL => DeclKind::PackageInstance,
            SyntaxKind::CONTEXT_DECL => DeclKind::Context,
            SyntaxKind::CONFIGURATION_DECL => DeclKind::Configuration,
            _ => continue,
        };
        let Some(name) = designator(&node) else {
            continue;
        };
        let primary = match kind {
            DeclKind::Architecture => node
                .children()
                .find(|n| n.kind() == SyntaxKind::NAME)
                .and_then(|n| simple_name_token(&n))
                .map(|t| vfl_syntax::identifier_key(t.text())),
            DeclKind::PackageBody => Some(vfl_syntax::identifier_key(name.text())),
            _ => None,
        };
        let deps = unit_dependencies(&design_unit);
        for library in &file.libraries {
            out.push((
                UnitInfo {
                    file: file_id,
                    index,
                    kind,
                    name: vfl_syntax::identifier_key(name.text()),
                    primary: primary.clone(),
                    library: library.clone(),
                    name_range: name.text_range(),
                },
                deps.clone(),
            ));
        }
    }
    out
}

/// The `library.unit` names a design unit mentions.
fn unit_dependencies(design_unit: &SyntaxNode) -> Vec<(String, String)> {
    let mut libraries: HashSet<String> = HashSet::from(["work".to_owned(), "std".to_owned()]);
    for clause in design_unit
        .descendants()
        .filter(|n| n.kind() == SyntaxKind::LIBRARY_CLAUSE)
    {
        for list in clause.children() {
            for token in crate::syntax::idents(&list) {
                libraries.insert(vfl_syntax::identifier_key(token.text()));
            }
        }
    }
    let mut deps = Vec::new();
    for node in design_unit
        .descendants()
        .filter(|n| n.kind() == SyntaxKind::SELECTED_NAME)
    {
        let Some(prefix) = node.children().next() else {
            continue;
        };
        let Some(library) =
            simple_name_token(&prefix).map(|t| vfl_syntax::identifier_key(t.text()))
        else {
            continue;
        };
        if !libraries.contains(&library) {
            continue;
        }
        if let Some(suffix) = node
            .children_with_tokens()
            .filter_map(|e| e.into_token())
            .find(|t| matches!(t.kind(), SyntaxKind::IDENT | SyntaxKind::EXT_IDENT))
        {
            deps.push((library, vfl_syntax::identifier_key(suffix.text())));
        }
    }
    deps.sort();
    deps.dedup();
    deps
}

/// Whether `files` and `units` are those of `previous`, in the same order,
/// texts aside.
fn same_structure(previous: &Analysis, files: &[AnalyzedFile], units: &[UnitInfo]) -> bool {
    previous.files.len() == files.len()
        && previous.units.len() == units.len()
        && previous
            .files
            .iter()
            .zip(files)
            .all(|(a, b)| a.path == b.path && a.libraries == b.libraries && a.builtin == b.builtin)
        && previous.units.iter().zip(units).all(|(a, b)| {
            a.file == b.file
                && a.index == b.index
                && a.kind == b.kind
                && a.name == b.name
                && a.primary == b.primary
                && a.library == b.library
        })
}

/// A hash of what other units can see of a unit: its declarations (but not
/// where they are), its region, and the imports and libraries its
/// secondary units inherit.
fn interface_hash(result: &UnitResult) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for decl in &result.decls {
        decl.name.hash(&mut h);
        decl.spelling.hash(&mut h);
        decl.kind.hash(&mut h);
        decl.parent.hash(&mut h);
        decl.subtype.hash(&mut h);
        decl.type_def.hash(&mut h);
        decl.params.hash(&mut h);
        decl.width.hash(&mut h);
        decl.value.hash(&mut h);
        match &decl.members {
            Some(region) => {
                1u8.hash(&mut h);
                region.hash_into(&mut h);
            }
            None => 0u8.hash(&mut h),
        }
    }
    result.region.hash_into(&mut h);
    for import in &result.imports {
        import.hash_into(&mut h);
    }
    result.libraries.hash(&mut h);
    h.finish()
}

/// The other units that a unit's results refer to.
fn used_units(unit: UnitId, result: &UnitResult) -> Vec<UnitId> {
    let mut out: Vec<UnitId> = Vec::new();
    let mut add = |id: DeclId| out.push(id.unit);
    for r in &result.refs {
        for t in &r.targets {
            if let crate::decl::Target::Decl(id) = t {
                add(*id);
            }
        }
    }
    for decl in &result.decls {
        decl.parent
            .into_iter()
            .chain(decl.subtype)
            .for_each(&mut add);
        decl.params
            .iter()
            .flat_map(|p| p.iter().copied())
            .for_each(&mut add);
        if let Some(region) = &decl.members {
            region.iter().for_each(&mut add);
        }
        if let Some(def) = &decl.type_def {
            def.decl_ids(&mut add);
        }
    }
    result.region.iter().for_each(&mut add);
    for import in &result.imports {
        import.decl_ids(&mut add);
    }
    out.sort_unstable();
    out.dedup();
    out.retain(|&u| u != unit);
    out
}

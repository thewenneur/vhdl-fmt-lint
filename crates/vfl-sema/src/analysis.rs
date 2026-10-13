//! Whole-project analysis: indexes the design units of every file by
//! library, orders them so each unit is analyzed after the units it uses,
//! resolves names unit by unit (in parallel within a level), then runs the
//! checks that need several units at once.

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
#[derive(Debug, Default)]
pub(crate) struct UnitResult {
    pub decls: Vec<Decl>,
    pub region: Arc<Region>,
    pub imports: Vec<crate::resolver::Import>,
    pub libraries: Vec<String>,
    pub refs: Vec<Reference>,
    pub diagnostics: Vec<Diagnostic>,
    pub use_items: Vec<UseItem>,
    pub used_imports: Vec<ImportId>,
}

/// Library name -> unit name -> primary units.
type LibraryIndex = HashMap<String, HashMap<String, Vec<UnitId>>>;

pub(crate) struct Ctx<'a> {
    pub files: &'a [AnalyzedFile],
    pub units: &'a [UnitInfo],
    pub libraries: &'a LibraryIndex,
    pub architectures: &'a HashMap<(String, String, String), UnitId>,
    pub results: &'a [OnceLock<UnitResult>],
    pub standard: Option<UnitId>,
}

impl Ctx<'_> {
    pub fn standard_region(&self) -> Option<Arc<Region>> {
        self.standard
            .and_then(|u| self.result(u))
            .map(|r| r.region.clone())
    }

    pub fn result(&self, unit: UnitId) -> Option<&UnitResult> {
        self.results.get(unit as usize)?.get()
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
    results: Vec<UnitResult>,
    diagnostics: Vec<Diagnostic>,
    user_files: usize,
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
        let user_files = inputs.len();
        let mut files: Vec<AnalyzedFile> = inputs
            .into_par_iter()
            .map(|input| AnalyzedFile {
                line_index: LineIndex::new(&input.text),
                path: input.path,
                text: input.text,
                green: input.green,
                libraries: input.libraries,
                builtin: false,
            })
            .collect();
        for (library, name, text, green) in bundled() {
            files.push(AnalyzedFile {
                path: PathBuf::from(format!("<bundled>/{library}/{name}")),
                line_index: LineIndex::new(text),
                text: text.clone(),
                green: green.clone(),
                libraries: vec![library.clone()],
                builtin: true,
            });
        }

        // Units, per file and library.
        let per_file: Vec<Vec<UnitWithDeps>> = files
            .par_iter()
            .enumerate()
            .map(|(id, file)| file_units(id as FileId, file))
            .collect();
        let mut units = Vec::new();
        let mut raw_deps = Vec::new();
        for file_units in per_file {
            for (unit, deps) in file_units {
                units.push(unit);
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

        let results: Vec<OnceLock<UnitResult>> =
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

        let analyze = |unit: UnitId| -> UnitResult {
            let info = &units[unit as usize];
            let root = files[info.file as usize].syntax();
            let design_unit = root
                .children()
                .filter(|n| n.kind() == SyntaxKind::DESIGN_UNIT)
                .nth(info.index)
                .expect("unit index is valid");
            let result = Resolver::new(&ctx, unit).run(&design_unit);
            checks::unit_checks(&ctx, unit, result)
        };

        let mut done = vec![false; units.len()];
        let mut remaining: Vec<UnitId> = (0..units.len() as UnitId).collect();
        if let Some(standard) = standard {
            let _ = results[standard as usize].set(analyze(standard));
            done[standard as usize] = true;
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
            let analyzed: Vec<(UnitId, UnitResult)> =
                ready.par_iter().map(|&u| (u, analyze(u))).collect();
            for (unit, result) in analyzed {
                let _ = results[unit as usize].set(result);
                done[unit as usize] = true;
            }
            let ready: HashSet<UnitId> = ready.into_iter().collect();
            remaining.retain(|u| !ready.contains(u));
        }

        let mut diagnostics = checks::project_checks(&ctx);
        let results: Vec<UnitResult> = results
            .into_iter()
            .map(|r| r.into_inner().unwrap_or_default())
            .collect();
        for result in &results {
            diagnostics.extend(result.diagnostics.iter().cloned());
        }
        diagnostics.retain(|d| (d.file as usize) < user_files);
        diagnostics.sort_by(|a, b| {
            (a.file, a.range.start(), a.code).cmp(&(b.file, b.range.start(), b.code))
        });
        diagnostics.dedup();

        Analysis {
            files,
            units,
            results,
            diagnostics,
            user_files,
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

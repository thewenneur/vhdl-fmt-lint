//! The language server's editor features at byte offsets, for editors that
//! link vfl directly instead of speaking LSP, like the Notepad++ plugin:
//! hover, signature help, completion, go to definition, references,
//! highlights, outline, project symbols, rename, quick fixes, the problems
//! of the whole project and semantic tokens.
//!
//! Each query brings the analysis up to date first, then answers from the
//! same code as the language server.

use std::ops::Range;
use std::path::{Path, PathBuf};

use lsp_types::{CompletionItemKind, ParameterLabel};
use vfl_sema::FileId;
use vfl_syntax::TextSize;

use crate::world::World;

pub use crate::semantic::{Kind as TokenKind, modifier as token_modifier};

/// What a hover shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HoverInfo {
    /// The byte range of the name the hover is about.
    pub range: Range<usize>,
    /// The text, in Markdown: kind, name, type, then the declaration in a
    /// fenced `vhdl` block. Several declarations are separated by `---`.
    pub markdown: String,
}

/// One candidate signature of a call or instantiation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Signature {
    /// `function f(a : integer; b : bit) return integer`.
    pub label: String,
    /// Where each parameter is in `label`, in bytes.
    pub parameters: Vec<Range<usize>>,
    /// The parameter the cursor is in. `None` when past the last one.
    pub active_parameter: Option<usize>,
}

/// The signatures of the call around the cursor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignatureHelp {
    pub signatures: Vec<Signature>,
    /// The signature that fits the arguments so far.
    pub active: usize,
}

/// What a completion is, roughly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CompletionKind {
    Keyword,
    /// A library, package, context or configuration.
    Module,
    /// An entity, component or architecture.
    Unit,
    Type,
    Function,
    Procedure,
    /// A signal, variable, constant or generic.
    Object,
    /// A record element, port or parameter.
    Field,
    /// An enumeration literal or physical unit.
    Literal,
    Other,
}

/// A completion candidate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Completion {
    pub label: String,
    pub kind: CompletionKind,
    /// What it is, or its type or signature. May be empty.
    pub detail: String,
}

/// The completions at the cursor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Completions {
    /// Where the word being completed starts, in bytes. The candidates
    /// replace the text from here to the cursor.
    pub start: usize,
    pub items: Vec<Completion>,
}

/// The analyzed file at `path` and `offset` as a valid position in it.
fn locate(world: &mut World, path: &Path, offset: usize) -> Option<(FileId, TextSize)> {
    world.refresh();
    let file = world.file_id(path)?;
    let text = world.doc(file).text;
    let mut offset = offset.min(text.len());
    while !text.is_char_boundary(offset) {
        offset -= 1;
    }
    Some((file, TextSize::from(offset as u32)))
}

/// A name colored by what it resolves to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemanticToken {
    pub range: Range<usize>,
    pub kind: TokenKind,
    /// [`token_modifier`] bits.
    pub modifiers: u32,
}

/// The names of the file at `path` with what they resolve to, in order.
pub fn semantic_tokens(world: &mut World, path: &Path) -> Vec<SemanticToken> {
    let Some((file, _)) = locate(world, path, 0) else {
        return Vec::new();
    };
    crate::semantic::tokens(world, file, None)
        .into_iter()
        .map(|t| SemanticToken {
            range: t.range.into(),
            kind: t.kind,
            modifiers: t.modifiers,
        })
        .collect()
}

/// The hover at byte `offset` of the file at `path`.
pub fn hover(world: &mut World, path: &Path, offset: usize) -> Option<HoverInfo> {
    let (file, offset) = locate(world, path, offset)?;
    let (range, markdown) = crate::handlers::hover_at(world, file, offset)?;
    Some(HoverInfo {
        range: range.into(),
        markdown,
    })
}

/// The signatures of the subprogram call, or of the port or generic map,
/// around byte `offset` of the file at `path`.
pub fn signature_help(world: &mut World, path: &Path, offset: usize) -> Option<SignatureHelp> {
    let (file, offset) = locate(world, path, offset)?;
    let help = crate::types::signature_help_at(world, file, offset)?;
    let signatures = help
        .signatures
        .into_iter()
        .map(|s| {
            let parameters: Vec<Range<usize>> = s
                .parameters
                .unwrap_or_default()
                .iter()
                .filter_map(|p| match p.label {
                    ParameterLabel::LabelOffsets([start, end]) => Some(
                        utf16_to_byte(&s.label, start as usize)
                            ..utf16_to_byte(&s.label, end as usize),
                    ),
                    ParameterLabel::Simple(_) => None,
                })
                .collect();
            let active_parameter = s
                .active_parameter
                .map(|p| p as usize)
                .filter(|&p| p < parameters.len());
            Signature {
                label: s.label,
                parameters,
                active_parameter,
            }
        })
        .collect();
    Some(SignatureHelp {
        signatures,
        active: help.active_signature.unwrap_or(0) as usize,
    })
}

/// The completions at byte `offset` of the file at `path`: what follows
/// `prefix.` after a dot, otherwise what is visible there and keywords.
pub fn completion(world: &mut World, path: &Path, offset: usize) -> Option<Completions> {
    let (file, offset) = locate(world, path, offset)?;
    let text = world.doc(file).text;
    let before = &text[..usize::from(offset)];
    let start = before
        .trim_end_matches(|c: char| c.is_alphanumeric() || c == '_')
        .len();
    let items = crate::handlers::completion_at(world, file, offset)
        .into_iter()
        .map(|item| Completion {
            kind: item.kind.map_or(CompletionKind::Other, kind),
            detail: item.detail.unwrap_or_default(),
            label: item.label,
        })
        .collect();
    Some(Completions { start, items })
}

fn kind(kind: CompletionItemKind) -> CompletionKind {
    match kind {
        CompletionItemKind::KEYWORD => CompletionKind::Keyword,
        CompletionItemKind::MODULE => CompletionKind::Module,
        CompletionItemKind::INTERFACE | CompletionItemKind::CLASS => CompletionKind::Unit,
        CompletionItemKind::STRUCT => CompletionKind::Type,
        CompletionItemKind::FUNCTION => CompletionKind::Function,
        CompletionItemKind::METHOD => CompletionKind::Procedure,
        CompletionItemKind::VARIABLE | CompletionItemKind::CONSTANT => CompletionKind::Object,
        CompletionItemKind::FIELD => CompletionKind::Field,
        CompletionItemKind::ENUM_MEMBER | CompletionItemKind::UNIT => CompletionKind::Literal,
        _ => CompletionKind::Other,
    }
}

/// The byte offset of UTF-16 offset `units` in `text`.
fn utf16_to_byte(text: &str, units: usize) -> usize {
    let mut seen = 0;
    for (i, c) in text.char_indices() {
        if seen >= units {
            return i;
        }
        seen += c.len_utf16();
    }
    text.len()
}

/// A place in a file. `text` is the file's text as analyzed, which the
/// range indexes; it may differ from the file on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Location {
    /// A file an editor can open: bundled library sources are written to
    /// a cache directory first.
    pub path: PathBuf,
    pub range: Range<usize>,
    /// The 1-based line of `range.start`.
    pub line: u32,
    /// The text of that line, trimmed.
    pub line_text: String,
}

fn location(world: &World, file: FileId, range: vfl_syntax::TextRange) -> Option<Location> {
    let path = crate::handlers::openable_path(world, file)?;
    let doc = world.doc(file);
    let line = doc.index.line_col(doc.text, range.start()).line;
    let start = usize::from(doc.index.line_start(line).unwrap_or_default());
    let end = doc.text[start..]
        .find('\n')
        .map_or(doc.text.len(), |i| start + i);
    Some(Location {
        path,
        range: range.into(),
        line: line + 1,
        line_text: doc.text[start..end].trim().to_owned(),
    })
}

/// Where what is named at byte `offset` is declared.
pub fn definition(world: &mut World, path: &Path, offset: usize) -> Vec<Location> {
    let Some((file, offset)) = locate(world, path, offset) else {
        return Vec::new();
    };
    crate::handlers::definitions(world, file, offset)
        .into_iter()
        .filter_map(|(file, range)| location(world, file, range))
        .collect()
}

/// Every place in the project that names what is named at byte `offset`,
/// its declarations included, in file order.
pub fn references(world: &mut World, path: &Path, offset: usize) -> Vec<Location> {
    let Some((file, offset)) = locate(world, path, offset) else {
        return Vec::new();
    };
    let Some((_, symbol)) = crate::refactor::symbol_at(world, file, offset, None) else {
        return Vec::new();
    };
    crate::refactor::occurrences(world, &symbol, true, None)
        .into_iter()
        .filter(|(f, _, _)| !world.analysis.files()[*f as usize].builtin)
        .filter_map(|(f, range, _)| location(world, f, range))
        .collect()
}

/// How a highlighted name is used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Use {
    Read,
    Write,
    /// Declared, or named without reading or writing a value.
    Other,
}

/// The places in the file at `path` that name what is named at byte
/// `offset`, and how each uses it.
pub fn highlights(world: &mut World, path: &Path, offset: usize) -> Vec<(Range<usize>, Use)> {
    let Some((file, offset)) = locate(world, path, offset) else {
        return Vec::new();
    };
    let Some((_, symbol)) = crate::refactor::symbol_at(world, file, offset, Some(file)) else {
        return Vec::new();
    };
    crate::refactor::occurrences(world, &symbol, true, Some(file))
        .into_iter()
        .filter(|(f, _, _)| *f == file)
        .map(|(_, range, access)| {
            let kind = if access.writes() {
                Use::Write
            } else if access.reads() {
                Use::Read
            } else {
                Use::Other
            };
            (range.into(), kind)
        })
        .collect()
}

/// A declaration in a file's outline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Symbol {
    pub name: String,
    /// What it is: `entity`, `signal`, `function`...
    pub kind: &'static str,
    /// How deeply it is nested in other listed declarations.
    pub depth: usize,
    /// The range of its name.
    pub range: Range<usize>,
}

/// A problem somewhere in the project, as the language server publishes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectProblem {
    pub location: Location,
    pub level: vfl_config::Level,
    pub rule: &'static str,
    pub message: String,
}

/// The problems of every file of the project, by path and position, like
/// the language server publishes them.
pub fn problems(world: &mut World) -> Vec<ProjectProblem> {
    world.refresh();
    let mut out: Vec<ProjectProblem> = world
        .lint
        .iter()
        .filter(|d| d.level != vfl_config::Level::Off)
        .filter_map(|d| {
            Some(ProjectProblem {
                location: location(world, d.file, d.range)?,
                level: d.level,
                rule: d.rule,
                message: d.message.clone(),
            })
        })
        .collect();
    out.sort_by(|a, b| {
        (&a.location.path, a.location.range.start).cmp(&(&b.location.path, b.location.range.start))
    });
    out
}

/// The declarations of the file at `path`, in order, nested as in the
/// language server's document symbols.
pub fn outline(world: &mut World, path: &Path) -> Vec<Symbol> {
    world.refresh();
    let Some(file) = world.file_id(path) else {
        return Vec::new();
    };
    let (decls, roots, children) = crate::handlers::outline(world, file);
    let mut out = Vec::new();
    let mut stack: Vec<(usize, usize)> = roots.iter().rev().map(|&i| (i, 0)).collect();
    while let Some((i, depth)) = stack.pop() {
        let decl = decls[i].1;
        out.push(Symbol {
            name: decl.spelling.clone(),
            kind: decl.kind.describe(),
            depth,
            range: decl.range.into(),
        });
        stack.extend(children[i].iter().rev().map(|&c| (c, depth + 1)));
    }
    out
}

/// A declaration found by [`project_symbols`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectSymbol {
    pub name: String,
    pub kind: &'static str,
    /// The design unit it is in, unless it is one.
    pub container: Option<String>,
    pub location: Location,
}

/// The project's design units, types, subprograms and package objects
/// whose name contains `query`, ignoring case.
pub fn project_symbols(world: &mut World, query: &str) -> Vec<ProjectSymbol> {
    world.refresh();
    crate::handlers::workspace_decls(world, query)
        .into_iter()
        .filter_map(|id| {
            let decl = world.analysis.decl(id)?;
            Some(ProjectSymbol {
                name: decl.spelling.clone(),
                kind: decl.kind.describe(),
                container: (id.index != 0)
                    .then(|| world.analysis.unit_decls(id.unit)[0].spelling.clone()),
                location: location(world, decl.file, decl.range)?,
            })
        })
        .collect()
}

/// The changes to one file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileEdits {
    pub path: PathBuf,
    /// The file's text the ranges index, as analyzed.
    pub text: String,
    /// Non-overlapping replacements, in order.
    pub edits: Vec<(Range<usize>, String)>,
}

/// The name a rename at byte `offset` would change, or why it cannot.
pub fn prepare_rename(
    world: &mut World,
    path: &Path,
    offset: usize,
) -> Result<Range<usize>, String> {
    let (file, offset) = locate(world, path, offset).ok_or("this file is not analyzed")?;
    Ok(crate::handlers::rename_targets_at(world, file, offset)?
        .0
        .into())
}

/// The edits renaming what is named at byte `offset` to `new_name`,
/// across the project, by file.
pub fn rename(
    world: &mut World,
    path: &Path,
    offset: usize,
    new_name: &str,
) -> Result<Vec<FileEdits>, String> {
    let new_name = new_name.trim();
    let (file, offset) = locate(world, path, offset).ok_or("this file is not analyzed")?;
    let (_, symbol) = crate::handlers::rename_targets_at(world, file, offset)?;
    let edits = crate::handlers::rename_edits(world, &symbol, new_name)?;
    Ok(file_edits(world, edits))
}

/// `edits` grouped by file, in order.
fn file_edits(
    world: &World,
    edits: impl IntoIterator<Item = (FileId, vfl_syntax::TextRange, String)>,
) -> Vec<FileEdits> {
    let mut out: Vec<FileEdits> = Vec::new();
    for (f, range, text) in edits {
        let analyzed = &world.analysis.files()[f as usize];
        let i = match out.iter().position(|e| e.path == analyzed.path) {
            Some(i) => i,
            None => {
                out.push(FileEdits {
                    path: analyzed.path.clone(),
                    text: analyzed.text.to_string(),
                    edits: Vec::new(),
                });
                out.len() - 1
            }
        };
        out[i].edits.push((range.into(), text));
    }
    // A file analyzed in two libraries names the same places twice.
    for file in &mut out {
        file.edits.sort_by_key(|(r, _)| (r.start, r.end));
        file.edits.dedup_by(|a, b| a.0 == b.0);
    }
    out
}

/// A fix for a problem.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuickFix {
    pub title: String,
    /// Whether it keeps the design's behavior.
    pub safe: bool,
    /// The rule of the problem it fixes.
    pub rule: &'static str,
    /// The edits to the file, unless it is a rename.
    pub edits: Vec<(Range<usize>, String)>,
    /// For a name that breaks a naming convention: the name to rename it to
    /// everywhere, with [`rename`] at byte `at`.
    pub rename: Option<String>,
    /// Where the problem starts.
    pub at: usize,
    /// Whether it adds a `-- vfl-ignore` comment rather than fixing the
    /// problem.
    pub silences: bool,
}

/// The fixes of the problems touching bytes `start..end` of the file at
/// `path`, as of the last lint: the rules' own fixes, renames that follow
/// naming conventions, `use` clauses for undeclared names, then comments
/// that silence the rules.
pub fn quick_fixes(world: &mut World, path: &Path, start: usize, end: usize) -> Vec<QuickFix> {
    world.refresh();
    let Some(file) = world.file_id(path) else {
        return Vec::new();
    };
    let at = |offset: usize| TextSize::from(offset.min(u32::MAX as usize) as u32);
    crate::actions::actions(world, file, at(start), at(end))
        .into_iter()
        .map(|action| QuickFix {
            title: action.title,
            safe: action.safe,
            rule: world.lint[action.problem].rule,
            edits: if action.rename.is_some() {
                Vec::new()
            } else {
                action
                    .edits
                    .into_iter()
                    .map(|(_, range, text)| (range.into(), text))
                    .collect()
            },
            rename: action.rename,
            at: world.lint[action.problem].range.start().into(),
            silences: action.silences,
        })
        .collect()
}

/// The text of the file at `path` with every safe fix applied that does
/// not overlap another, and how many were. `None` when there is none.
pub fn fix_all(world: &mut World, path: &Path) -> Option<(String, usize)> {
    world.refresh();
    let file = world.file_id(path)?;
    let text = world.doc(file).text;
    let fixes = crate::handlers::compatible_fixes(
        text,
        world
            .lint
            .iter()
            .filter(|d| d.file == file)
            .filter_map(|d| d.fix.as_ref())
            .filter(|f| f.safe),
    );
    if fixes.is_empty() {
        return None;
    }
    Some(vfl_lint::apply_fixes(text, fixes))
}

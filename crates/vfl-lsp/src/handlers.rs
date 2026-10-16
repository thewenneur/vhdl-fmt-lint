//! Request handlers.

use std::path::{Path, PathBuf};

use lsp_types::{
    CodeAction, CodeActionKind, CodeActionOrCommand, CodeActionParams, CompletionItem,
    CompletionItemKind, CompletionParams, CompletionResponse, DocumentFormattingParams,
    DocumentHighlight, DocumentHighlightKind, DocumentHighlightParams,
    DocumentOnTypeFormattingParams, DocumentRangeFormattingParams, DocumentSymbol,
    DocumentSymbolParams, DocumentSymbolResponse, FoldingRange, FoldingRangeKind,
    FoldingRangeParams, GotoDefinitionParams, GotoDefinitionResponse, Hover, HoverContents,
    HoverParams, Location, MarkupContent, MarkupKind, NumberOrString, PrepareRenameResponse,
    ReferenceParams, RenameParams, SymbolInformation, SymbolKind, TextDocumentPositionParams,
    TextEdit, Url, WorkspaceEdit, WorkspaceSymbolParams, WorkspaceSymbolResponse,
};
use rustc_hash::{FxHashMap, FxHashSet};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use vfl_config::Level;
use vfl_sema::{Access, Decl, DeclId, DeclKind, FileId, Target};
use vfl_syntax::{LineIndex, TextRange, TextSize};

use crate::refactor::{self, Symbol};
use crate::world::{Doc, World};

pub enum Error {
    Unknown,
    Invalid(String),
    Failed(String),
}

fn parse<P: DeserializeOwned>(value: Value) -> Result<P, Error> {
    serde_json::from_value(value).map_err(|e| Error::Invalid(e.to_string()))
}

fn json<T: Serialize>(value: T) -> Result<Value, Error> {
    serde_json::to_value(value).map_err(|e| Error::Failed(e.to_string()))
}

/// Requests answered from the documents' text alone, without bringing the
/// analysis up to date first.
pub fn needs_analysis(method: &str) -> bool {
    !matches!(
        method,
        "textDocument/formatting"
            | "textDocument/rangeFormatting"
            | "textDocument/onTypeFormatting"
            | "textDocument/foldingRange"
    )
}

pub fn request(world: &mut World, method: &str, params: Value) -> Result<Value, Error> {
    match method {
        "textDocument/formatting" => json(formatting(world, parse(params)?)),
        "textDocument/rangeFormatting" => json(range_formatting(world, parse(params)?)),
        "textDocument/onTypeFormatting" => json(on_type_formatting(world, parse(params)?)),
        "textDocument/definition" => json(definition(world, parse(params)?)),
        "textDocument/references" => json(references(world, parse(params)?)),
        "textDocument/documentHighlight" => json(highlight(world, parse(params)?)),
        "textDocument/hover" => json(hover(world, parse(params)?)),
        "textDocument/documentSymbol" => json(document_symbols(world, parse(params)?)),
        "textDocument/foldingRange" => json(folding_ranges(world, parse(params)?)),
        "workspace/symbol" => json(workspace_symbols(world, parse(params)?)),
        "textDocument/prepareRename" => json(prepare_rename(world, parse(params)?)?),
        "textDocument/rename" => json(rename(world, parse(params)?)?),
        "textDocument/completion" => json(completion(world, parse(params)?)),
        "textDocument/codeAction" => json(code_actions(world, parse(params)?)),
        "textDocument/signatureHelp" => json(crate::types::signature_help(world, parse(params)?)),
        "textDocument/semanticTokens/full" => json(crate::semantic::full(world, parse(params)?)),
        "textDocument/semanticTokens/full/delta" => {
            json(crate::semantic::delta(world, parse(params)?))
        }
        "textDocument/semanticTokens/range" => json(crate::semantic::range(world, parse(params)?)),
        "codeAction/resolve" => {
            let action: CodeAction = parse(params)?;
            let data = action.data.clone().unwrap_or_default();
            if let Ok(rename) = serde_json::from_value(data.clone()) {
                json(resolve_rename(world, action, rename).map_err(Error::Failed)?)
            } else if let Ok(associate) = serde_json::from_value(data.clone()) {
                json(resolve_associate(world, action, associate).map_err(Error::Failed)?)
            } else if let Ok(config) = serde_json::from_value(data) {
                json(crate::settings::resolve(world, action, config).map_err(Error::Failed)?)
            } else {
                json(crate::generate::resolve(world, action).map_err(Error::Failed)?)
            }
        }
        "textDocument/implementation" => {
            json(crate::navigate::implementation(world, parse(params)?))
        }
        "textDocument/typeDefinition" => {
            json(crate::navigate::type_definition(world, parse(params)?))
        }
        "textDocument/declaration" => json(crate::navigate::declaration(world, parse(params)?)),
        "textDocument/linkedEditingRange" => {
            json(crate::navigate::linked_editing(world, parse(params)?))
        }
        "textDocument/selectionRange" => {
            json(crate::navigate::selection_range(world, parse(params)?))
        }
        "textDocument/inlayHint" => json(crate::inlay::inlay_hints(world, parse(params)?)),
        "textDocument/codeLens" => json(crate::hierarchy::code_lens(world, parse(params)?)),
        "codeLens/resolve" => json(crate::hierarchy::resolve_code_lens(world, parse(params)?)),
        "textDocument/prepareCallHierarchy" => {
            json(crate::hierarchy::prepare(world, parse(params)?))
        }
        "callHierarchy/incomingCalls" => json(crate::hierarchy::incoming(world, parse(params)?)),
        "callHierarchy/outgoingCalls" => json(crate::hierarchy::outgoing(world, parse(params)?)),
        "vfl/entities" => json(crate::generate::entities(world)),
        "vfl/projectConfig" => json(crate::settings::project_config(world).map_err(Error::Failed)?),
        "vfl/hierarchy" => {
            let params = if params.is_null() {
                Default::default()
            } else {
                parse(params)?
            };
            json(crate::design_tree::request(world, params))
        }
        "vfl/generate" => {
            json(crate::generate::generate_request(world, parse(params)?).map_err(Error::Failed)?)
        }
        _ => Err(Error::Unknown),
    }
}

fn file_of(world: &World, uri: &Url) -> Option<FileId> {
    world.file_id(&uri.to_file_path().ok()?)
}

fn uri(path: &Path) -> Option<Url> {
    Url::from_file_path(path).ok()
}

/// A problem as an LSP diagnostic.
fn lsp_diagnostic(world: &World, d: &vfl_lint::Diagnostic) -> lsp_types::Diagnostic {
    let tags = d
        .rule
        .starts_with("sema/unused-")
        .then(|| vec![lsp_types::DiagnosticTag::UNNECESSARY]);
    lsp_types::Diagnostic {
        range: world.doc(d.file).range(d.range),
        severity: Some(match d.level {
            Level::Error => lsp_types::DiagnosticSeverity::ERROR,
            Level::Warn => lsp_types::DiagnosticSeverity::WARNING,
            _ => lsp_types::DiagnosticSeverity::INFORMATION,
        }),
        code: Some(NumberOrString::String(d.rule.into())),
        source: Some("vfl".into()),
        message: d.message.clone(),
        tags,
        ..Default::default()
    }
}

/// The diagnostics of every user file that has some, Verilog files
/// included.
pub fn diagnostics(world: &World) -> Vec<(Url, Vec<lsp_types::Diagnostic>)> {
    let mut by_file: FxHashMap<FileId, Vec<lsp_types::Diagnostic>> = FxHashMap::default();
    for d in &world.lint {
        by_file
            .entry(d.file)
            .or_default()
            .push(lsp_diagnostic(world, d));
    }
    let mut out: Vec<(Url, Vec<lsp_types::Diagnostic>)> = by_file
        .into_iter()
        .filter_map(|(file, diagnostics)| {
            Some((
                uri(&world.analysis.files()[file as usize].path)?,
                diagnostics,
            ))
        })
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// The latest text of the document at `path` and its config. Formatting
/// reads it instead of the analysis, so it never waits for one.
fn latest(world: &mut World, path: &Path) -> Option<(vfl_config::Config, String)> {
    let config = world.config(path).clone();
    let text = match world.overlays.get(path) {
        Some(text) => text.clone(),
        None => match crate::verilog::file(world, path) {
            Some(file) => file.text.to_string(),
            None => world.doc(world.file_id(path)?).text.to_owned(),
        },
    };
    Some((config, text))
}

fn formatting(world: &mut World, params: DocumentFormattingParams) -> Option<Vec<TextEdit>> {
    let path = params.text_document.uri.to_file_path().ok()?;
    let (config, text) = latest(world, &path)?;
    let index = LineIndex::new(&text);
    let doc = Doc {
        text: &text,
        index: &index,
    };
    let root = || world.root.dir.clone();
    let formatted = crate::format::document(Some(&path), doc.text, &config, root).ok()?;
    if formatted == doc.text {
        return Some(Vec::new());
    }
    // One edit covering what changed, so the cursor keeps its place.
    let edit = vfl_fmt::minimal_edit(doc.text, &formatted);
    let range = TextRange::new(
        TextSize::from(edit.range.start as u32),
        TextSize::from(edit.range.end as u32),
    );
    Some(vec![TextEdit {
        range: doc.range(range),
        new_text: edit.text,
    }])
}

/// Formats the line just finished: the one holding a `;` just typed, or
/// the one above a new line. Nothing when the file does not parse yet.
fn on_type_formatting(
    world: &mut World,
    params: DocumentOnTypeFormattingParams,
) -> Option<Vec<TextEdit>> {
    let position = params.text_document_position;
    let path = position.text_document.uri.to_file_path().ok()?;
    let (config, text) = latest(world, &path)?;
    let index = LineIndex::new(&text);
    let doc = Doc {
        text: &text,
        index: &index,
    };
    let cursor = usize::from(doc.offset(position.position));
    let ch = params.ch.chars().next()?;
    let root = || world.root.dir.clone();
    let edits = crate::format::typed(Some(&path), &text, cursor, ch, &config, root).ok()?;
    Some(text_edits(doc, edits))
}

fn range_formatting(
    world: &mut World,
    params: DocumentRangeFormattingParams,
) -> Option<Vec<TextEdit>> {
    let path = params.text_document.uri.to_file_path().ok()?;
    let (config, text) = latest(world, &path)?;
    let index = LineIndex::new(&text);
    let doc = Doc {
        text: &text,
        index: &index,
    };
    let range =
        usize::from(doc.offset(params.range.start))..usize::from(doc.offset(params.range.end));
    let root = || world.root.dir.clone();
    let edits = crate::format::range(Some(&path), &text, range, &config, root).ok()?;
    Some(text_edits(doc, edits))
}

fn text_edits(doc: Doc, edits: Vec<vfl_fmt::Edit>) -> Vec<TextEdit> {
    edits
        .into_iter()
        .map(|e| TextEdit {
            range: doc.range(TextRange::new(
                TextSize::from(e.range.start as u32),
                TextSize::from(e.range.end as u32),
            )),
            new_text: e.text,
        })
        .collect()
}

/// What is at a position: a reference's targets, or a declaration's name.
pub(crate) struct Hit {
    pub range: TextRange,
    pub targets: Vec<DeclId>,
    pub library: Option<String>,
}

/// The references in a file, unit by unit.
pub(crate) fn file_references(
    world: &World,
    file: FileId,
) -> impl Iterator<Item = &vfl_sema::Reference> {
    world
        .analysis
        .file_units(file)
        .flat_map(|unit| world.analysis.unit_references(unit))
}

pub(crate) fn hit_at(world: &World, file: FileId, offset: TextSize) -> Option<Hit> {
    let contains = |r: TextRange| r.start() <= offset && offset <= r.end();
    for reference in file_references(world, file) {
        if contains(reference.range) {
            let mut targets = Vec::new();
            let mut library = None;
            for t in &reference.targets {
                match t {
                    Target::Decl(id) => targets.push(*id),
                    Target::Library(name) => library = Some(name.to_string()),
                }
            }
            return Some(Hit {
                range: reference.range,
                targets,
                library,
            });
        }
    }
    let decls = world.analysis.file_units(file).flat_map(|unit| {
        world
            .analysis
            .unit_decls(unit)
            .iter()
            .enumerate()
            .map(move |(index, decl)| {
                let index = index as u32;
                (DeclId { unit, index }, decl)
            })
    });
    for (id, decl) in decls {
        if decl.file == file && contains(decl.range) && !decl.spelling.is_empty() {
            return Some(Hit {
                range: decl.range,
                targets: vec![id],
                library: None,
            });
        }
    }
    None
}

/// Where a declaration is, writing bundled library sources to a cache
/// directory so editors can open them.
fn location(world: &World, decl: &Decl) -> Option<Location> {
    let path = openable_path(world, decl.file)?;
    Some(Location {
        uri: uri(&path)?,
        range: world.doc(decl.file).range(decl.range),
    })
}

/// A path an editor can open for `file`: bundled library sources are
/// written to a cache directory first.
pub(crate) fn openable_path(world: &World, file: FileId) -> Option<PathBuf> {
    let file = &world.analysis.files()[file as usize];
    if file.builtin {
        bundled_path(&file.path, &file.text)
    } else {
        Some(file.path.clone())
    }
}

fn bundled_path(path: &Path, text: &str) -> Option<PathBuf> {
    let relative = path.strip_prefix("<bundled>").ok()?;
    let target = std::env::temp_dir()
        .join(format!("vfl-{}", env!("CARGO_PKG_VERSION")))
        .join(relative)
        .with_extension("vhd");
    if std::fs::read_to_string(&target).ok().as_deref() != Some(text) {
        std::fs::create_dir_all(target.parent()?).ok()?;
        std::fs::write(&target, text).ok()?;
    }
    Some(target)
}

fn definition(world: &mut World, params: GotoDefinitionParams) -> Option<GotoDefinitionResponse> {
    let position = &params.text_document_position_params;
    let file = file_of(world, &position.text_document.uri)?;
    let offset = world.doc(file).offset(position.position);
    let locations: Vec<Location> = definitions(world, file, offset)
        .into_iter()
        .filter_map(|(file, range)| {
            Some(Location {
                uri: uri(&openable_path(world, file)?)?,
                range: world.doc(file).range(range),
            })
        })
        .collect();
    (!locations.is_empty()).then_some(GotoDefinitionResponse::Array(locations))
}

/// Where what is named at `offset` is declared.
pub(crate) fn definitions(
    world: &World,
    file: FileId,
    offset: TextSize,
) -> Vec<(FileId, TextRange)> {
    if let Some((_, Symbol::Element { record, name })) = refactor::element_at(world, file, offset) {
        return refactor::element_decls(world, record, &name);
    }
    let Some(hit) = hit_at(world, file, offset) else {
        return Vec::new();
    };
    hit.targets
        .iter()
        .filter_map(|id| world.analysis.decl(*id))
        .map(|decl| (decl.file, decl.range))
        .collect()
}

/// Every place that names one of `targets`, in file `within` or in all:
/// declarations and references, once each.
pub(crate) fn occurrences(
    world: &World,
    targets: &[DeclId],
    declarations: bool,
    within: Option<FileId>,
) -> Vec<(FileId, TextRange, Access)> {
    let mut out = Vec::new();
    if declarations {
        for id in targets {
            if let Some(decl) = world.analysis.decl(*id)
                && within.is_none_or(|f| f == decl.file)
            {
                out.push((decl.file, decl.range, Access::Other));
            }
        }
    }
    let references: Box<dyn Iterator<Item = (FileId, &vfl_sema::Reference)>> = match within {
        Some(file) => Box::new(world.analysis.file_units(file).flat_map(move |u| {
            world
                .analysis
                .unit_references(u)
                .iter()
                .map(move |r| (file, r))
        })),
        None => Box::new(world.analysis.references()),
    };
    for (file, reference) in references {
        if reference
            .targets
            .iter()
            .any(|t| matches!(t, Target::Decl(id) if targets.contains(id)))
        {
            out.push((file, reference.range, reference.access));
        }
    }
    out.sort_by_key(|(f, r, _)| (*f, r.start(), r.end()));
    out.dedup_by_key(|(f, r, _)| (*f, *r));
    out
}

fn references(world: &mut World, params: ReferenceParams) -> Option<Vec<Location>> {
    let position = &params.text_document_position;
    let file = file_of(world, &position.text_document.uri)?;
    let offset = world.doc(file).offset(position.position);
    let (_, symbol) = refactor::symbol_at(world, file, offset, None)?;
    let found = refactor::occurrences(world, &symbol, params.context.include_declaration, None);
    Some(
        found
            .into_iter()
            .filter(|(file, _, _)| !world.analysis.files()[*file as usize].builtin)
            .filter_map(|(file, range, _)| {
                Some(Location {
                    uri: uri(&world.analysis.files()[file as usize].path)?,
                    range: world.doc(file).range(range),
                })
            })
            .collect(),
    )
}

fn highlight(world: &mut World, params: DocumentHighlightParams) -> Option<Vec<DocumentHighlight>> {
    let position = &params.text_document_position_params;
    let file = file_of(world, &position.text_document.uri)?;
    let doc = world.doc(file);
    let (_, symbol) = refactor::symbol_at(world, file, doc.offset(position.position), Some(file))?;
    Some(
        refactor::occurrences(world, &symbol, true, Some(file))
            .into_iter()
            .filter(|(f, _, _)| *f == file)
            .map(|(_, range, access)| DocumentHighlight {
                range: doc.range(range),
                kind: Some(if access.writes() {
                    DocumentHighlightKind::WRITE
                } else if access.reads() {
                    DocumentHighlightKind::READ
                } else {
                    DocumentHighlightKind::TEXT
                }),
            })
            .collect(),
    )
}

/// The source of a declaration, shortened for hovers.
fn declaration_text(world: &World, decl: &Decl) -> String {
    let doc = world.doc(decl.file);
    let text = &doc.text[std::ops::Range::<usize>::from(decl.full_range)];
    let mut lines: Vec<&str> = Vec::new();
    for line in text.lines() {
        lines.push(line);
        let lower = line.trim().to_ascii_lowercase();
        let body_starts = matches!(
            decl.kind,
            DeclKind::Function | DeclKind::Procedure | DeclKind::Architecture | DeclKind::Label
        ) && (lower == "is" || lower.ends_with(" is") || lower == "begin");
        if body_starts || lines.len() >= 15 {
            if lines.len() < text.lines().count() {
                lines.push("  ...");
            }
            break;
        }
    }
    // Re-indent relative to the first line's column, which counts
    // characters: the indentation may hold multi-byte spaces.
    let column = doc.index.line_col(doc.text, decl.full_range.start()).col as usize;
    lines
        .iter()
        .enumerate()
        .map(|(i, line)| {
            if i == 0 {
                line.to_string()
            } else {
                let strip: usize = line
                    .chars()
                    .take(column)
                    .take_while(|c| c.is_whitespace())
                    .map(char::len_utf8)
                    .sum();
                line[strip..].to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// For a declaration in a Verilog file: what it is (module, parameter,
/// port) and its declaration as written there.
fn foreign_declaration(world: &World, decl: &Decl) -> Option<(&'static str, String)> {
    let modules = world.analysis.foreign_modules(decl.file)?;
    if decl.kind == DeclKind::Entity {
        return Some(("module", declaration_text(world, decl)));
    }
    let (kind, object) = modules.iter().find_map(|m| {
        m.generics
            .iter()
            .map(|g| ("parameter", g))
            .chain(m.ports.iter().map(|p| ("port", p)))
            .find(|(_, o)| o.name_range == decl.range)
    })?;
    Some((kind, object.declaration.clone()))
}

fn hover(world: &mut World, params: HoverParams) -> Option<Hover> {
    let position = &params.text_document_position_params;
    let file = file_of(world, &position.text_document.uri)?;
    let doc = world.doc(file);
    let (range, value) = hover_at(world, file, doc.offset(position.position))?;
    Some(Hover {
        contents: HoverContents::Markup(MarkupContent {
            kind: MarkupKind::Markdown,
            value,
        }),
        range: Some(doc.range(range)),
    })
}

/// The hover text at `offset`, in Markdown, and the range it is about.
pub(crate) fn hover_at(
    world: &World,
    file: FileId,
    offset: TextSize,
) -> Option<(TextRange, String)> {
    let Some(hit) = hit_at(world, file, offset) else {
        // A record element, which has no declaration of its own.
        return crate::types::element_hover(world, file, offset);
    };
    let mut sections = Vec::new();
    for id in &hit.targets {
        let Some(decl) = world.analysis.decl(*id) else {
            continue;
        };
        let unit = &world.analysis.units()[id.unit as usize];
        let place = if id.index == 0 {
            format!("in library `{}`", unit.library)
        } else {
            format!(
                "in `{}.{}`",
                unit.library,
                world.analysis.unit_decls(id.unit)[0].spelling
            )
        };
        if let Some((kind, text)) = foreign_declaration(world, decl) {
            sections.push(format!(
                "{kind} `{}` {place}\n\n```systemverilog\n{text}\n```",
                decl.spelling
            ));
            continue;
        }
        let types = crate::types::hover_lines(&world.analysis, *id)
            .map(|t| format!("\n\n{t}"))
            .unwrap_or_default();
        sections.push(format!(
            "{} `{}` {place}{types}\n\n```vhdl\n{}\n```",
            decl.kind.describe(),
            decl.spelling,
            declaration_text(world, decl)
        ));
    }
    if sections.is_empty()
        && let Some(library) = &hit.library
    {
        sections.push(format!("library `{library}`"));
    }
    if sections.is_empty() {
        return None;
    }
    Some((hit.range, sections.join("\n\n---\n\n")))
}

fn symbol_kind(kind: DeclKind) -> Option<SymbolKind> {
    Some(match kind {
        DeclKind::Entity => SymbolKind::INTERFACE,
        DeclKind::Architecture => SymbolKind::CLASS,
        DeclKind::Package | DeclKind::PackageInstance | DeclKind::PackageBody => {
            SymbolKind::PACKAGE
        }
        DeclKind::Context | DeclKind::Configuration => SymbolKind::MODULE,
        DeclKind::Component => SymbolKind::INTERFACE,
        DeclKind::Signal | DeclKind::Variable | DeclKind::File => SymbolKind::VARIABLE,
        DeclKind::Constant | DeclKind::Generic { .. } => SymbolKind::CONSTANT,
        DeclKind::Port { .. } => SymbolKind::FIELD,
        DeclKind::Type | DeclKind::Subtype | DeclKind::ProtectedBody | DeclKind::GenericType => {
            SymbolKind::STRUCT
        }
        DeclKind::EnumLiteral => SymbolKind::ENUM_MEMBER,
        DeclKind::Function | DeclKind::GenericSubprogram => SymbolKind::FUNCTION,
        DeclKind::Procedure => SymbolKind::METHOD,
        DeclKind::Alias | DeclKind::Attribute => SymbolKind::PROPERTY,
        DeclKind::Label => SymbolKind::EVENT,
        DeclKind::View | DeclKind::Group => SymbolKind::OBJECT,
        DeclKind::GenericPackage => SymbolKind::PACKAGE,
        DeclKind::IncompleteType
        | DeclKind::Parameter { .. }
        | DeclKind::PhysicalUnit
        | DeclKind::LoopParameter
        | DeclKind::GenerateParameter => return None,
    })
}

/// The declarations of a file worth listing, once each (a file in two
/// libraries is analyzed twice), with their ids.
fn file_decls(world: &World, file: FileId) -> Vec<(DeclId, &Decl)> {
    let mut seen = FxHashSet::default();
    let mut out = Vec::new();
    for unit in world.analysis.file_units(file) {
        for (index, decl) in world.analysis.unit_decls(unit).iter().enumerate() {
            if decl.file != file || decl.spelling.is_empty() || !seen.insert(decl.range) {
                continue;
            }
            out.push((
                DeclId {
                    unit,
                    index: index as u32,
                },
                decl,
            ));
        }
    }
    out
}

#[allow(deprecated)]
fn document_symbols(
    world: &mut World,
    params: DocumentSymbolParams,
) -> Option<DocumentSymbolResponse> {
    let Some(file) = file_of(world, &params.text_document.uri) else {
        return verilog_symbols(world, &params.text_document.uri);
    };
    let doc = world.doc(file);
    let (decls, roots, children) = outline(world, file);
    fn build(
        i: usize,
        decls: &[(DeclId, &Decl)],
        children: &[Vec<usize>],
        doc: Doc,
    ) -> DocumentSymbol {
        let decl = decls[i].1;
        let mut range = decl.full_range;
        if !range.contains_range(decl.range) {
            range = decl.range;
        }
        DocumentSymbol {
            name: decl.spelling.clone(),
            detail: Some(decl.kind.describe().into()),
            kind: symbol_kind(decl.kind).unwrap_or(SymbolKind::NULL),
            tags: None,
            deprecated: None,
            range: doc.range(range),
            selection_range: doc.range(decl.range),
            children: Some(
                children[i]
                    .iter()
                    .map(|&c| build(c, decls, children, doc))
                    .collect(),
            ),
        }
    }
    Some(DocumentSymbolResponse::Nested(
        roots
            .into_iter()
            .map(|i| build(i, &decls, &children, doc))
            .collect(),
    ))
}

/// The outline of a Verilog file.
#[allow(deprecated)]
fn verilog_symbols(world: &World, uri: &Url) -> Option<DocumentSymbolResponse> {
    let file = crate::verilog::file(world, &uri.to_file_path().ok()?)?;
    let index = LineIndex::new(&file.text);
    let doc = Doc {
        text: &file.text,
        index: &index,
    };
    fn build(symbol: crate::verilog::Symbol, doc: Doc) -> DocumentSymbol {
        DocumentSymbol {
            name: symbol.name,
            detail: Some(symbol.detail),
            kind: symbol.kind,
            tags: None,
            deprecated: None,
            range: doc.range(symbol.range),
            selection_range: doc.range(symbol.selection),
            children: Some(symbol.children.into_iter().map(|c| build(c, doc)).collect()),
        }
    }
    Some(DocumentSymbolResponse::Nested(
        crate::verilog::symbols(&file.parse.syntax())
            .into_iter()
            .map(|s| build(s, doc))
            .collect(),
    ))
}

/// The regions of a Verilog file that can be folded. VHDL files keep the
/// editor's folding by indentation.
fn folding_ranges(world: &mut World, params: FoldingRangeParams) -> Option<Vec<FoldingRange>> {
    let path = params.text_document.uri.to_file_path().ok()?;
    let file = crate::verilog::file(world, &path)?;
    let text = &*file.text;
    let index = LineIndex::new(text);
    let line = |offset: TextSize| index.line(offset);
    let mut out = Vec::new();
    for (range, fold) in crate::verilog::folds(&file.parse.syntax()) {
        let start = line(range.start());
        let mut end = line(range.end());
        // Keep a closing line (`end`, `);`) visible under the fold.
        let closing = fold == crate::verilog::Fold::Code
            && text[..usize::from(range.end())]
                .rsplit('\n')
                .next()
                .is_some_and(|before| before.trim().is_empty());
        if closing {
            end = end.saturating_sub(1);
        }
        if end > start {
            out.push(FoldingRange {
                start_line: start,
                end_line: end,
                kind: match fold {
                    crate::verilog::Fold::Code => None,
                    crate::verilog::Fold::Comment => Some(FoldingRangeKind::Comment),
                    crate::verilog::Fold::Region => Some(FoldingRangeKind::Region),
                },
                ..Default::default()
            });
        }
    }
    Some(out)
}

/// A file's outline: its listed declarations, the indexes of the top-level
/// ones, and the children of each.
#[allow(clippy::type_complexity)]
pub(crate) fn outline(
    world: &World,
    file: FileId,
) -> (Vec<(DeclId, &Decl)>, Vec<usize>, Vec<Vec<usize>>) {
    let decls = file_decls(world, file);
    let ids: FxHashMap<DeclId, usize> = decls
        .iter()
        .enumerate()
        .map(|(i, (id, _))| (*id, i))
        .collect();
    let mut children: Vec<Vec<usize>> = vec![Vec::new(); decls.len()];
    let mut roots = Vec::new();
    for (i, (_, decl)) in decls.iter().enumerate() {
        if symbol_kind(decl.kind).is_none() {
            continue;
        }
        // Attach to the nearest listed ancestor.
        let mut parent = decl.parent;
        let mut attached = false;
        while let Some(p) = parent {
            if let Some(&j) = ids.get(&p)
                && symbol_kind(decls[j].1.kind).is_some()
            {
                children[j].push(i);
                attached = true;
                break;
            }
            parent = world.analysis.decl(p).and_then(|d| d.parent);
        }
        if !attached {
            roots.push(i);
        }
    }
    (decls, roots, children)
}

#[allow(deprecated)]
fn workspace_symbols(
    world: &mut World,
    params: WorkspaceSymbolParams,
) -> Option<WorkspaceSymbolResponse> {
    let mut out = Vec::new();
    for id in workspace_decls(world, &params.query) {
        let Some(decl) = world.analysis.decl(id) else {
            continue;
        };
        let Some(location) = location(world, decl) else {
            continue;
        };
        let container =
            (id.index != 0).then(|| world.analysis.unit_decls(id.unit)[0].spelling.clone());
        out.push(SymbolInformation {
            name: decl.spelling.clone(),
            kind: symbol_kind(decl.kind).unwrap_or(SymbolKind::NULL),
            tags: None,
            deprecated: None,
            location,
            container_name: container,
        });
    }
    Some(WorkspaceSymbolResponse::Flat(out))
}

/// The project's design units, types, subprograms and package objects
/// whose name contains `query`, ignoring case; 1000 at most.
pub(crate) fn workspace_decls(world: &World, query: &str) -> Vec<DeclId> {
    let query = query.to_ascii_lowercase();
    let mut out = Vec::new();
    let user_files = world.analysis.user_files().len() as FileId;
    for file in 0..user_files {
        for (id, decl) in file_decls(world, file) {
            let listed = matches!(
                decl.kind,
                DeclKind::Entity
                    | DeclKind::Architecture
                    | DeclKind::Package
                    | DeclKind::PackageInstance
                    | DeclKind::Context
                    | DeclKind::Configuration
                    | DeclKind::Component
                    | DeclKind::Type
                    | DeclKind::Subtype
                    | DeclKind::Function
                    | DeclKind::Procedure
            ) || (matches!(decl.kind, DeclKind::Constant | DeclKind::Signal)
                && decl.parent.is_some_and(|p| {
                    p.index == 0
                        && world.analysis.units()[p.unit as usize].kind == DeclKind::Package
                }));
            if !listed || !decl.name.contains(&query) {
                continue;
            }
            out.push(id);
            if out.len() >= 1000 {
                return out;
            }
        }
    }
    out
}

/// The name a rename at `position` would change, if it may.
fn rename_targets(
    world: &World,
    position: &TextDocumentPositionParams,
) -> Result<(FileId, TextRange, Symbol), String> {
    let file = file_of(world, &position.text_document.uri).ok_or("nothing to rename here")?;
    let offset = world.doc(file).offset(position.position);
    let (range, symbol) = rename_targets_at(world, file, offset)?;
    Ok((file, range, symbol))
}

/// The name a rename at `offset` would change and what it stands for, if
/// it may be renamed.
pub(crate) fn rename_targets_at(
    world: &World,
    file: FileId,
    offset: TextSize,
) -> Result<(TextRange, Symbol), String> {
    let Some((range, symbol)) = refactor::symbol_at(world, file, offset, None) else {
        return Err(match hit_at(world, file, offset) {
            Some(_) => "this name is not declared in the project".into(),
            None => "nothing to rename here".into(),
        });
    };
    let ids: &[DeclId] = match &symbol {
        Symbol::Decls(ids) => ids,
        Symbol::Element { record, .. } => std::slice::from_ref(record),
    };
    for id in ids {
        let decl = world.analysis.decl(*id).ok_or("unknown declaration")?;
        if world.analysis.files()[decl.file as usize].builtin {
            return Err(format!(
                "`{}` is declared in a bundled library",
                decl.spelling
            ));
        }
        if world.analysis.foreign_modules(decl.file).is_some() {
            return Err(format!(
                "`{}` is declared in a Verilog file, which renaming does not change",
                decl.spelling
            ));
        }
        if matches!(symbol, Symbol::Decls(_))
            && (decl.spelling.starts_with('"') || decl.spelling.starts_with('\''))
        {
            return Err("operators and character literals cannot be renamed".into());
        }
    }
    // Selected names (`work.pkg.x`) cover more than the identifier.
    let doc = world.doc(file);
    if doc.text[std::ops::Range::<usize>::from(range)].contains('.') {
        return Err("rename the name where it is declared or used alone".into());
    }
    Ok((range, symbol))
}

fn prepare_rename(
    world: &mut World,
    params: TextDocumentPositionParams,
) -> Result<Option<PrepareRenameResponse>, Error> {
    let (file, range, _) = rename_targets(world, &params).map_err(Error::Failed)?;
    Ok(Some(PrepareRenameResponse::Range(
        world.doc(file).range(range),
    )))
}

fn rename(world: &mut World, params: RenameParams) -> Result<Option<WorkspaceEdit>, Error> {
    let new_name = params.new_name.trim();
    let (_, _, symbol) =
        rename_targets(world, &params.text_document_position).map_err(Error::Failed)?;
    let edits = rename_edits(world, &symbol, new_name).map_err(Error::Failed)?;
    Ok(Some(workspace_edit(world, edits)))
}

/// A workspace edit making `edits`, by file.
pub(crate) fn workspace_edit(
    world: &World,
    edits: impl IntoIterator<Item = (FileId, TextRange, String)>,
) -> WorkspaceEdit {
    let mut changes: std::collections::HashMap<Url, Vec<TextEdit>> = Default::default();
    for (file, range, text) in edits {
        let Some(uri) = uri(&world.analysis.files()[file as usize].path) else {
            continue;
        };
        changes.entry(uri).or_default().push(TextEdit {
            range: world.doc(file).range(range),
            new_text: text,
        });
    }
    WorkspaceEdit {
        changes: Some(changes),
        ..Default::default()
    }
}

/// The edits renaming `symbol` to `new_name`, or why it may not be.
pub(crate) fn rename_edits(
    world: &World,
    symbol: &Symbol,
    new_name: &str,
) -> Result<Vec<(FileId, TextRange, String)>, String> {
    check_identifier(new_name)?;
    if let Some(conflict) = refactor::rename_conflict(world, symbol, new_name) {
        return Err(format!("cannot rename to `{new_name}`: {conflict}"));
    }
    Ok(rename_ranges(world, symbol)
        .into_iter()
        .map(|(file, range)| (file, range, new_name.to_owned()))
        .collect())
}

/// Whether `name` is a VHDL identifier, basic or extended.
pub(crate) fn check_identifier(name: &str) -> Result<(), String> {
    let new_name = name;
    let valid = if new_name.starts_with('\\') {
        new_name.len() > 2 && new_name.ends_with('\\')
    } else {
        new_name
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic())
            && new_name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_')
            && !new_name.contains("__")
            && !new_name.ends_with('_')
    };
    if !valid {
        Err(format!("`{new_name}` is not a VHDL identifier"))
    } else if KEYWORDS.contains(&new_name.to_ascii_lowercase().as_str()) {
        Err(format!("`{new_name}` is a reserved word"))
    } else {
        Ok(())
    }
}

/// What renaming `symbol` replaces: each name, or the last part of a
/// selected name, in the project's own files, once each.
pub(crate) fn rename_ranges(world: &World, symbol: &Symbol) -> Vec<(FileId, TextRange)> {
    let mut out: Vec<(FileId, TextRange)> = refactor::occurrences(world, symbol, true, None)
        .into_iter()
        .filter(|(file, _, _)| !world.analysis.files()[*file as usize].builtin)
        .map(|(file, range, _)| {
            let text = &world.doc(file).text[std::ops::Range::<usize>::from(range)];
            let start = text.rfind('.').map_or(0, |i| i + 1);
            let range = TextRange::new(range.start() + TextSize::from(start as u32), range.end());
            (file, range)
        })
        .collect();
    out.sort_by_key(|(f, r)| (*f, r.start()));
    out.dedup();
    out
}

const KEYWORDS: &[&str] = &[
    "abs",
    "access",
    "after",
    "alias",
    "all",
    "and",
    "architecture",
    "array",
    "assert",
    "attribute",
    "begin",
    "block",
    "body",
    "buffer",
    "bus",
    "case",
    "component",
    "configuration",
    "constant",
    "context",
    "downto",
    "else",
    "elsif",
    "end",
    "entity",
    "exit",
    "file",
    "for",
    "function",
    "generate",
    "generic",
    "group",
    "guarded",
    "if",
    "impure",
    "in",
    "inertial",
    "inout",
    "is",
    "label",
    "library",
    "linkage",
    "literal",
    "loop",
    "map",
    "mod",
    "nand",
    "new",
    "next",
    "nor",
    "not",
    "null",
    "of",
    "on",
    "open",
    "or",
    "others",
    "out",
    "package",
    "port",
    "postponed",
    "procedure",
    "process",
    "protected",
    "pure",
    "range",
    "record",
    "register",
    "reject",
    "rem",
    "report",
    "return",
    "rol",
    "ror",
    "select",
    "severity",
    "shared",
    "signal",
    "sla",
    "sll",
    "sra",
    "srl",
    "subtype",
    "then",
    "to",
    "transport",
    "type",
    "unaffected",
    "units",
    "until",
    "use",
    "variable",
    "view",
    "wait",
    "when",
    "while",
    "with",
    "xnor",
    "xor",
];

fn completion_kind(kind: DeclKind) -> CompletionItemKind {
    match kind {
        DeclKind::Entity | DeclKind::Component => CompletionItemKind::INTERFACE,
        DeclKind::Architecture => CompletionItemKind::CLASS,
        DeclKind::Package
        | DeclKind::PackageInstance
        | DeclKind::PackageBody
        | DeclKind::Context
        | DeclKind::Configuration
        | DeclKind::GenericPackage => CompletionItemKind::MODULE,
        DeclKind::Signal | DeclKind::Variable | DeclKind::File | DeclKind::LoopParameter => {
            CompletionItemKind::VARIABLE
        }
        DeclKind::Constant | DeclKind::Generic { .. } | DeclKind::GenerateParameter => {
            CompletionItemKind::CONSTANT
        }
        DeclKind::Port { .. } | DeclKind::Parameter { .. } => CompletionItemKind::FIELD,
        DeclKind::Type
        | DeclKind::Subtype
        | DeclKind::IncompleteType
        | DeclKind::ProtectedBody
        | DeclKind::GenericType => CompletionItemKind::STRUCT,
        DeclKind::EnumLiteral => CompletionItemKind::ENUM_MEMBER,
        DeclKind::PhysicalUnit => CompletionItemKind::UNIT,
        DeclKind::Function | DeclKind::GenericSubprogram => CompletionItemKind::FUNCTION,
        DeclKind::Procedure => CompletionItemKind::METHOD,
        _ => CompletionItemKind::REFERENCE,
    }
}

#[derive(Default)]
struct Items {
    items: Vec<CompletionItem>,
    seen: FxHashSet<(String, String)>,
}

impl Items {
    fn push(&mut self, label: &str, kind: CompletionItemKind, detail: &str) {
        if label.is_empty() || label.starts_with('"') {
            return;
        }
        if self
            .seen
            .insert((label.to_ascii_lowercase(), format!("{kind:?}")))
        {
            self.items.push(CompletionItem {
                label: label.to_owned(),
                kind: Some(kind),
                detail: (!detail.is_empty()).then(|| detail.to_owned()),
                ..Default::default()
            });
        }
    }

    fn decl(&mut self, decl: &Decl) {
        self.push(
            &decl.spelling,
            completion_kind(decl.kind),
            decl.kind.describe(),
        );
    }
}

/// Calls `f` with the declarations visible at `offset` in `file`: those of
/// the file's units and their primary units, of the packages the file
/// refers to, and of `std.standard`.
pub(crate) fn visible_decls(
    world: &World,
    file: FileId,
    offset: TextSize,
    mut f: impl FnMut(DeclId, &Decl),
) {
    let analysis = &world.analysis;
    // Declarations in scope in this file's units and their primary units.
    let units: Vec<_> = analysis.file_units(file).collect();
    let mut scope_units = units.clone();
    for &unit in &units {
        let info = &analysis.units()[unit as usize];
        if let Some(primary) = &info.primary
            && let Some(p) = analysis.units().iter().position(|u| {
                u.library == info.library && u.name == *primary && u.primary.is_none()
            })
        {
            scope_units.push(p as vfl_sema::UnitId);
        }
    }
    for &unit in &scope_units {
        for (index, decl) in analysis.unit_decls(unit).iter().enumerate() {
            let visible = match decl.parent {
                None => true,
                Some(p) if p.index == 0 => true,
                Some(p) => analysis.decl(p).is_some_and(|parent| {
                    parent.file == file && parent.full_range.contains_inclusive(offset)
                }),
            };
            if visible {
                let id = DeclId {
                    unit,
                    index: index as u32,
                };
                f(id, decl);
            }
        }
    }
    // Packages this file refers to, and std.standard.
    let mut packages: Vec<DeclId> = Vec::new();
    for reference in file_references(world, file) {
        for target in &reference.targets {
            if let Target::Decl(id) = target
                && id.index == 0
                && matches!(
                    analysis.units()[id.unit as usize].kind,
                    DeclKind::Package | DeclKind::PackageInstance
                )
                && !packages.contains(id)
            {
                packages.push(*id);
            }
        }
    }
    if let Some(standard) = analysis.units().iter().position(|u| {
        u.library == "std" && u.name == "standard" && analysis.files()[u.file as usize].builtin
    }) {
        packages.push(DeclId {
            unit: standard as vfl_sema::UnitId,
            index: 0,
        });
    }
    for id in packages {
        if let Some(members) = analysis.decl(id).and_then(|d| d.members.clone()) {
            for member in members.iter() {
                if let Some(decl) = analysis.decl(member) {
                    f(member, decl);
                }
            }
        }
    }
}

fn completion(world: &mut World, params: CompletionParams) -> Option<CompletionResponse> {
    let position = &params.text_document_position;
    let file = file_of(world, &position.text_document.uri)?;
    let offset = world.doc(file).offset(position.position);
    Some(CompletionResponse::Array(completion_at(
        world, file, offset,
    )))
}

/// The completions at `offset`.
pub(crate) fn completion_at(world: &World, file: FileId, offset: TextSize) -> Vec<CompletionItem> {
    let doc = world.doc(file);
    let offset = offset.min(TextSize::of(doc.text));
    let before = &doc.text[..usize::from(offset)];
    let word_start = before
        .trim_end_matches(|c: char| c.is_alphanumeric() || c == '_')
        .len();
    let analysis = &world.analysis;
    let mut items = Items::default();

    // After `prefix.`: what the prefix contains.
    if before[..word_start].ends_with('.') {
        let dot = TextSize::from((word_start - 1) as u32);
        let mut found = false;
        for reference in file_references(world, file) {
            if reference.range.end() != dot {
                continue;
            }
            for target in &reference.targets {
                match target {
                    Target::Library(library) => {
                        for unit in analysis.units() {
                            if *unit.library == **library
                                && unit.primary.is_none()
                                && !matches!(
                                    unit.kind,
                                    DeclKind::Architecture | DeclKind::PackageBody
                                )
                            {
                                items.push(
                                    &analysis.files()[unit.file as usize].text
                                        [std::ops::Range::<usize>::from(unit.name_range)],
                                    completion_kind(unit.kind),
                                    unit.kind.describe(),
                                );
                            }
                        }
                        found = true;
                    }
                    Target::Decl(id) => {
                        if let Some(members) = analysis.decl(*id).and_then(|d| d.members.clone()) {
                            for member in members.iter() {
                                if let Some(decl) = analysis.decl(member) {
                                    items.decl(decl);
                                }
                            }
                            items.push("all", CompletionItemKind::KEYWORD, "");
                            found = true;
                        }
                    }
                }
            }
        }
        if found {
            return items.items;
        }
        // Elements of a record, methods of a protected type.
        if let Some(members) = crate::types::members_after_dot(world, file, dot) {
            for (label, kind, detail) in members {
                items.push(&label, kind, &detail);
            }
            return items.items;
        }
    }

    visible_decls(world, file, offset, |_, decl| items.decl(decl));
    // Libraries and their units.
    let mut libraries: Vec<&str> = vec!["work"];
    for unit in analysis.units() {
        let unit_file = &analysis.files()[unit.file as usize];
        if !unit_file.builtin && unit.primary.is_none() {
            items.push(
                &unit_file.text[std::ops::Range::<usize>::from(unit.name_range)],
                completion_kind(unit.kind),
                if unit_file.foreign.is_some() {
                    "module"
                } else {
                    unit.kind.describe()
                },
            );
        }
        if !libraries.contains(&unit.library.as_str()) {
            libraries.push(&unit.library);
        }
    }
    for library in libraries {
        items.push(library, CompletionItemKind::MODULE, "library");
    }
    for keyword in KEYWORDS {
        items.push(keyword, CompletionItemKind::KEYWORD, "");
    }
    items.items
}

fn code_actions(world: &mut World, params: CodeActionParams) -> Option<Vec<CodeActionOrCommand>> {
    // Verilog files have quick fixes too.
    let file = world.any_file_id(&params.text_document.uri.to_file_path().ok()?)?;
    let doc = world.doc(file);
    let start = doc.offset(params.range.start);
    let end = doc.offset(params.range.end);
    let uri = params.text_document.uri.clone();
    let only = params.context.only.as_ref();
    let wants = |kind: &CodeActionKind| {
        only.is_none_or(|only| only.iter().any(|k| kind.as_str().starts_with(k.as_str())))
    };
    let mut out = Vec::new();
    if wants(&CodeActionKind::QUICKFIX) {
        for action in crate::actions::actions(world, file, start, end) {
            let problem = &world.lint[action.problem];
            // A rename's edits are worked out when it is chosen, and so
            // are an association's.
            let (edit, data) = match (&action.rename, action.associate) {
                (_, Some(associate)) => (
                    None,
                    serde_json::to_value(AssociateData {
                        associate,
                        uri: uri.clone(),
                        offset: problem.range.start().into(),
                    })
                    .ok(),
                ),
                (Some(new_name), None) => (
                    None,
                    serde_json::to_value(RenameData {
                        rename: new_name.clone(),
                        uri: uri.clone(),
                        offset: problem.range.start().into(),
                    })
                    .ok(),
                ),
                (None, None) => (Some(workspace_edit(world, action.edits)), None),
            };
            out.push(CodeActionOrCommand::CodeAction(CodeAction {
                title: action.title,
                kind: Some(CodeActionKind::QUICKFIX),
                diagnostics: Some(vec![lsp_diagnostic(world, problem)]),
                edit,
                data,
                is_preferred: action.preferred.then_some(true),
                ..Default::default()
            }));
        }
    }
    let fix_all = CodeActionKind::new("source.fixAll.vfl");
    if wants(&fix_all) {
        let in_file = world.lint.iter().filter(|d| d.file == file);
        let fixes: Vec<&vfl_lint::Fix> = in_file
            .filter_map(|d| d.fix.as_ref())
            .filter(|f| f.safe)
            .collect();
        if !fixes.is_empty() {
            let kept = compatible_fixes(doc.text, fixes);
            let edits = kept
                .iter()
                .flat_map(|f| &f.edits)
                .map(|e| (file, e.range, e.text.clone()));
            out.push(CodeActionOrCommand::CodeAction(CodeAction {
                title: "Fix all safe vfl problems".into(),
                kind: Some(fix_all),
                edit: Some(workspace_edit(world, edits)),
                ..Default::default()
            }));
        }
    }
    if world.is_vhdl(file) && wants(&CodeActionKind::new(crate::generate::TESTBENCH_KIND)) {
        out.extend(crate::generate::code_actions(world, file, start, end));
    }
    if wants(&CodeActionKind::new(crate::settings::GENERATE_CONFIG_KIND)) {
        out.extend(crate::settings::code_actions(world));
    }
    Some(out)
}

/// What a rename quick fix needs to work out its edits.
#[derive(serde::Serialize, serde::Deserialize)]
struct RenameData {
    rename: String,
    uri: Url,
    offset: u32,
}

/// The edits of a rename quick fix.
fn resolve_rename(
    world: &mut World,
    mut action: CodeAction,
    data: RenameData,
) -> Result<CodeAction, String> {
    world.refresh();
    let file = file_of(world, &data.uri).ok_or("the file is not analyzed")?;
    let (_, symbol) = rename_targets_at(world, file, TextSize::from(data.offset))?;
    let edits = rename_edits(world, &symbol, &data.rename)?;
    action.edit = Some(workspace_edit(world, edits));
    Ok(action)
}

/// What a quick fix associating left-out ports needs to work out its
/// edits.
#[derive(serde::Serialize, serde::Deserialize)]
struct AssociateData {
    associate: crate::actions::Associate,
    uri: Url,
    offset: u32,
}

/// The edits of a quick fix associating left-out ports.
fn resolve_associate(
    world: &mut World,
    mut action: CodeAction,
    data: AssociateData,
) -> Result<CodeAction, String> {
    world.refresh();
    let file = file_of(world, &data.uri).ok_or("the file is not analyzed")?;
    let edits = crate::associate::edits(
        world,
        file,
        TextSize::from(data.offset),
        data.associate.ports,
        data.associate.all,
    )
    .ok_or("the instantiation has changed")?;
    action.edit = Some(workspace_edit(world, edits));
    Ok(action)
}

/// The fixes that can be applied together, in order: edits must not
/// overlap, so keep the ones `apply_fixes` keeps.
pub(crate) fn compatible_fixes<'a>(
    text: &str,
    fixes: impl IntoIterator<Item = &'a vfl_lint::Fix>,
) -> Vec<&'a vfl_lint::Fix> {
    let mut kept: Vec<&vfl_lint::Fix> = Vec::new();
    for fix in fixes {
        let mut trial = kept.clone();
        trial.push(fix);
        if vfl_lint::apply_fixes(text, trial.iter().copied()).1 == trial.len() {
            kept.push(fix);
        }
    }
    kept
}

//! Hover, signature help and completion at byte offsets, for editors that
//! link vfl directly instead of speaking LSP, like the Notepad++ plugin.
//!
//! Each query brings the analysis up to date first, then answers from the
//! same code as the language server.

use std::ops::Range;
use std::path::Path;

use lsp_types::{CompletionItemKind, ParameterLabel};
use vfl_sema::FileId;
use vfl_syntax::TextSize;

use crate::world::World;

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
        .rfind(|c: char| !(c.is_alphanumeric() || c == '_'))
        .map_or(0, |i| i + 1);
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

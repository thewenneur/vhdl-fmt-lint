//! Hovers.

use lsp_types::{Hover, HoverContents, HoverParams, MarkupContent, MarkupKind};
use vfl_sema::{Decl, DeclKind, FileId};
use vfl_syntax::{TextRange, TextSize};

use super::file_of;
use super::references::hit_at;
use crate::world::World;

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

pub(super) fn hover(world: &mut World, params: HoverParams) -> Option<Hover> {
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

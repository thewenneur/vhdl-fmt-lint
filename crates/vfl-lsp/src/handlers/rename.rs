//! Rename.

use lsp_types::{
    PrepareRenameResponse, RenameParams, TextDocumentPositionParams, TextEdit, Url, WorkspaceEdit,
};
use vfl_sema::{DeclId, FileId};
use vfl_syntax::{TextRange, TextSize};

use super::completion::KEYWORDS;
use super::references::hit_at;
use super::{Error, file_of, uri};
use crate::refactor::{self, Symbol};
use crate::world::World;

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

pub(super) fn prepare_rename(
    world: &mut World,
    params: TextDocumentPositionParams,
) -> Result<Option<PrepareRenameResponse>, Error> {
    let (file, range, _) = rename_targets(world, &params).map_err(Error::Failed)?;
    Ok(Some(PrepareRenameResponse::Range(
        world.doc(file).range(range),
    )))
}

pub(super) fn rename(
    world: &mut World,
    params: RenameParams,
) -> Result<Option<WorkspaceEdit>, Error> {
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

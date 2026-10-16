//! Code actions: quick fixes and their resolution.

use lsp_types::{CodeAction, CodeActionKind, CodeActionOrCommand, CodeActionParams, Url};
use vfl_syntax::TextSize;

use super::diagnostic::lsp_diagnostic;
use super::file_of;
use super::rename::{rename_edits, rename_targets_at, workspace_edit};
use crate::world::World;

pub(super) fn code_actions(
    world: &mut World,
    params: CodeActionParams,
) -> Option<Vec<CodeActionOrCommand>> {
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
pub(super) struct RenameData {
    rename: String,
    uri: Url,
    offset: u32,
}

/// The edits of a rename quick fix.
pub(super) fn resolve_rename(
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
pub(super) struct AssociateData {
    associate: crate::actions::Associate,
    uri: Url,
    offset: u32,
}

/// The edits of a quick fix associating left-out ports.
pub(super) fn resolve_associate(
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

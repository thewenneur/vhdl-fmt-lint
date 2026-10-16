//! Request handlers.

mod code_actions;
mod completion;
mod diagnostic;
mod formatting;
mod hover;
mod references;
mod rename;
mod symbols;

use std::path::Path;

use lsp_types::{CodeAction, Url};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use vfl_sema::FileId;

pub(crate) use self::code_actions::compatible_fixes;
use self::code_actions::{code_actions, resolve_associate, resolve_rename};
use self::completion::completion;
pub(crate) use self::completion::{completion_at, visible_decls};
#[cfg(test)]
pub use self::diagnostic::diagnostics;
pub(crate) use self::diagnostic::{config_diagnostics, lsp_diagnostic};
use self::formatting::{formatting, on_type_formatting, range_formatting};
use self::hover::hover;
pub(crate) use self::hover::hover_at;
pub(crate) use self::references::{Hit, definitions, hit_at, occurrences, openable_path};
use self::references::{definition, highlight, references};
use self::rename::{prepare_rename, rename};
pub(crate) use self::rename::{rename_edits, rename_targets_at};
use self::symbols::{document_symbols, folding_ranges, workspace_symbols};
pub(crate) use self::symbols::{outline, workspace_decls};
use crate::world::World;

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

pub(crate) fn uri(path: &Path) -> Option<Url> {
    Url::from_file_path(path).ok()
}

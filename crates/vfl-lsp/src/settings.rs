//! The editor's settings, and turning the settings in effect into a
//! config file: the `vfl/projectConfig` request and the "Generate project
//! config" code action.
//!
//! An editor passes `vfl.toml` keys as `{ "config": { ... } }`, in
//! `initializationOptions` or later in `workspace/didChangeConfiguration`.
//! They are the config of the files no config file applies to; a config
//! file replaces them all.

use std::collections::HashMap;
use std::path::PathBuf;

use lsp_types::{
    CodeAction, CodeActionKind, CodeActionOrCommand, CreateFile, CreateFileOptions,
    DocumentChangeOperation, DocumentChanges, OneOf, OptionalVersionedTextDocumentIdentifier,
    Position, Range, ResourceOp, TextDocumentEdit, TextEdit, Url, WorkspaceEdit,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use vfl_config::Config;
use vfl_project::RootKind;

use crate::world::World;

pub const GENERATE_CONFIG_KIND: &str = "source.vfl.generateConfig";

const REFERENCE: &str =
    "https://github.com/thewenneur/vhdl-fmt-lint/blob/main/docs/configuration.md";

/// The config the editor's settings give, if they have a `config` object.
pub fn editor_config(settings: &Value) -> Option<Result<Config, String>> {
    let config = settings.get("config").filter(|c| !c.is_null())?;
    Some(
        toml::Table::deserialize(config)
            .map_err(|e| e.to_string())
            .and_then(|table| Config::try_from(table).map_err(|e| e.to_string()))
            .map_err(|e| format!("invalid editor settings: {}", e.trim())),
    )
}

/// The project's config file and what to write in it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectConfig {
    /// The config file at the project root: the one there, or `vfl.toml`.
    pub path: PathBuf,
    /// The settings in effect at the project root, every key set.
    pub text: String,
    /// Whether `path` exists, and would be replaced.
    pub exists: bool,
}

/// `vfl/projectConfig`: the settings in effect at the project root as the
/// text of a config file, and where it goes.
pub fn project_config(world: &mut World) -> Result<ProjectConfig, String> {
    let dir = world.root.dir.clone();
    if dir.as_os_str().is_empty() {
        return Err("no project folder is open".into());
    }
    let existing = vfl_config::find_config(&dir);
    let chain = vfl_config::config_chain(&dir).map_err(|e| e.to_string())?;
    let (config, source) = if chain.is_empty() {
        (world.defaults().clone(), "the editor's settings".to_owned())
    } else {
        let config = vfl_config::load_layered(&chain).map_err(|e| e.to_string())?;
        let files: Vec<String> = chain
            .iter()
            .map(|f| {
                let shown = f.strip_prefix(&dir).unwrap_or(f);
                shown.display().to_string().replace('\\', "/")
            })
            .collect();
        (config, files.join(", "))
    };
    let comment = format!(
        "vfl configuration, generated from {source}.\n\
         Every key is set to the value it had; delete the ones to leave at their default.\n\
         Reference: {REFERENCE}"
    );
    Ok(ProjectConfig {
        exists: existing.is_some(),
        path: existing.unwrap_or_else(|| dir.join(vfl_config::CONFIG_FILE_NAMES[0])),
        text: config.file_text(&comment),
    })
}

/// What the code action needs to be resolved.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerateConfigData {
    generate_config: PathBuf,
}

/// "Generate project config (vfl.toml)", when no config file applies to
/// the project. Asked for at every cursor move, so it only looks at the
/// project root; the file is written when the action is chosen
/// ([`resolve`]).
pub fn code_actions(world: &World) -> Option<CodeActionOrCommand> {
    if world.root.kind == RootKind::Config || world.root.dir.as_os_str().is_empty() {
        return None;
    }
    let path = world.root.dir.join(vfl_config::CONFIG_FILE_NAMES[0]);
    Some(CodeActionOrCommand::CodeAction(CodeAction {
        title: "Generate project config (vfl.toml) from the settings in effect".into(),
        kind: Some(CodeActionKind::new(GENERATE_CONFIG_KIND)),
        data: serde_json::to_value(GenerateConfigData {
            generate_config: path,
        })
        .ok(),
        ..Default::default()
    }))
}

/// `codeAction/resolve`: adds the edit that creates the config file.
pub fn resolve(
    world: &mut World,
    mut action: CodeAction,
    data: GenerateConfigData,
) -> Result<CodeAction, String> {
    let generated = project_config(world)?;
    if generated.exists || generated.path != data.generate_config {
        return Err(format!("{} exists already", generated.path.display()));
    }
    let uri = Url::from_file_path(&generated.path).map_err(|()| "not a file path".to_owned())?;
    action.edit = Some(WorkspaceEdit {
        document_changes: Some(DocumentChanges::Operations(vec![
            DocumentChangeOperation::Op(ResourceOp::Create(CreateFile {
                uri: uri.clone(),
                options: Some(CreateFileOptions {
                    overwrite: Some(false),
                    ignore_if_exists: Some(true),
                }),
                annotation_id: None,
            })),
            DocumentChangeOperation::Edit(TextDocumentEdit {
                text_document: OptionalVersionedTextDocumentIdentifier { uri, version: None },
                edits: vec![OneOf::Left(TextEdit {
                    range: Range::new(Position::new(0, 0), Position::new(0, 0)),
                    new_text: generated.text,
                })],
            }),
        ])),
        changes: None::<HashMap<Url, Vec<TextEdit>>>,
        change_annotations: None,
    });
    Ok(action)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use vfl_config::{Preset, Style};

    #[test]
    fn editor_config_reads_vfl_toml_keys() {
        let settings = json!({ "config": { "lint": { "preset": "strict" }, "format": { "style": "compact" } } });
        let config = editor_config(&settings).unwrap().unwrap();
        assert_eq!(config.lint.preset, Preset::Strict);
        assert_eq!(config.format.style, Style::Compact);
        assert_eq!(
            config.format,
            vfl_config::FormatConfig::preset(Style::Compact)
        );
        assert!(editor_config(&json!({})).is_none());
        assert!(
            editor_config(&json!({ "config": { "lint": { "preset": "loose" } } }))
                .unwrap()
                .is_err()
        );
    }

    #[test]
    fn project_config_writes_the_editor_settings() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join(".git")).unwrap();
        std::fs::write(dir.path().join("a.vhd"), "entity a is\nend entity;\n").unwrap();
        let defaults = Config::from_toml_str("[lint]\npreset = \"none\"\n").unwrap();
        let mut world = World::with_defaults(dir.path(), defaults.clone());
        if !vfl_config::config_chain(dir.path()).unwrap().is_empty() {
            return; // A stray config above the temporary directory.
        }
        let action = code_actions(&world).expect("no config file, so an action");
        let generated = project_config(&mut world).unwrap();
        assert_eq!(generated.path, dir.path().join("vfl.toml"));
        assert!(!generated.exists);
        assert!(
            generated
                .text
                .starts_with("# vfl configuration, generated from the editor's settings.")
        );
        assert_eq!(Config::from_toml_str(&generated.text).unwrap(), defaults);

        let CodeActionOrCommand::CodeAction(action) = action else {
            panic!("not a code action");
        };
        let data = serde_json::from_value(action.data.clone().unwrap()).unwrap();
        let resolved = resolve(&mut world, action, data).unwrap();
        assert!(resolved.edit.is_some());

        std::fs::write(&generated.path, &generated.text).unwrap();
        world.reload();
        assert!(code_actions(&world).is_none());
        let again = project_config(&mut world).unwrap();
        assert!(again.exists);
        assert!(
            again
                .text
                .starts_with("# vfl configuration, generated from vfl.toml.")
        );
        assert_eq!(Config::from_toml_str(&again.text).unwrap(), defaults);
    }
}

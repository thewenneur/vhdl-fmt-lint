//! Code generated from an entity: the `vfl/entities` and `vfl/generate`
//! requests, and the "Generate testbench" code action.

use std::collections::HashMap;
use std::path::PathBuf;

use lsp_types::{
    CodeAction, CodeActionKind, CodeActionOrCommand, CreateFile, CreateFileOptions,
    DocumentChangeOperation, DocumentChanges, OneOf, OptionalVersionedTextDocumentIdentifier,
    Position, Range, ResourceOp, TextDocumentEdit, TextEdit, Url, WorkspaceEdit,
};
use serde::{Deserialize, Serialize};
use vfl_gen::{Found, Source};
use vfl_sema::FileId;
use vfl_syntax::TextSize;

use crate::world::World;

/// What to generate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Kind {
    /// A testbench, for a new file.
    Testbench,
    /// An entity instantiation.
    Instance,
    /// A component declaration.
    Component,
    /// A component instantiation.
    ComponentInstance,
}

/// An entity of the project.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EntityRef {
    pub library: String,
    pub name: String,
    pub path: PathBuf,
}

impl EntityRef {
    /// `library.name`.
    pub fn qualified(&self) -> String {
        format!("{}.{}", self.library, self.name)
    }
}

/// Generated code.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Generated {
    pub text: String,
    /// For a testbench, the file it is meant for, next to the entity.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
}

fn sources(world: &World) -> Vec<Source<'_>> {
    world
        .analysis
        .user_files()
        .iter()
        .map(|f| Source {
            path: &f.path,
            root: f.syntax(),
            libraries: f.libraries.clone(),
        })
        .collect()
}

/// The entities of the project, sorted by library and name.
pub fn entities(world: &mut World) -> Vec<EntityRef> {
    world.refresh();
    let mut out: Vec<EntityRef> = vfl_gen::find_entities(&sources(world), None)
        .into_iter()
        .map(|f| EntityRef {
            library: f.entity.library,
            name: f.entity.name,
            path: f.path,
        })
        .collect();
    out.sort_by_key(|e| (e.library.to_lowercase(), e.name.to_lowercase()));
    out.dedup();
    out
}

fn find(world: &World, entity: &str) -> Result<Found, String> {
    let mut found = vfl_gen::find_entities(&sources(world), Some(entity));
    match found.len() {
        0 => Err(format!("no entity `{entity}` in the project")),
        1 => Ok(found.pop().unwrap()),
        _ => Err(format!(
            "several entities are named `{entity}`; name one as `library.entity`"
        )),
    }
}

/// Generates `kind` for `entity` (`name` or `library.name`), with the
/// settings of the entity's directory.
pub fn generate(world: &mut World, kind: Kind, entity: &str) -> Result<Generated, String> {
    world.refresh();
    let found = find(world, entity)?;
    generate_for(world, kind, &found)
}

fn generate_for(world: &mut World, kind: Kind, found: &Found) -> Result<Generated, String> {
    let config = world.config(&found.path).clone();
    let entity = &found.entity;
    let text = match kind {
        Kind::Testbench => vfl_gen::testbench(entity, &config),
        Kind::Instance => vfl_gen::instance(entity, &config),
        Kind::Component => vfl_gen::component(entity, &config),
        Kind::ComponentInstance => vfl_gen::component_instance(entity, &config),
    }
    .map_err(|e| e.to_string())?;
    let path = (kind == Kind::Testbench).then(|| {
        let extension = found
            .path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("vhd");
        found.path.with_file_name(vfl_gen::testbench_file_name(
            entity,
            &config.generate,
            extension,
        ))
    });
    Ok(Generated { text, path })
}

/// `vfl/generate` parameters.
#[derive(Debug, Deserialize)]
pub struct GenerateParams {
    pub kind: Kind,
    pub entity: String,
}

pub fn generate_request(world: &mut World, params: GenerateParams) -> Result<Generated, String> {
    generate(world, params.kind, &params.entity)
}

pub const TESTBENCH_KIND: &str = "refactor.generate.testbench";

/// "Generate testbench for `e`" on an entity declaration of `file`
/// touching `start..end`, when its testbench file does not exist yet.
pub fn code_actions(
    world: &mut World,
    file: FileId,
    start: TextSize,
    end: TextSize,
) -> Vec<CodeActionOrCommand> {
    let analyzed = &world.analysis.files()[file as usize];
    let path = analyzed.path.clone();
    // Only the entities under the cursor, so that a request elsewhere costs nothing.
    let names: Vec<String> = vfl_gen::entity_units(&analyzed.syntax())
        .filter(|u| {
            u.children()
                .find(|n| n.kind() == vfl_syntax::SyntaxKind::ENTITY_DECL)
                .is_some_and(|d| d.text_range().start() <= end && start <= d.text_range().end())
        })
        .filter_map(|u| vfl_gen::Entity::from_unit(&u, "work").map(|e| e.name))
        .collect();
    let mut out = Vec::new();
    let mut seen = Vec::new();
    let found: Vec<Found> = names
        .iter()
        .flat_map(|name| vfl_gen::find_entities(&sources(world), Some(name)))
        .filter(|f| f.path == path && f.range.start() <= end && start <= f.range.end())
        .collect();
    for f in found {
        let Ok(generated) = generate_for(world, Kind::Testbench, &f) else {
            continue;
        };
        let Some(target) = generated.path else {
            continue;
        };
        if target.exists() || seen.contains(&target) {
            continue;
        }
        seen.push(target.clone());
        let Ok(uri) = Url::from_file_path(&target) else {
            continue;
        };
        let edit = WorkspaceEdit {
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
        };
        let name = target
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        out.push(CodeActionOrCommand::CodeAction(CodeAction {
            title: format!("Generate testbench for `{}` in {name}", f.entity.name),
            kind: Some(CodeActionKind::new(TESTBENCH_KIND)),
            edit: Some(edit),
            ..Default::default()
        }));
    }
    out
}

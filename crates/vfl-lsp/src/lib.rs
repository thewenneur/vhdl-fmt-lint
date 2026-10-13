//! VHDL language server.
//!
//! Speaks LSP over stdio ([`run_stdio`]). It analyzes the whole project
//! around the workspace folder, as `vfl lint` does, and offers
//! diagnostics with quick fixes, formatting, go to definition, find
//! references, hover, document and workspace symbols, rename and
//! completion.

mod handlers;
mod world;

use std::path::PathBuf;

use anyhow::Result;
use lsp_server::{Connection, ErrorCode, Message, Notification, Request, Response};
use lsp_types::notification::Notification as _;
use lsp_types::{
    CodeActionKind, CodeActionOptions, CodeActionProviderCapability, CompletionOptions,
    DidChangeWatchedFilesRegistrationOptions, FileSystemWatcher, GlobPattern, InitializeParams,
    OneOf, Registration, RegistrationParams, RenameOptions, ServerCapabilities, ServerInfo,
    TextDocumentSyncCapability, TextDocumentSyncKind, Url, notification as n,
};
use serde::de::DeserializeOwned;

pub use world::World;

pub fn capabilities() -> ServerCapabilities {
    ServerCapabilities {
        text_document_sync: Some(TextDocumentSyncCapability::Kind(
            TextDocumentSyncKind::INCREMENTAL,
        )),
        document_formatting_provider: Some(OneOf::Left(true)),
        definition_provider: Some(OneOf::Left(true)),
        references_provider: Some(OneOf::Left(true)),
        hover_provider: Some(lsp_types::HoverProviderCapability::Simple(true)),
        document_symbol_provider: Some(OneOf::Left(true)),
        workspace_symbol_provider: Some(OneOf::Left(true)),
        document_highlight_provider: Some(OneOf::Left(true)),
        rename_provider: Some(OneOf::Right(RenameOptions {
            prepare_provider: Some(true),
            work_done_progress_options: Default::default(),
        })),
        completion_provider: Some(CompletionOptions {
            trigger_characters: Some(vec![".".into()]),
            ..Default::default()
        }),
        code_action_provider: Some(CodeActionProviderCapability::Options(CodeActionOptions {
            code_action_kinds: Some(vec![
                CodeActionKind::QUICKFIX,
                CodeActionKind::new("source.fixAll.vfl"),
            ]),
            ..Default::default()
        })),
        ..Default::default()
    }
}

/// Runs the server on stdin and stdout until the client shuts it down.
pub fn run_stdio() -> Result<()> {
    let (connection, io) = Connection::stdio();
    run(&connection)?;
    drop(connection);
    io.join()?;
    Ok(())
}

/// Runs the server on a connection.
pub fn run(connection: &Connection) -> Result<()> {
    let (id, params) = connection.initialize_start()?;
    let params: InitializeParams = serde_json::from_value(params)?;
    let result = lsp_types::InitializeResult {
        capabilities: capabilities(),
        server_info: Some(ServerInfo {
            name: "vfl".into(),
            version: Some(env!("CARGO_PKG_VERSION").into()),
        }),
    };
    connection.initialize_finish(id, serde_json::to_value(result)?)?;

    #[allow(deprecated)]
    let root = params
        .workspace_folders
        .as_ref()
        .and_then(|f| f.first())
        .map(|f| f.uri.clone())
        .or(params.root_uri.clone())
        .and_then(|uri| uri.to_file_path().ok())
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."));
    let watch = params
        .capabilities
        .workspace
        .as_ref()
        .and_then(|w| w.did_change_watched_files)
        .and_then(|d| d.dynamic_registration)
        .unwrap_or(false);

    let mut server = Server {
        connection,
        world: World::new(&root),
        published: Vec::new(),
        shown_errors: Vec::new(),
        next_request: 0,
    };
    if watch {
        server.register_watchers()?;
    }
    server.publish()?;
    server.main_loop()
}

struct Server<'a> {
    connection: &'a Connection,
    world: World,
    /// Files with diagnostics shown in the client.
    published: Vec<Url>,
    /// Configuration errors already shown.
    shown_errors: Vec<String>,
    next_request: i32,
}

impl Server<'_> {
    fn main_loop(&mut self) -> Result<()> {
        while let Ok(message) = self.connection.receiver.recv() {
            match message {
                Message::Request(request) => {
                    if self.connection.handle_shutdown(&request)? {
                        return Ok(());
                    }
                    if self.world.dirty {
                        self.world.refresh();
                        self.publish()?;
                    }
                    let response = self.handle_request(request);
                    self.connection.sender.send(Message::Response(response))?;
                }
                Message::Notification(notification) => {
                    if notification.method == n::Exit::METHOD {
                        return Ok(());
                    }
                    self.handle_notification(notification);
                }
                Message::Response(_) => {}
            }
            // Analyze once the burst of edits is over.
            if self.world.dirty && self.connection.receiver.is_empty() {
                self.world.refresh();
                self.publish()?;
            }
        }
        Ok(())
    }

    fn register_watchers(&mut self) -> Result<()> {
        let watchers = ["**/*.{vhd,vhdl,vho,vht}", "**/{vfl,.vfl,vhdl_ls}.toml"]
            .iter()
            .map(|glob| FileSystemWatcher {
                glob_pattern: GlobPattern::String(glob.to_string()),
                kind: None,
            })
            .collect();
        let params = RegistrationParams {
            registrations: vec![Registration {
                id: "vfl-watch".into(),
                method: n::DidChangeWatchedFiles::METHOD.into(),
                register_options: Some(serde_json::to_value(
                    DidChangeWatchedFilesRegistrationOptions { watchers },
                )?),
            }],
        };
        self.next_request += 1;
        let request = Request::new(
            self.next_request.into(),
            "client/registerCapability".into(),
            params,
        );
        self.connection.sender.send(Message::Request(request))?;
        Ok(())
    }

    fn handle_request(&mut self, request: Request) -> Response {
        let id = request.id.clone();
        match handlers::request(&mut self.world, &request.method, request.params) {
            Ok(value) => Response::new_ok(id, value),
            Err(handlers::Error::Unknown) => Response::new_err(
                id,
                ErrorCode::MethodNotFound as i32,
                format!("unsupported request {}", request.method),
            ),
            Err(handlers::Error::Invalid(message)) => {
                Response::new_err(id, ErrorCode::InvalidParams as i32, message)
            }
            Err(handlers::Error::Failed(message)) => {
                Response::new_err(id, ErrorCode::RequestFailed as i32, message)
            }
        }
    }

    fn handle_notification(&mut self, notification: Notification) {
        let world = &mut self.world;
        match notification.method.as_str() {
            n::DidOpenTextDocument::METHOD => {
                if let Some(p) = params::<lsp_types::DidOpenTextDocumentParams>(notification.params)
                    && let Ok(path) = p.text_document.uri.to_file_path()
                {
                    if world.overlays.get(&path) != Some(&p.text_document.text) {
                        world.set_text(path, p.text_document.text);
                    } else {
                        world.overlays.insert(path, p.text_document.text);
                    }
                }
            }
            n::DidChangeTextDocument::METHOD => {
                if let Some(p) =
                    params::<lsp_types::DidChangeTextDocumentParams>(notification.params)
                    && let Ok(path) = p.text_document.uri.to_file_path()
                {
                    let mut text = world.overlays.get(&path).cloned().unwrap_or_default();
                    for change in p.content_changes {
                        match change.range {
                            Some(range) => {
                                let index = vfl_syntax::LineIndex::new(&text);
                                let start = usize::from(world::offset(&text, &index, range.start));
                                let end = usize::from(world::offset(&text, &index, range.end));
                                text.replace_range(start..end.max(start), &change.text);
                            }
                            None => text = change.text,
                        }
                    }
                    world.set_text(path, text);
                }
            }
            n::DidCloseTextDocument::METHOD => {
                if let Some(p) =
                    params::<lsp_types::DidCloseTextDocumentParams>(notification.params)
                    && let Ok(path) = p.text_document.uri.to_file_path()
                {
                    world.close(&path);
                }
            }
            n::DidChangeWatchedFiles::METHOD => {
                if let Some(p) =
                    params::<lsp_types::DidChangeWatchedFilesParams>(notification.params)
                {
                    for change in p.changes {
                        let Ok(path) = change.uri.to_file_path() else {
                            continue;
                        };
                        if path.extension().is_some_and(|e| e == "toml") {
                            world.reload();
                            break;
                        }
                        world.reread(&path);
                    }
                }
            }
            _ => {}
        }
    }

    /// Sends the diagnostics of every file, and clears those of files
    /// that no longer have any.
    fn publish(&mut self) -> Result<()> {
        let by_file = handlers::diagnostics(&self.world);
        let mut published = Vec::new();
        for (uri, diagnostics) in by_file {
            published.push(uri.clone());
            self.notify::<n::PublishDiagnostics>(lsp_types::PublishDiagnosticsParams {
                uri,
                diagnostics,
                version: None,
            })?;
        }
        for uri in std::mem::take(&mut self.published) {
            if !published.contains(&uri) {
                self.notify::<n::PublishDiagnostics>(lsp_types::PublishDiagnosticsParams {
                    uri,
                    diagnostics: Vec::new(),
                    version: None,
                })?;
            }
        }
        self.published = published;
        for error in std::mem::take(&mut self.world.config_errors) {
            if self.shown_errors.contains(&error) {
                continue;
            }
            self.shown_errors.push(error.clone());
            self.notify::<n::ShowMessage>(lsp_types::ShowMessageParams {
                typ: lsp_types::MessageType::WARNING,
                message: format!("vfl: {error}"),
            })?;
        }
        Ok(())
    }

    fn notify<N: n::Notification>(&self, params: N::Params) -> Result<()> {
        self.connection
            .sender
            .send(Message::Notification(Notification::new(
                N::METHOD.into(),
                params,
            )))?;
        Ok(())
    }
}

fn params<P: DeserializeOwned>(value: serde_json::Value) -> Option<P> {
    serde_json::from_value(value).ok()
}

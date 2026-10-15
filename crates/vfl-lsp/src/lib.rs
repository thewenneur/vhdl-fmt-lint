//! VHDL language server.
//!
//! Speaks LSP over stdio ([`run_stdio`]). It analyzes the whole project
//! around the workspace folder, as `vfl lint` does, and offers
//! diagnostics with quick fixes, formatting, go to definition, find
//! references, hover with types, signature help, document and workspace
//! symbols, rename, completion and semantic tokens. [`editor`] answers hover, signature
//! help and completion directly, for editors that link it.

mod actions;
mod associate;
pub mod editor;
pub mod format;
pub mod generate;
mod handlers;
mod refactor;
mod semantic;
mod types;
mod verilog;
mod world;

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::mpsc;

use anyhow::Result;
use lsp_server::{Connection, ErrorCode, Message, Notification, Request, RequestId, Response};
use lsp_types::notification::Notification as _;
use lsp_types::{
    CodeActionKind, CodeActionOptions, CodeActionProviderCapability, CompletionOptions,
    DidChangeWatchedFilesRegistrationOptions, FileSystemWatcher, GlobPattern, InitializeParams,
    OneOf, Registration, RegistrationParams, RenameOptions, ServerCapabilities, ServerInfo,
    TextDocumentSyncCapability, TextDocumentSyncKind, Url, notification as n, request::Shutdown,
};
use rustc_hash::{FxHashMap, FxHashSet};
use serde::de::DeserializeOwned;

pub use world::World;

/// Answers one request against `world`, as the server would: for
/// examples and benchmarks. `None` when the method is unknown or fails.
#[doc(hidden)]
pub fn handle(
    world: &mut World,
    method: &str,
    params: serde_json::Value,
) -> Option<serde_json::Value> {
    handlers::request(world, method, params).ok()
}

pub fn capabilities() -> ServerCapabilities {
    ServerCapabilities {
        text_document_sync: Some(TextDocumentSyncCapability::Kind(
            TextDocumentSyncKind::INCREMENTAL,
        )),
        document_formatting_provider: Some(OneOf::Left(true)),
        document_range_formatting_provider: Some(OneOf::Left(true)),
        document_on_type_formatting_provider: Some(lsp_types::DocumentOnTypeFormattingOptions {
            first_trigger_character: ";".into(),
            more_trigger_character: Some(vec!["\n".into()]),
        }),
        definition_provider: Some(OneOf::Left(true)),
        references_provider: Some(OneOf::Left(true)),
        hover_provider: Some(lsp_types::HoverProviderCapability::Simple(true)),
        document_symbol_provider: Some(OneOf::Left(true)),
        folding_range_provider: Some(lsp_types::FoldingRangeProviderCapability::Simple(true)),
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
        signature_help_provider: Some(lsp_types::SignatureHelpOptions {
            trigger_characters: Some(vec!["(".into(), ",".into()]),
            retrigger_characters: Some(vec![",".into()]),
            work_done_progress_options: Default::default(),
        }),
        semantic_tokens_provider: Some(
            lsp_types::SemanticTokensServerCapabilities::SemanticTokensOptions(
                lsp_types::SemanticTokensOptions {
                    legend: semantic::legend(),
                    range: Some(true),
                    full: Some(lsp_types::SemanticTokensFullOptions::Delta { delta: Some(true) }),
                    work_done_progress_options: Default::default(),
                },
            ),
        ),
        code_action_provider: Some(CodeActionProviderCapability::Options(CodeActionOptions {
            code_action_kinds: Some(vec![
                CodeActionKind::QUICKFIX,
                CodeActionKind::new("source.fixAll.vfl"),
                CodeActionKind::new(generate::TESTBENCH_KIND),
            ]),
            resolve_provider: Some(true),
            ..Default::default()
        })),
        ..Default::default()
    }
}

/// Applies the changes of a `textDocument/didChange` notification to the
/// text of a document. Positions past a line or the text are clamped.
#[doc(hidden)]
pub fn apply_changes(text: &mut String, changes: Vec<lsp_types::TextDocumentContentChangeEvent>) {
    for change in changes {
        match change.range {
            Some(range) => {
                let index = vfl_syntax::LineIndex::new(text);
                let start = usize::from(world::offset(text, &index, range.start));
                let end = usize::from(world::offset(text, &index, range.end));
                text.replace_range(start..end.max(start), &change.text);
            }
            None => *text = change.text,
        }
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

    // `lsp_server` hands messages over a rendezvous channel, which always
    // looks empty: read them into a queue of our own so the loop can see
    // the edits that are already waiting and analyze once after them.
    let (sender, inbox) = mpsc::channel();
    let receiver = connection.receiver.clone();
    std::thread::spawn(move || {
        for message in receiver {
            if sender.send(message).is_err() {
                break;
            }
        }
    });

    let mut server = Server {
        connection,
        inbox,
        queue: VecDeque::new(),
        cancelled: FxHashSet::default(),
        world: World::new(&root),
        published: FxHashMap::default(),
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
    /// Messages from the client, as they arrive.
    inbox: mpsc::Receiver<Message>,
    /// Messages received but not handled yet.
    queue: VecDeque<Message>,
    /// Queued requests the client cancelled.
    cancelled: FxHashSet<RequestId>,
    world: World,
    /// The diagnostics shown in the client, by file.
    published: FxHashMap<Url, Vec<lsp_types::Diagnostic>>,
    /// Configuration errors already shown.
    shown_errors: Vec<String>,
    next_request: i32,
}

impl Server<'_> {
    fn main_loop(&mut self) -> Result<()> {
        loop {
            self.receive_waiting();
            while self.queue.is_empty() {
                let Ok(message) = self.inbox.recv() else {
                    return Ok(());
                };
                self.accept(message);
            }
            let Some(message) = self.queue.pop_front() else {
                unreachable!("the queue is not empty");
            };
            match message {
                Message::Request(request) => self.answer(request)?,
                Message::Notification(notification) => {
                    if notification.method == n::Exit::METHOD {
                        return Ok(());
                    }
                    self.handle_notification(notification);
                }
                Message::Response(_) => {}
            }
            // Analyze once the burst of edits is over: typing faster than
            // the project analyzes must not queue one analysis per key.
            self.receive_waiting();
            if self.world.dirty && self.queue.is_empty() {
                self.world.refresh();
                self.publish()?;
            }
        }
    }

    /// Answers a request, after analyzing the latest edits if it needs them.
    fn answer(&mut self, request: Request) -> Result<()> {
        let response = if self.cancelled.remove(&request.id) {
            Response::new_err(
                request.id,
                ErrorCode::RequestCanceled as i32,
                "canceled".into(),
            )
        } else if request.method == <Shutdown as lsp_types::request::Request>::METHOD {
            Response::new_ok(request.id, ())
        } else {
            if self.world.dirty && handlers::needs_analysis(&request.method) {
                self.world.refresh();
                self.publish()?;
            }
            self.handle_request(request)
        };
        self.connection.sender.send(Message::Response(response))?;
        Ok(())
    }

    /// Queues the messages that already arrived.
    fn receive_waiting(&mut self) {
        while let Ok(message) = self.inbox.try_recv() {
            self.accept(message);
        }
    }

    /// Queues a message, or notes which queued request it cancels: the
    /// editor cancels the requests it no longer needs, such as the
    /// completion of each previous key.
    fn accept(&mut self, message: Message) {
        if let Message::Notification(notification) = &message
            && notification.method == n::Cancel::METHOD
        {
            if let Some(p) = params::<lsp_types::CancelParams>(notification.params.clone()) {
                let id = match p.id {
                    lsp_types::NumberOrString::Number(id) => RequestId::from(id),
                    lsp_types::NumberOrString::String(id) => RequestId::from(id),
                };
                let queued = self
                    .queue
                    .iter()
                    .any(|m| matches!(m, Message::Request(r) if r.id == id));
                if queued {
                    self.cancelled.insert(id);
                }
            }
            return;
        }
        self.queue.push_back(message);
    }

    fn register_watchers(&mut self) -> Result<()> {
        let watchers = [
            "**/*.{vhd,vhdl,vho,vht}",
            "**/*.{v,vh,sv,svh}",
            "**/{vfl,.vfl,vhdl_ls,.vhdl_ls}.toml",
            "**/run.py",
            "**/*.{core,xpr,qsf,qip,tcl}",
        ]
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
                    world.set_text(path, p.text_document.text);
                }
            }
            n::DidChangeTextDocument::METHOD => {
                if let Some(p) =
                    params::<lsp_types::DidChangeTextDocumentParams>(notification.params)
                    && let Ok(path) = p.text_document.uri.to_file_path()
                {
                    let mut text = world.overlays.get(&path).cloned().unwrap_or_default();
                    apply_changes(&mut text, p.content_changes);
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
                        if path.extension().is_some_and(|e| e == "toml")
                            || vfl_project::is_tool_file(&path)
                        {
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

    /// Sends the diagnostics of every file whose diagnostics changed, and
    /// clears those of files that no longer have any.
    fn publish(&mut self) -> Result<()> {
        let mut previous = std::mem::take(&mut self.published);
        for (uri, diagnostics) in handlers::diagnostics(&self.world) {
            if previous.remove(&uri).as_ref() != Some(&diagnostics) {
                self.notify::<n::PublishDiagnostics>(lsp_types::PublishDiagnosticsParams {
                    uri: uri.clone(),
                    diagnostics: diagnostics.clone(),
                    version: None,
                })?;
            }
            self.published.insert(uri, diagnostics);
        }
        for uri in previous.into_keys() {
            self.notify::<n::PublishDiagnostics>(lsp_types::PublishDiagnosticsParams {
                uri,
                diagnostics: Vec::new(),
                version: None,
            })?;
        }
        let notices = self.world.notices.clone();
        for error in std::mem::take(&mut self.world.config_errors)
            .into_iter()
            .chain(notices)
        {
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Edits that wait in the queue are analyzed once, and cancelled
    /// requests are not answered.
    #[test]
    fn waiting_edits_are_analyzed_once() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.vhd");
        let text = "entity a is\nend entity;\narchitecture rtl of a is\n  signal s : bit;\nbegin\nend architecture;\n";
        std::fs::write(&path, text).unwrap();
        let uri = Url::from_file_path(&path).unwrap();

        let (server, client) = Connection::memory();
        let (sender, inbox) = mpsc::channel();
        let notify = |method: &str, params: serde_json::Value| {
            sender
                .send(Message::Notification(Notification::new(
                    method.into(),
                    params,
                )))
                .unwrap();
        };
        notify(
            "textDocument/didOpen",
            json!({ "textDocument": { "uri": uri, "languageId": "vhdl", "version": 0, "text": text } }),
        );
        for version in 1..20 {
            notify(
                "textDocument/didChange",
                json!({
                    "textDocument": { "uri": uri, "version": version },
                    "contentChanges": [{ "text": format!("{text}-- {version}\n") }],
                }),
            );
        }
        let at =
            json!({ "textDocument": { "uri": uri }, "position": { "line": 3, "character": 9 } });
        let request = |id: i32, method: &str| {
            sender
                .send(Message::Request(Request::new(
                    id.into(),
                    method.into(),
                    at.clone(),
                )))
                .unwrap();
        };
        request(1, "textDocument/hover");
        notify("$/cancelRequest", json!({ "id": 1 }));
        request(2, "textDocument/hover");
        drop(sender);

        let mut server = Server {
            connection: &server,
            inbox,
            queue: VecDeque::new(),
            cancelled: FxHashSet::default(),
            world: World::new(dir.path()),
            published: FxHashMap::default(),
            shown_errors: Vec::new(),
            next_request: 0,
        };
        server.main_loop().unwrap();

        let messages: Vec<Message> = client.receiver.try_iter().collect();
        let published = messages
            .iter()
            .filter(|m| matches!(m, Message::Notification(n) if n.method == "textDocument/publishDiagnostics"))
            .count();
        assert_eq!(published, 1, "{messages:?}");
        let responses: Vec<&Response> = messages
            .iter()
            .filter_map(|m| match m {
                Message::Response(r) => Some(r),
                _ => None,
            })
            .collect();
        assert_eq!(responses.len(), 2);
        assert_eq!(responses[0].id, 1.into());
        assert_eq!(
            responses[0].error.as_ref().map(|e| e.code),
            Some(ErrorCode::RequestCanceled as i32)
        );
        assert_eq!(responses[1].id, 2.into());
        assert!(responses[1].result.as_ref().is_some_and(|r| !r.is_null()));
    }
}

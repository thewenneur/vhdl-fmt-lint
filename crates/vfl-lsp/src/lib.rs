//! VHDL language server.
//!
//! Speaks LSP over stdio ([`run_stdio`]). It analyzes the whole project
//! around the workspace folder, as `vfl lint` does, and offers
//! diagnostics with quick fixes, formatting, go to definition, find
//! references, hover with types, signature help, document and workspace
//! symbols, rename, completion and semantic tokens; go to implementation,
//! type definition and declaration, linked editing, selection ranges
//! (`navigate`), inlay hints (`inlay`), and instance-count code lenses
//! and the call hierarchy (`hierarchy`). [`editor`] answers hover, signature
//! help and completion directly, for editors that link it.

mod actions;
mod associate;
pub mod design_tree;
pub mod editor;
pub mod format;
pub mod generate;
mod handlers;
mod hierarchy;
mod inlay;
mod navigate;
mod publish;
mod refactor;
mod semantic;
pub mod settings;
mod types;
mod vendor;
mod verilog;
mod watch;
mod world;

use std::collections::VecDeque;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::sync::mpsc;

use anyhow::Result;
use lsp_server::{Connection, ErrorCode, Message, Notification, Request, RequestId, Response};
use lsp_types::notification::Notification as _;
use lsp_types::{
    CodeActionKind, CodeActionOptions, CodeActionProviderCapability, CompletionOptions,
    DidChangeWatchedFilesRegistrationOptions, FileSystemWatcher, GlobPattern, InitializeParams,
    OneOf, Registration, RegistrationParams, RenameOptions, ServerCapabilities, ServerInfo,
    TextDocumentSyncCapability, TextDocumentSyncKind, notification as n, request::Shutdown,
};
use rustc_hash::FxHashSet;
use serde::de::DeserializeOwned;

pub use hierarchy::SHOW_REFERENCES;
pub use publish::Published;
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
                CodeActionKind::new(settings::GENERATE_CONFIG_KIND),
            ]),
            resolve_provider: Some(true),
            ..Default::default()
        })),
        // Navigation, hints and hierarchies: see navigate.rs, inlay.rs and
        // hierarchy.rs.
        implementation_provider: Some(lsp_types::ImplementationProviderCapability::Simple(true)),
        type_definition_provider: Some(lsp_types::TypeDefinitionProviderCapability::Simple(true)),
        declaration_provider: Some(lsp_types::DeclarationCapability::Simple(true)),
        linked_editing_range_provider: Some(
            lsp_types::LinkedEditingRangeServerCapabilities::Simple(true),
        ),
        selection_range_provider: Some(lsp_types::SelectionRangeProviderCapability::Simple(true)),
        inlay_hint_provider: Some(OneOf::Left(true)),
        code_lens_provider: Some(lsp_types::CodeLensOptions {
            resolve_provider: Some(true),
        }),
        call_hierarchy_provider: Some(lsp_types::CallHierarchyServerCapability::Simple(true)),
        ..Default::default()
    }
}

/// Applies the changes of a `textDocument/didChange` notification to the
/// text of a document. Positions past a line or the text are clamped.
#[doc(hidden)]
pub fn apply_changes(text: &mut String, changes: Vec<lsp_types::TextDocumentContentChangeEvent>) {
    // Where each line starts: found once for all the changes, then kept up
    // to date after each.
    let mut starts: Option<Vec<usize>> = None;
    let count = changes.len();
    for (i, change) in changes.into_iter().enumerate() {
        match change.range {
            Some(range) => {
                let lines = starts.get_or_insert_with(|| {
                    std::iter::once(0)
                        .chain(text.match_indices('\n').map(|(i, _)| i + 1))
                        .collect()
                });
                let offset = |p: lsp_types::Position| {
                    let start = lines.get(p.line as usize).copied();
                    usize::from(world::offset_from(text, start, p))
                };
                let start = offset(range.start);
                let end = offset(range.end).max(start);
                text.replace_range(start..end, &change.text);
                if i + 1 < count {
                    shift_lines(lines, start, end, &change.text);
                }
            }
            None => {
                *text = change.text;
                starts = None;
            }
        }
    }
}

/// Updates the line starts of a text after `start..end` was replaced by
/// `new`.
fn shift_lines(starts: &mut Vec<usize>, start: usize, end: usize, new: &str) {
    let low = starts.partition_point(|&s| s <= start);
    let high = starts.partition_point(|&s| s <= end);
    let added = new.match_indices('\n').map(|(i, _)| start + i + 1);
    starts.splice(low..high, added);
    let after = low + new.matches('\n').count();
    for s in &mut starts[after..] {
        *s = *s + new.len() - (end - start);
    }
}

/// Applies the changes of a `textDocument/didChange` notification to the
/// open document at `path`, as the server does.
#[doc(hidden)]
pub fn did_change(
    world: &mut World,
    path: PathBuf,
    changes: Vec<lsp_types::TextDocumentContentChangeEvent>,
) {
    // The document's text is changed in place, then parsed: its only copy
    // is the one the parse keeps.
    let mut text = world.overlays.remove(&path).unwrap_or_default();
    apply_changes(&mut text, changes);
    world.set_text(path, text);
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
    let events = sender.clone();
    let sender_for_analyzer = sender.clone();
    let receiver = connection.receiver.clone();
    std::thread::spawn(move || {
        for message in receiver {
            if sender.send(message).is_err() {
                return;
            }
        }
        // The client is gone: the file watcher's sender must not keep
        // the server waiting for more.
        let exit = Notification::new(n::Exit::METHOD.into(), serde_json::Value::Null);
        let _ = sender.send(Message::Notification(exit));
    });

    // The editor's settings: the config of files no config file applies to.
    let (defaults, settings_error) = match params
        .initialization_options
        .as_ref()
        .and_then(settings::editor_config)
    {
        Some(Ok(config)) => (config, None),
        Some(Err(error)) => (vfl_config::Config::default(), Some(error)),
        None => (vfl_config::Config::default(), None),
    };

    let progress = params
        .capabilities
        .window
        .as_ref()
        .and_then(|w| w.work_done_progress)
        .unwrap_or(false);
    // Without file watchers of the editor's, the server watches itself,
    // from before the load: what changes while it lasts is read after.
    let root = world::uri_form(&root);
    let _watcher =
        (!watch).then(|| watch::spawn(vfl_project::find_root(&root).dir, defaults.clone(), events));
    let world = if progress {
        load_with_progress(connection, &root, defaults)?
    } else {
        World::with_defaults(&root, defaults)
    };
    let mut server = Server::new(connection, inbox, world);
    server.analyzer = Some(Analyzer::spawn(sender_for_analyzer));
    if let Some(options) = &params.initialization_options {
        inlay::editor_settings(&mut server.world, options);
    }
    if let Some(error) = settings_error {
        server.warn(&error)?;
    }
    if watch {
        server.register_watchers()?;
    }
    server.publish()?;
    server.main_loop()
}

/// Loads the project, showing how it goes in the editor (`$/progress`).
fn load_with_progress(
    connection: &Connection,
    root: &std::path::Path,
    defaults: vfl_config::Config,
) -> Result<World> {
    use lsp_types::{
        ProgressParams, ProgressParamsValue, WorkDoneProgress, WorkDoneProgressBegin,
        WorkDoneProgressCreateParams, WorkDoneProgressEnd, WorkDoneProgressReport,
    };
    let token = lsp_types::NumberOrString::String("vfl/load".into());
    let request = Request::new(
        RequestId::from("vfl/load".to_owned()),
        "window/workDoneProgress/create".into(),
        WorkDoneProgressCreateParams {
            token: token.clone(),
        },
    );
    connection.sender.send(Message::Request(request))?;
    let send = |progress: WorkDoneProgress| {
        let params = ProgressParams {
            token: token.clone(),
            value: ProgressParamsValue::WorkDone(progress),
        };
        // A failure to send shows when answering.
        let _ = connection
            .sender
            .send(Message::Notification(Notification::new(
                n::Progress::METHOD.into(),
                params,
            )));
    };
    send(WorkDoneProgress::Begin(WorkDoneProgressBegin {
        title: "vfl".into(),
        message: Some("Loading the project".into()),
        percentage: Some(0),
        cancellable: Some(false),
    }));
    // Parsing is most of the loading; one report per percent at most.
    let last = std::sync::atomic::AtomicU32::new(0);
    let report = |progress: world::Progress| {
        let (message, percentage) = match progress {
            world::Progress::Parsing { done, total } => {
                let percentage = (done * 80 / total.max(1)) as u32;
                if last.fetch_max(percentage, std::sync::atomic::Ordering::Relaxed) >= percentage
                    && done != total
                {
                    return;
                }
                (format!("Parsing {done}/{total} files"), percentage)
            }
            world::Progress::Analyzing { files } => (format!("Analyzing {files} files"), 80),
        };
        send(WorkDoneProgress::Report(WorkDoneProgressReport {
            message: Some(message),
            percentage: Some(percentage),
            cancellable: None,
        }));
    };
    let world = World::with_progress(root, defaults, &report);
    let files = world.project.files.len() + world.project.verilog.len();
    send(WorkDoneProgress::End(WorkDoneProgressEnd {
        message: Some(format!("{files} files")),
    }));
    Ok(world)
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
    /// The diagnostics shown in the client.
    published: Published,
    /// Configuration errors already shown, while they last.
    shown_errors: Vec<String>,
    next_request: i32,
    /// Files created, changed or deleted on disk, not re-read yet: those
    /// of a burst of events are re-read together.
    changed_files: Vec<PathBuf>,
    /// Whether a config or library list changed on disk, which reloads
    /// the project.
    reload: bool,
    /// The thread that analyzes, if any: without one, the server analyzes
    /// itself, and answers nothing meanwhile.
    analyzer: Option<Analyzer>,
    /// Whether the analyzer is analyzing. The world has no analysis
    /// meanwhile: requests that need one wait for it.
    analyzing: bool,
    /// The open FPGA vendor files, linted apart from the project.
    vendor: vendor::Documents,
}

/// The internal notification the analyzer wakes the server with when an
/// analysis is done.
const ANALYZED: &str = "vfl/analyzed";

/// A thread that analyzes the project, so that the server reads edits,
/// formats and takes cancellations while it does.
struct Analyzer {
    jobs: mpsc::Sender<world::Refresh>,
    /// The result of each job, or the message of the panic it hit.
    done: mpsc::Receiver<Result<world::Refreshed, String>>,
}

impl Analyzer {
    /// Starts the thread, which sends `ANALYZED` to `wake` after each job.
    fn spawn(wake: mpsc::Sender<Message>) -> Analyzer {
        let (jobs, inbox) = mpsc::channel::<world::Refresh>();
        let (outbox, done) = mpsc::channel();
        // Without the thread, sending a job fails, and the server runs it.
        let _ = std::thread::Builder::new()
            .name("vfl-analysis".into())
            .spawn(move || {
                for job in inbox {
                    if outbox.send(guard("analysis", || job.run())).is_err() {
                        return;
                    }
                    let wakeup = Notification::new(ANALYZED.into(), serde_json::Value::Null);
                    if wake.send(Message::Notification(wakeup)).is_err() {
                        return;
                    }
                }
            });
        Analyzer { jobs, done }
    }
}

impl<'a> Server<'a> {
    fn new(connection: &'a Connection, inbox: mpsc::Receiver<Message>, world: World) -> Self {
        Server {
            connection,
            inbox,
            queue: VecDeque::new(),
            cancelled: FxHashSet::default(),
            world,
            published: Published::default(),
            shown_errors: Vec::new(),
            next_request: 0,
            changed_files: Vec::new(),
            reload: false,
            analyzer: None,
            analyzing: false,
            vendor: vendor::Documents::default(),
        }
    }

    /// Re-reads the files changed on disk since the last time.
    fn reread_changed(&mut self) {
        if std::mem::take(&mut self.reload) {
            self.changed_files.clear();
            self.world.reload();
        } else if !self.changed_files.is_empty() {
            let mut paths = std::mem::take(&mut self.changed_files);
            paths.sort();
            paths.dedup();
            self.world.reread_all(&paths);
        }
    }

    fn main_loop(&mut self) -> Result<()> {
        loop {
            self.receive_waiting();
            self.take_analysis()?;
            // Analyze once the burst of edits is over: typing faster than
            // the project analyzes must not queue one analysis per key.
            // Files changed on disk are re-read once a burst of them is
            // over too. A request that needs the latest edits analyzed
            // waits for that.
            let waits = self.waits();
            if !self.analyzing && (self.queue.is_empty() || waits) {
                guard("file changes", || self.reread_changed()).unwrap_or(());
                if self.world.dirty {
                    self.analyze()?;
                    continue;
                }
            }
            // Meanwhile edits are read and documents formatted, but not
            // past a request that waits: what follows it comes after it.
            if self.queue.is_empty() || waits {
                let Ok(message) = self.inbox.recv() else {
                    return Ok(());
                };
                self.accept(message);
                continue;
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
                    let method = notification.method.clone();
                    guard(&method, || self.handle_notification(notification)).unwrap_or(Ok(()))?;
                }
                Message::Response(_) => {}
            }
        }
    }

    /// Whether the next message is a request that waits for the analysis
    /// of the latest edits, those on disk included.
    fn waits(&self) -> bool {
        (self.analyzing || self.world.dirty || self.reload || !self.changed_files.is_empty())
            && matches!(self.queue.front(), Some(Message::Request(r))
                if handlers::needs_analysis(&r.method)
                    && !self.cancelled.contains(&r.id)
                    && r.method != <Shutdown as lsp_types::request::Request>::METHOD)
    }

    /// Analyzes the latest edits: starts the analyzer on them, or, without
    /// one, analyzes them now.
    fn analyze(&mut self) -> Result<()> {
        let Ok(Some(job)) = guard("analysis", || self.world.start_refresh()) else {
            return Ok(());
        };
        let job = match &self.analyzer {
            Some(analyzer) => match analyzer.jobs.send(job) {
                Ok(()) => {
                    self.analyzing = true;
                    return Ok(());
                }
                Err(mpsc::SendError(job)) => job,
            },
            None => job,
        };
        self.analyzed(guard("analysis", || job.run()))
    }

    /// Takes the analysis the analyzer finished, if it did.
    fn take_analysis(&mut self) -> Result<()> {
        if !self.analyzing {
            return Ok(());
        }
        let Some(analyzer) = &self.analyzer else {
            return Ok(());
        };
        match analyzer.done.try_recv() {
            Ok(done) => {
                self.analyzing = false;
                self.analyzed(done)
            }
            Err(mpsc::TryRecvError::Empty) => Ok(()),
            Err(mpsc::TryRecvError::Disconnected) => {
                // The thread is gone: the server analyzes from now on.
                self.analyzing = false;
                self.analyzer = None;
                self.world.refresh_failed();
                self.world.dirty = true;
                Ok(())
            }
        }
    }

    /// Takes the result of an analysis and publishes its diagnostics. A
    /// failed one leaves those shown as they were.
    fn analyzed(&mut self, done: Result<world::Refreshed, String>) -> Result<()> {
        match done {
            Ok(done) => {
                self.world.finish_refresh(done);
                if !self.vendor.is_empty() {
                    let sent = guard("vendor files", || self.vendor.analyzed(&mut self.world));
                    for params in sent.unwrap_or_default() {
                        self.notify::<n::PublishDiagnostics>(params)?;
                    }
                }
                guard("analysis", || self.publish()).unwrap_or(Ok(()))
            }
            Err(_) => {
                self.world.refresh_failed();
                Ok(())
            }
        }
    }

    /// Answers a request; one that needs the analysis has it up to date.
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
            let (id, method) = (request.id.clone(), request.method.clone());
            guard(&method, || self.handle_request(request)).unwrap_or_else(|message| {
                Response::new_err(
                    id,
                    ErrorCode::InternalError as i32,
                    format!("vfl: internal error in {method}: {message}"),
                )
            })
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
            && notification.method == ANALYZED
        {
            // It only woke the server: the main loop takes the analysis.
            return;
        }
        // Vendor files are handled as they come, apart from the project.
        let message = match self.vendor_message(message) {
            Some(message) => message,
            None => return,
        };
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

    /// Handles a message about an open vendor file (`.qsf`, `.sdc`, ...):
    /// lints it, or answers a request on it with nothing. Other messages
    /// are given back.
    fn vendor_message(&mut self, message: Message) -> Option<Message> {
        fn target(params: &serde_json::Value) -> Option<lsp_types::Url> {
            let uri = params.get("textDocument")?.get("uri")?.as_str()?;
            // A look at the extension first: HDL edits pay nothing more.
            if !vendor::may_be_vendor_uri(uri) {
                return None;
            }
            lsp_types::Url::parse(uri)
                .ok()
                .filter(vendor::is_vendor_uri)
        }
        match &message {
            Message::Notification(notification) => {
                let Some(uri) = target(&notification.params) else {
                    return Some(message);
                };
                let world = &mut self.world;
                let documents = &mut self.vendor;
                let method = notification.method.as_str();
                let sent = guard(method, || match method {
                    n::DidOpenTextDocument::METHOD => {
                        params::<lsp_types::DidOpenTextDocumentParams>(notification.params.clone())
                            .and_then(|p| documents.open(world, uri, p.text_document.text))
                    }
                    n::DidChangeTextDocument::METHOD => {
                        params::<lsp_types::DidChangeTextDocumentParams>(
                            notification.params.clone(),
                        )
                        .and_then(|p| documents.change(world, &uri, p.content_changes))
                    }
                    n::DidCloseTextDocument::METHOD => documents.close(&uri),
                    _ => None,
                });
                if let Ok(Some(params)) = sent {
                    let _ = self.notify::<n::PublishDiagnostics>(params);
                }
                None
            }
            Message::Request(request) => {
                if target(&request.params).is_none() {
                    return Some(message);
                }
                let response = Response::new_ok(request.id.clone(), serde_json::Value::Null);
                let _ = self.connection.sender.send(Message::Response(response));
                None
            }
            Message::Response(_) => Some(message),
        }
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
        #[cfg(test)]
        if request.method == "vfl/test/panic" {
            panic!("a test panic");
        }
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

    fn handle_notification(&mut self, notification: Notification) -> Result<()> {
        let world = &mut self.world;
        match notification.method.as_str() {
            #[cfg(test)]
            "vfl/test/panic" => panic!("a test panic"),
            n::DidChangeConfiguration::METHOD => {
                if let Some(p) =
                    params::<lsp_types::DidChangeConfigurationParams>(notification.params)
                {
                    inlay::editor_settings(world, &p.settings);
                    match settings::editor_config(&p.settings) {
                        Some(Ok(config)) => world.set_defaults(config),
                        Some(Err(error)) => self.warn(&error)?,
                        None => {}
                    }
                }
            }
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
                    did_change(world, path, p.content_changes);
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
                            self.reload = true;
                        } else {
                            self.changed_files.push(path);
                        }
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// Shows a warning in the editor.
    fn warn(&self, message: &str) -> Result<()> {
        self.notify::<n::ShowMessage>(lsp_types::ShowMessageParams {
            typ: lsp_types::MessageType::WARNING,
            message: format!("vfl: {message}"),
        })
    }

    /// Sends the diagnostics of every file whose diagnostics changed, and
    /// clears those of files that no longer have any.
    fn publish(&mut self) -> Result<()> {
        for params in self.published.update(&self.world) {
            self.notify::<n::PublishDiagnostics>(params)?;
        }
        // Each error is shown once while it lasts: fixed, then broken
        // again, it is shown again.
        let current: Vec<String> = (self.world.config_errors.iter())
            .chain(&self.world.notices)
            .cloned()
            .collect();
        self.shown_errors.retain(|e| current.contains(e));
        for error in current {
            if self.shown_errors.contains(&error) {
                continue;
            }
            self.warn(&error)?;
            self.shown_errors.push(error);
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

/// Runs `f`, catching a panic so that one bug fails one request, not the
/// whole server: the panic is logged to stderr and its message returned.
fn guard<T>(what: &str, f: impl FnOnce() -> T) -> Result<T, String> {
    catch_unwind(AssertUnwindSafe(f)).map_err(|payload| {
        let message = payload
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| payload.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "unknown panic".into());
        eprintln!("vfl: internal error in {what}, recovered: {message}");
        message
    })
}

fn params<P: DeserializeOwned>(value: serde_json::Value) -> Option<P> {
    serde_json::from_value(value).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use lsp_types::Url;
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

        let mut server = Server::new(&server, inbox, World::new(dir.path()));
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

    /// While the analyzer analyzes, edits are read, documents formatted
    /// and requests waiting for the analysis cancelled; those that are
    /// not get the analysis of the latest edits.
    #[test]
    fn the_server_answers_while_it_analyzes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.vhd");
        let text = "entity a is\nend entity;\narchitecture rtl of a is\n  signal s : bit;\nbegin\nend architecture;\n";
        std::fs::write(&path, text).unwrap();
        let uri = Url::from_file_path(&path).unwrap();
        let (server, client) = Connection::memory();
        let (sender, inbox) = mpsc::channel();
        let mut server = Server::new(&server, inbox, World::new(dir.path()));
        // An analyzer the test runs the jobs of.
        let (jobs, started) = mpsc::channel();
        let (finished, done) = mpsc::channel();
        server.analyzer = Some(Analyzer { jobs, done });

        let notify = |method: &str, params: serde_json::Value| {
            let message = Message::Notification(Notification::new(method.into(), params));
            sender.send(message).unwrap();
        };
        let request = |id: i32, method: &str, params: serde_json::Value| {
            let message = Message::Request(Request::new(id.into(), method.into(), params));
            sender.send(message).unwrap();
        };
        let response = || loop {
            match client.receiver.recv().unwrap() {
                Message::Response(r) => return r,
                _ => continue,
            }
        };
        let hover =
            json!({ "textDocument": { "uri": uri }, "position": { "line": 3, "character": 9 } });
        std::thread::scope(|scope| {
            scope.spawn(|| server.main_loop().unwrap());
            notify(
                "textDocument/didOpen",
                json!({ "textDocument": { "uri": uri, "languageId": "vhdl", "version": 0,
                    "text": format!("{text}-- 1\n") } }),
            );
            let job: world::Refresh = started.recv().unwrap();
            request(1, "textDocument/hover", hover.clone());
            request(2, "textDocument/hover", hover.clone());
            // Not past the waiting hovers: edits after them wait too.
            notify(
                "textDocument/didChange",
                json!({ "textDocument": { "uri": uri, "version": 1 },
                    "contentChanges": [{ "text": format!("{text}-- 2\n") }] }),
            );
            notify("$/cancelRequest", json!({ "id": 1 }));
            let cancelled = response();
            assert_eq!(cancelled.id, 1.into());
            assert_eq!(
                cancelled.error.map(|e| e.code),
                Some(ErrorCode::RequestCanceled as i32)
            );
            finished.send(Ok(job.run())).unwrap();
            notify(ANALYZED, serde_json::Value::Null);
            let answered = response();
            assert_eq!(answered.id, 2.into());
            assert!(answered.result.is_some_and(|r| !r.is_null()));

            // The edit after the hover starts another analysis, during
            // which formatting, which needs none, is answered.
            let job: world::Refresh = started.recv().unwrap();
            request(
                3,
                "textDocument/formatting",
                json!({ "textDocument": { "uri": uri }, "options": { "tabSize": 2, "insertSpaces": true } }),
            );
            let formatted = response();
            assert_eq!(formatted.id, 3.into());
            assert!(formatted.result.is_some());
            finished.send(Ok(job.run())).unwrap();
            notify(ANALYZED, serde_json::Value::Null);
            request(4, "textDocument/hover", hover.clone());
            assert_eq!(response().id, 4.into());
            notify("exit", serde_json::Value::Null);
        });
        assert!(server.world.overlays[&path].ends_with("-- 2\n"));
    }

    /// Runs `server` on `messages`, as if they had just arrived, and
    /// returns what it sent.
    fn step(server: &mut Server, client: &Connection, messages: Vec<Message>) -> Vec<Message> {
        let (sender, inbox) = mpsc::channel();
        for message in messages {
            sender.send(message).unwrap();
        }
        drop(sender);
        server.inbox = inbox;
        server.main_loop().unwrap();
        client.receiver.try_iter().collect()
    }

    fn watched(changes: &[(&std::path::Path, u8)]) -> Message {
        let changes: Vec<serde_json::Value> = changes
            .iter()
            .map(|(path, typ)| json!({ "uri": Url::from_file_path(path).unwrap(), "type": typ }))
            .collect();
        Message::Notification(Notification::new(
            "workspace/didChangeWatchedFiles".into(),
            json!({ "changes": changes }),
        ))
    }

    /// The diagnostics published for `path`, if any were.
    fn published(
        messages: &[Message],
        path: &std::path::Path,
    ) -> Option<Vec<lsp_types::Diagnostic>> {
        let uri = Url::from_file_path(path).unwrap();
        messages.iter().rev().find_map(|m| match m {
            Message::Notification(n) if n.method == "textDocument/publishDiagnostics" => {
                let p: lsp_types::PublishDiagnosticsParams =
                    serde_json::from_value(n.params.clone()).unwrap();
                (p.uri == uri).then_some(p.diagnostics)
            }
            _ => None,
        })
    }

    fn warnings(messages: &[Message]) -> usize {
        messages
            .iter()
            .filter(|m| matches!(m, Message::Notification(n) if n.method == "window/showMessage"))
            .count()
    }

    /// An invalid config shows on the file, where the problem is; fixed,
    /// it no longer does; broken again, it is shown again.
    #[test]
    fn invalid_config_is_shown_on_the_file_while_it_lasts() {
        let dir = tempfile::tempdir().unwrap();
        let root = world::tests::real_dir(dir.path());
        let config = root.join("vfl.toml");
        let broken = "[format]\nindnet = 2\n";
        std::fs::write(&config, broken).unwrap();
        std::fs::write(root.join("a.vhd"), "entity a is\nend entity;\n").unwrap();

        let (server, client) = Connection::memory();
        let (_, inbox) = mpsc::channel();
        let mut server = Server::new(&server, inbox, World::new(&root));
        server.publish().unwrap();
        let messages: Vec<Message> = client.receiver.try_iter().collect();
        let diagnostics = published(&messages, &config).expect("diagnostics on vfl.toml");
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(
            diagnostics[0].range,
            lsp_types::Range::new(
                lsp_types::Position::new(1, 0),
                lsp_types::Position::new(1, 6)
            )
        );
        assert!(diagnostics[0].message.contains("indnet"));
        assert_eq!(warnings(&messages), 1);

        std::fs::write(&config, "[format]\nindent = 2\n").unwrap();
        let messages = step(&mut server, &client, vec![watched(&[(&config, 2)])]);
        assert_eq!(published(&messages, &config), Some(Vec::new()));
        assert_eq!(warnings(&messages), 0);

        std::fs::write(&config, broken).unwrap();
        let messages = step(&mut server, &client, vec![watched(&[(&config, 2)])]);
        assert_eq!(published(&messages, &config).map(|d| d.len()), Some(1));
        assert_eq!(warnings(&messages), 1, "shown again");
    }

    /// Vendor files are linted as they are opened and edited, apart from
    /// the project: they never make it analyze again, requests on them get
    /// nothing, and a port renamed in the design shows in them.
    #[test]
    fn vendor_files_are_linted_apart() {
        let dir = tempfile::tempdir().unwrap();
        let root = world::tests::real_dir(dir.path());
        let vhd = root.join("top.vhd");
        let entity =
            |port: &str| format!("entity top is\n  port ({port} : in bit);\nend entity;\n");
        std::fs::write(&vhd, entity("clk")).unwrap();
        let qsf = root.join("top.qsf");
        let (server, client) = Connection::memory();
        let (_, inbox) = mpsc::channel();
        let mut server = Server::new(&server, inbox, World::new(&root));
        let notification = |method: &str, params: serde_json::Value| {
            Message::Notification(Notification::new(method.into(), params))
        };
        let uri = Url::from_file_path(&qsf).unwrap();
        let text = "set_global_assignment -name TOP_LEVEL_ENTITY top\n\
                    set_location_assignment PIN_A1 -to clk\n";
        let open = notification(
            "textDocument/didOpen",
            serde_json::json!({"textDocument": {"uri": uri, "languageId": "quartus",
                "version": 1, "text": text}}),
        );
        let messages = step(&mut server, &client, vec![open]);
        assert_eq!(published(&messages, &qsf), None, "nothing to report");
        assert!(!server.world.dirty);
        let edit = notification(
            "textDocument/didChange",
            serde_json::json!({"textDocument": {"uri": uri, "version": 2},
                "contentChanges": [{"text": format!("{text}set_location_assignment PIN_A1 -to rst\n")}]}),
        );
        let hover = Message::Request(Request::new(
            7.into(),
            "textDocument/hover".into(),
            serde_json::json!({"textDocument": {"uri": uri}, "position": {"line": 0, "character": 1}}),
        ));
        let messages = step(&mut server, &client, vec![edit, hover]);
        let rules: Vec<String> = published(&messages, &qsf)
            .unwrap()
            .iter()
            .map(|d| match &d.code {
                Some(lsp_types::NumberOrString::String(code)) => code.clone(),
                _ => String::new(),
            })
            .collect();
        assert_eq!(rules, ["vendor/pin-conflict", "vendor/unknown-port"]);
        assert!(messages.iter().any(|m| matches!(m, Message::Response(r)
            if r.id == 7.into() && r.result == Some(serde_json::Value::Null))));
        assert!(!server.world.dirty, "the project is not analyzed again");

        // `clk` renamed `clock` in the design: the `.qsf` shows it.
        let vhd_uri = Url::from_file_path(&vhd).unwrap();
        let rename = notification(
            "textDocument/didOpen",
            serde_json::json!({"textDocument": {"uri": vhd_uri, "languageId": "vhdl",
                "version": 1, "text": entity("clock")}}),
        );
        let messages = step(&mut server, &client, vec![rename]);
        let unknown = published(&messages, &qsf)
            .unwrap()
            .iter()
            .filter(|d| d.message.contains("has no port"))
            .count();
        assert_eq!(unknown, 2);

        let close = notification(
            "textDocument/didClose",
            serde_json::json!({"textDocument": {"uri": uri}}),
        );
        let messages = step(&mut server, &client, vec![close]);
        assert_eq!(published(&messages, &qsf), Some(Vec::new()));
    }

    /// Files created by the hundred, in several notifications, are read
    /// once the burst is over, and analyzed once.
    #[test]
    fn a_burst_of_new_files_is_read_at_once() {
        let dir = tempfile::tempdir().unwrap();
        let root = world::tests::real_dir(dir.path());
        std::fs::create_dir(root.join("rtl")).unwrap();
        std::fs::write(root.join("rtl/a.vhd"), "entity a is\nend entity;\n").unwrap();
        let (server, client) = Connection::memory();
        let (_, inbox) = mpsc::channel();
        let mut server = Server::new(&server, inbox, World::new(&root));
        server.publish().unwrap();
        client.receiver.try_iter().for_each(drop);

        let mut events = Vec::new();
        for i in 0..100 {
            let dir = root.join(if i % 2 == 0 { "rtl" } else { "new" });
            std::fs::create_dir_all(&dir).unwrap();
            let path = dir.join(format!("e{i}.vhd"));
            std::fs::write(&path, format!("entity e{i} is\nend entity;\nfoo\n")).unwrap();
            events.push(watched(&[(&path, 1)]));
        }
        let messages = step(&mut server, &client, events);
        assert_eq!(server.world.project.files.len(), 101);
        // One analysis: each file's diagnostics are published once.
        let path = root.join("new/e1.vhd");
        let count = messages
            .iter()
            .filter(|m| matches!(m, Message::Notification(n) if n.method == "textDocument/publishDiagnostics"
                && n.params["uri"] == json!(Url::from_file_path(&path).unwrap())))
            .count();
        assert_eq!(count, 1);
    }

    /// The client of [`run`] on another thread, for its whole life.
    fn client_of_run(capabilities: serde_json::Value, root: &std::path::Path) -> Connection {
        let (server, client) = Connection::memory();
        std::thread::spawn(move || run(&server).unwrap());
        let initialize = Request::new(
            0.into(),
            "initialize".into(),
            json!({
                "capabilities": capabilities,
                "rootUri": Url::from_file_path(root).unwrap(),
            }),
        );
        client.sender.send(Message::Request(initialize)).unwrap();
        let initialized = Notification::new("initialized".into(), json!({}));
        client
            .sender
            .send(Message::Notification(initialized))
            .unwrap();
        client
    }

    /// Waits for a message that `wanted` picks.
    fn wait_for<T>(client: &Connection, mut wanted: impl FnMut(&Message) -> Option<T>) -> T {
        loop {
            let message = client
                .receiver
                .recv_timeout(std::time::Duration::from_secs(20))
                .expect("the server answers");
            if let Some(found) = wanted(&message) {
                return found;
            }
        }
    }

    /// Loading a project shows its progress, with file counts, to a
    /// client that can show it.
    #[test]
    fn loading_shows_progress() {
        let dir = tempfile::tempdir().unwrap();
        let root = world::tests::real_dir(dir.path());
        for i in 0..5 {
            std::fs::write(
                root.join(format!("e{i}.vhd")),
                format!("entity e{i} is\nend entity;\n"),
            )
            .unwrap();
        }
        let capabilities = json!({
            "window": { "workDoneProgress": true },
            "workspace": { "didChangeWatchedFiles": { "dynamicRegistration": true } },
        });
        let client = client_of_run(capabilities, &root);
        let mut created = false;
        let mut values = Vec::new();
        wait_for(&client, |m| {
            match m {
                Message::Request(r) if r.method == "window/workDoneProgress/create" => {
                    created = true;
                }
                Message::Notification(n) if n.method == "$/progress" => {
                    assert!(created, "the token is created first");
                    values.push(n.params["value"].clone());
                    return (n.params["value"]["kind"] == "end").then_some(());
                }
                _ => {}
            }
            None
        });
        assert_eq!(values[0]["kind"], "begin");
        let reports: Vec<&str> = values
            .iter()
            .filter(|v| v["kind"] == "report")
            .filter_map(|v| v["message"].as_str())
            .collect();
        assert!(reports.contains(&"Parsing 5/5 files"), "{reports:?}");
        assert!(reports.contains(&"Analyzing 5 files"), "{reports:?}");
        assert_eq!(values.last().unwrap()["message"], "5 files");
        client
            .sender
            .send(Message::Request(Request::new(
                1.into(),
                "shutdown".into(),
                json!(null),
            )))
            .unwrap();
        let exit = Notification::new("exit".into(), json!(null));
        client.sender.send(Message::Notification(exit)).unwrap();
    }

    /// Without the client's file watchers, the server notices new files
    /// and config changes by itself.
    #[test]
    fn without_client_watchers_the_server_watches() {
        let dir = tempfile::tempdir().unwrap();
        let root = world::tests::real_dir(dir.path());
        std::fs::write(root.join("a.vhd"), "entity a is\nend entity;\nfoo\n").unwrap();
        let client = client_of_run(json!({}), &root);
        // The first diagnostics: the project is loaded, and watched.
        wait_for(&client, |m| {
            matches!(m, Message::Notification(n) if n.method == "textDocument/publishDiagnostics")
                .then_some(())
        });
        let new = root.join("b.vhd");
        std::fs::write(&new, "entity b is\nend entity;\nfoo\n").unwrap();
        let uri = json!(Url::from_file_path(&new).unwrap());
        wait_for(&client, |m| {
            matches!(m, Message::Notification(n) if n.method == "textDocument/publishDiagnostics"
                && n.params["uri"] == uri)
            .then_some(())
        });
        let config = root.join("vfl.toml");
        std::fs::write(&config, "[lint\n").unwrap();
        let uri = json!(Url::from_file_path(&config).unwrap());
        wait_for(&client, |m| {
            matches!(m, Message::Notification(n) if n.method == "textDocument/publishDiagnostics"
                && n.params["uri"] == uri)
            .then_some(())
        });
        // The client leaves without a word: the server stops all the same.
        drop(client);
    }

    /// A handler that panics fails its request; the server goes on.
    #[test]
    fn a_panic_fails_one_request() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.vhd");
        let text = "entity a is\nend entity;\narchitecture rtl of a is\n  signal s : bit;\nbegin\nend architecture;\n";
        std::fs::write(&path, text).unwrap();
        let uri = Url::from_file_path(&path).unwrap();
        let at =
            json!({ "textDocument": { "uri": uri }, "position": { "line": 3, "character": 9 } });

        let (server, client) = Connection::memory();
        let (sender, inbox) = mpsc::channel();
        let send = |message| sender.send(message).unwrap();
        send(Message::Notification(Notification::new(
            "vfl/test/panic".into(),
            json!({}),
        )));
        send(Message::Request(Request::new(
            1.into(),
            "vfl/test/panic".into(),
            json!({}),
        )));
        // An edit after the panics is still analyzed and published.
        send(Message::Notification(Notification::new(
            "textDocument/didOpen".into(),
            json!({ "textDocument": { "uri": uri, "languageId": "vhdl", "version": 0,
                "text": text.replace("bit;", "bit; signal t : bit;") } }),
        )));
        send(Message::Request(Request::new(
            2.into(),
            "textDocument/hover".into(),
            at,
        )));
        drop(sender);

        let mut server = Server::new(&server, inbox, World::new(dir.path()));
        server.main_loop().unwrap();

        let messages: Vec<Message> = client.receiver.try_iter().collect();
        let responses: Vec<&Response> = messages
            .iter()
            .filter_map(|m| match m {
                Message::Response(r) => Some(r),
                _ => None,
            })
            .collect();
        assert_eq!(responses.len(), 2, "{messages:?}");
        let error = responses[0].error.as_ref().unwrap();
        assert_eq!(error.code, ErrorCode::InternalError as i32);
        assert!(error.message.contains("a test panic"), "{}", error.message);
        assert!(responses[1].result.as_ref().is_some_and(|r| !r.is_null()));
        let unused_t = messages.iter().any(|m| {
            matches!(m, Message::Notification(n)
                if n.method == "textDocument/publishDiagnostics"
                    && n.params.to_string().contains("`t`"))
        });
        assert!(unused_t, "{messages:?}");
    }

    /// Several changes in one notification, each against the text the
    /// previous ones left, give the same text as one at a time.
    #[test]
    fn batched_changes_apply_in_order() {
        let mut text = "entity a is\r\nend entity;\nℵ -- é\n\narchitecture\n".to_owned();
        // A small deterministic generator of positions and texts.
        let mut seed = 7u32;
        let mut next = |n: u32| {
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            (seed >> 16) % n.max(1)
        };
        let inserts = ["", "x", "\n", "a\nb", "\r\n", "--\n\nℵ", "é"];
        for _ in 0..200 {
            let changes: Vec<lsp_types::TextDocumentContentChangeEvent> = (0..1 + next(5))
                .map(|_| {
                    let at = |next: &mut dyn FnMut(u32) -> u32| {
                        lsp_types::Position::new(next(8), next(14))
                    };
                    let (a, b) = (at(&mut next), at(&mut next));
                    let (start, end) = if (a.line, a.character) <= (b.line, b.character) {
                        (a, b)
                    } else {
                        (b, a)
                    };
                    lsp_types::TextDocumentContentChangeEvent {
                        range: Some(lsp_types::Range::new(start, end)),
                        range_length: None,
                        text: inserts[next(inserts.len() as u32) as usize].into(),
                    }
                })
                .collect();
            let mut one_by_one = text.clone();
            for change in changes.clone() {
                apply_changes(&mut one_by_one, vec![change]);
            }
            apply_changes(&mut text, changes);
            assert_eq!(text, one_by_one);
        }
    }

    /// A position past the end of a CRLF line is before its `\r`: an edit
    /// there does not split the line break.
    #[test]
    fn edits_past_the_end_of_a_crlf_line() {
        let change = |line, character, text: &str| lsp_types::TextDocumentContentChangeEvent {
            range: Some(lsp_types::Range::new(
                lsp_types::Position::new(line, character),
                lsp_types::Position::new(line, character),
            )),
            range_length: None,
            text: text.into(),
        };
        let mut text = "ab\r\ncd\r\nef".to_owned();
        apply_changes(&mut text, vec![change(0, 2, "X"), change(1, 99, "Y")]);
        assert_eq!(text, "abX\r\ncdY\r\nef");
        // The last line, without a line break, and a line past the text.
        apply_changes(&mut text, vec![change(2, 99, "Z"), change(9, 0, "!")]);
        assert_eq!(text, "abX\r\ncdY\r\nefZ!");
        // A lone `\r` ending the text is not a line break: it stays.
        let mut text = "a\r".to_owned();
        apply_changes(&mut text, vec![change(0, 99, "b")]);
        assert_eq!(text, "a\rb");
    }
}

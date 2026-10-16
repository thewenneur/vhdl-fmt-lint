//! The server's view of the project: files on disk overlaid with open
//! editor buffers, their analysis and lint results.
//!
//! An edit re-parses only the edited file. Library inference runs again
//! only when the set of files, what a file declares or the library names
//! it refers to, or a library list (`vhdl_ls.toml`, tool files) changed.
//! The analysis then reuses the results of every design unit the edit
//! cannot have affected.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rustc_hash::{FxHashMap, FxHashSet};
use vfl_config::Config;
use vfl_lint::FileSettings;
use vfl_project::{
    Encoding, FileSummary, LibrarySources, Project, ProjectRoot, RootKind, SourceFile, VerilogFile,
    collect_project, collect_sources, decode, find_root, verilog_standard,
};
use vfl_sema::{Analysis, FileId};
use vfl_syntax::{LineIndex, TextSize};

pub struct World {
    pub root: ProjectRoot,
    pub project: Project,
    /// Open documents, by path.
    pub overlays: FxHashMap<PathBuf, String>,
    /// Open documents that are not files of the project.
    outside: FxHashSet<PathBuf>,
    pub analysis: Analysis,
    pub lint: Vec<vfl_lint::Diagnostic>,
    pub config_errors: Vec<String>,
    /// What to tell the user once, such as that Verilog support is in
    /// progress.
    pub notices: Vec<String>,
    /// Why the project could not be loaded as configured.
    load_error: Option<String>,
    /// The config files left out because they are invalid, with where
    /// in each the problem is: shown on the file itself.
    pub config_file_errors: Vec<vfl_config::FileError>,
    configs: FxHashMap<PathBuf, Config>,
    /// The files over `max-file-size`, by path, as they are analyzed.
    outlines: FxHashMap<PathBuf, Outline>,
    lint_cache: vfl_lint::LintCache,
    /// The library lists the libraries were last inferred from.
    sources: Option<LibrarySources>,
    /// The path and summary of each file (VHDL, then Verilog) when
    /// libraries were last inferred.
    inferred_from: Vec<(PathBuf, FileSummary)>,
    /// Files whose text changed since the last analysis.
    pub dirty: bool,
    /// The semantic tokens last sent, for answering with deltas.
    pub(crate) semantic: crate::semantic::Sent,
    /// Whether every file has the project's config.
    fixed_config: bool,
    /// The config of files no config file applies to: the editor's
    /// settings.
    defaults: Config,
}

/// The project root of `path`, in the form the editor's file URIs give
/// back, which every path the server compares with it has: on Windows,
/// `C:\dir` rather than the `\\?\C:\dir` of a canonical path.
pub fn project_root(path: &Path) -> ProjectRoot {
    let path = lsp_types::Url::from_file_path(path)
        .ok()
        .and_then(|uri| uri.to_file_path().ok())
        .unwrap_or_else(|| path.to_owned());
    find_root(&path)
}

impl World {
    pub fn new(root: &Path) -> World {
        World::with_defaults(root, Config::default())
    }

    /// Like [`World::new`], with `defaults` as the config of the files no
    /// config file applies to: the settings of the editor.
    pub fn with_defaults(root: &Path, defaults: Config) -> World {
        World::with_progress(root, defaults, &|_| {})
    }

    /// Like [`World::with_defaults`], telling `progress` how loading the
    /// project goes, from the threads that do it.
    pub fn with_progress(
        root: &Path,
        defaults: Config,
        progress: &(dyn Fn(Progress) + Sync),
    ) -> World {
        let root = project_root(root);
        let mut config_file_errors = Vec::new();
        let (project, load_error) = load(&root, &defaults, &mut config_file_errors, progress);
        progress(Progress::Analyzing {
            files: project.files.len() + project.verilog.len(),
        });
        let mut world = World {
            load_error,
            config_file_errors,
            root,
            analysis: Analysis::empty(),
            project,
            overlays: FxHashMap::default(),
            outside: FxHashSet::default(),
            lint: Vec::new(),
            config_errors: Vec::new(),
            notices: Vec::new(),
            configs: FxHashMap::default(),
            outlines: FxHashMap::default(),
            lint_cache: vfl_lint::LintCache::default(),
            sources: None,
            inferred_from: Vec::new(),
            dirty: true,
            semantic: Default::default(),
            fixed_config: false,
            defaults,
        };
        world.refresh();
        world
    }

    /// A world with no project, for documents that are not saved
    /// anywhere: only open documents are analyzed, with default settings.
    pub fn detached() -> World {
        World::detached_with(Config::default(), false)
    }

    /// A world with no project, whose open documents are analyzed with
    /// `config`, or, unless `everywhere`, the config of their directory
    /// when they are saved somewhere.
    pub fn detached_with(config: Config, everywhere: bool) -> World {
        let root = ProjectRoot {
            dir: PathBuf::new(),
            kind: RootKind::Fallback,
        };
        World {
            fixed_config: everywhere,
            defaults: config.clone(),
            project: empty(&root, config),
            load_error: None,
            config_file_errors: Vec::new(),
            root,
            analysis: Analysis::empty(),
            overlays: FxHashMap::default(),
            outside: FxHashSet::default(),
            lint: Vec::new(),
            config_errors: Vec::new(),
            notices: Vec::new(),
            configs: FxHashMap::default(),
            outlines: FxHashMap::default(),
            lint_cache: vfl_lint::LintCache::default(),
            sources: None,
            inferred_from: Vec::new(),
            dirty: true,
            semantic: Default::default(),
        }
    }

    /// Reloads every file from disk, keeping open documents.
    pub fn reload(&mut self) {
        self.configs.clear();
        self.config_file_errors.clear();
        self.root = find_root(&self.root.dir);
        (self.project, self.load_error) = load(
            &self.root,
            &self.defaults,
            &mut self.config_file_errors,
            &|_| {},
        );
        self.outside.clear();
        self.sources = None;
        self.inferred_from.clear();
        let overlays: Vec<(PathBuf, String)> = self.overlays.drain().collect();
        for (path, text) in overlays {
            self.set_text(path, text);
        }
        self.dirty = true;
    }

    /// The config for a file. A relative path names a document that is not
    /// saved anywhere, which gets the defaults. An invalid config file is
    /// left out, as if it were not there, and reported.
    pub fn config(&mut self, path: &Path) -> &Config {
        let dir = config_dir(path);
        let project = &self.project.config;
        let fixed = self.fixed_config;
        let defaults = &self.defaults;
        let errors = &mut self.config_file_errors;
        self.configs.entry(dir.clone()).or_insert_with(|| {
            if fixed {
                project.clone()
            } else if dir.as_os_str().is_empty() {
                defaults.clone()
            } else {
                let (config, found) = vfl_config::load_for_dir_checked(&dir, defaults);
                add_errors(errors, found);
                config
            }
        })
    }

    /// The config of the files no config file applies to.
    pub fn defaults(&self) -> &Config {
        &self.defaults
    }

    /// Changes the config of the files no config file applies to, and
    /// reloads the project when that changes anything.
    pub fn set_defaults(&mut self, defaults: Config) {
        if defaults != self.defaults {
            self.defaults = defaults;
            // A world with no project only has the open documents.
            if self.fixed_config || self.root.dir.as_os_str().is_empty() {
                self.project.config = self.defaults.clone();
                self.configs.clear();
                self.dirty = true;
            } else {
                self.reload();
            }
        }
    }

    /// Sets the text of an open document. Opening a document that matches
    /// the file on disk changes nothing, so it does not analyze again.
    pub fn set_text(&mut self, path: PathBuf, text: String) {
        if verilog_standard(self.config(&path), &path).is_some() {
            self.set_verilog_text(path, text);
            return;
        }
        if self
            .project
            .file_index(&path)
            .is_some_and(|i| *self.project.files[i].text == *text)
        {
            self.overlays.insert(path, text);
            return;
        }
        let standard = self.config(&path).standard();
        let file = SourceFile::new(path.clone(), text.clone(), Encoding::Utf8, standard);
        match self.project.file_index(&path) {
            Some(i) => self.project.files[i] = file,
            None => {
                self.project.files.push(file);
                self.outside.insert(path.clone());
            }
        }
        self.overlays.insert(path, text);
        self.dirty = true;
    }

    /// Sets the text of an open Verilog document, whose modules VHDL may
    /// instantiate.
    fn set_verilog_text(&mut self, path: PathBuf, text: String) {
        let index = self.project.verilog_index(&path);
        if index.is_some_and(|i| *self.project.verilog[i].text == *text) {
            self.overlays.insert(path, text);
            return;
        }
        let file = self.verilog_file(path.clone(), text.clone(), Encoding::Utf8);
        let Some(file) = file else { return };
        match index {
            Some(i) => self.project.verilog[i] = file,
            None => {
                self.project.verilog.push(file);
                self.outside.insert(path.clone());
            }
        }
        self.overlays.insert(path, text);
        self.dirty = true;
    }

    /// Parses a Verilog file with the settings that apply to it.
    fn verilog_file(
        &mut self,
        path: PathBuf,
        text: String,
        encoding: Encoding,
    ) -> Option<VerilogFile> {
        let root = self.root.dir.clone();
        let config = self.config(&path);
        if !config.verilog.loaded() {
            return None;
        }
        let standard = verilog_standard(config, &path)?;
        Some(VerilogFile::new(
            path, text, encoding, config, &root, standard,
        ))
    }

    /// Re-reads a Verilog file from disk (created, changed or deleted).
    /// `members` are the new files the project's patterns take.
    fn reread_verilog(&mut self, path: &Path, members: &FxHashSet<PathBuf>) {
        let index = self.project.verilog_index(path);
        let known = index.is_some() && !self.outside.remove(path);
        let read = std::fs::read(path).ok().map(decode);
        if let (true, Some(i), Some((text, _))) = (known, index, &read)
            && *self.project.verilog[i].text == **text
        {
            return;
        }
        let in_project = read.is_some() && (known || members.contains(path));
        let file = match read.filter(|_| in_project) {
            Some((text, encoding)) => self.verilog_file(path.to_owned(), text, encoding),
            None => None,
        };
        match (index, file) {
            (Some(i), Some(file)) => self.project.verilog[i] = file,
            (None, Some(file)) => self.project.verilog.push(file),
            (Some(i), None) => {
                self.project.verilog.remove(i);
            }
            (None, None) => return,
        }
        self.dirty = true;
    }

    /// A document was closed: back to the file on disk, if any.
    pub fn close(&mut self, path: &Path) {
        self.overlays.remove(path);
        self.semantic.forget(path);
        self.reread(path);
    }

    /// Re-reads a file from disk (created, changed or deleted). A file
    /// whose text did not change is left as it is.
    pub fn reread(&mut self, path: &Path) {
        self.reread_all(&[path.to_owned()]);
    }

    /// Re-reads files from disk (created, changed or deleted), such as
    /// those a `git checkout` changed. Whether new files belong to the
    /// project takes one walk of it at most, for all of them.
    pub fn reread_all(&mut self, paths: &[PathBuf]) {
        let members = self.members(paths);
        for path in paths {
            self.reread_one(path, &members);
        }
    }

    /// Those of `paths` that the project does not have yet and its file
    /// patterns take. Where the project already has files in the
    /// directory of each, the directories are listed alone; otherwise
    /// the project is walked once.
    fn members(&self, paths: &[PathBuf]) -> FxHashSet<PathBuf> {
        let project = &self.project;
        let loaded = |p: &Path| {
            (project.file_index(p).is_some() || project.verilog_index(p).is_some())
                && !self.outside.contains(p)
        };
        let new: Vec<&PathBuf> = paths
            .iter()
            .filter(|p| {
                !self.overlays.contains_key(*p)
                    && p.starts_with(&self.root.dir)
                    && !loaded(p)
                    && p.is_file()
            })
            .collect();
        if new.is_empty() {
            return FxHashSet::default();
        }
        // The directories the project's walk went into.
        let walked: FxHashSet<&Path> = project
            .files
            .iter()
            .map(|f| &f.path)
            .chain(project.verilog.iter().map(|f| &f.path))
            .filter(|p| !self.outside.contains(*p))
            .filter_map(|p| p.parent())
            .collect();
        let dirs: FxHashSet<&Path> = new.iter().filter_map(|p| p.parent()).collect();
        let root = &self.root.dir;
        let found: Vec<PathBuf> = if dirs.iter().all(|d| walked.contains(d)) {
            dirs.iter()
                .flat_map(|dir| {
                    vfl_project::collect_project_in(root, dir, &project.config).unwrap_or_default()
                })
                .collect()
        } else {
            collect_project(root, &project.config)
                .map(|(paths, _)| paths)
                .unwrap_or_default()
        };
        let new: FxHashSet<&PathBuf> = new.into_iter().collect();
        found.into_iter().filter(|p| new.contains(p)).collect()
    }

    fn reread_one(&mut self, path: &Path, members: &FxHashSet<PathBuf>) {
        if self.overlays.contains_key(path) {
            return;
        }
        if verilog_standard(self.config(path), path).is_some() {
            self.reread_verilog(path, members);
            return;
        }
        let index = self.project.file_index(path);
        // A file loaded with the project, rather than a document opened
        // from elsewhere.
        let known = index.is_some() && !self.outside.remove(path);
        let read = std::fs::read(path).ok().map(decode);
        if let (true, Some(i), Some((text, _))) = (known, index, &read)
            && *self.project.files[i].text == **text
        {
            return;
        }
        // Whether the file belongs to the project: a file loaded with it
        // does; another one, if the project's file patterns take it.
        let in_project = read.is_some() && (known || members.contains(path));
        let standard = self.config(path).standard();
        let file = read
            .filter(|_| in_project)
            .map(|(text, encoding)| SourceFile::new(path.to_owned(), text, encoding, standard));
        match (index, file) {
            (Some(i), Some(file)) => self.project.files[i] = file,
            (None, Some(file)) => self.project.files.push(file),
            (Some(i), None) => {
                self.project.files.remove(i);
            }
            (None, None) => return,
        }
        self.dirty = true;
    }

    /// Re-analyzes and re-lints the project if anything changed.
    pub fn refresh(&mut self) {
        if !self.dirty {
            return;
        }
        self.dirty = false;
        self.infer();
        let paths: Vec<PathBuf> = self
            .project
            .files
            .iter()
            .map(|f| f.path.clone())
            .chain(self.project.verilog.iter().map(|f| f.path.clone()))
            .collect();
        for path in &paths {
            self.config(path);
        }
        let inputs = self.analysis_inputs();
        let previous = std::mem::replace(&mut self.analysis, Analysis::empty());
        self.analysis =
            previous.into_update_with_foreign(inputs, vfl_lint::foreign_inputs(&self.project));
        let configs: Vec<&Config> = paths
            .iter()
            .map(|p| &self.configs[&config_dir(p)])
            .collect();
        // Verilog files follow the VHDL ones in the analysis, linted only
        // with `[verilog]` `mode = "full"`. Files over the size limit are
        // not linted.
        let vhdl = self.project.files.len();
        let outlines = &self.outlines;
        let settings: Vec<FileSettings> = self
            .project
            .files
            .iter()
            .map(|file| (file.parse.errors(), None))
            .chain(
                self.project
                    .verilog
                    .iter()
                    .map(|file| (file.parse.errors(), Some(&file.parse))),
            )
            .zip(&configs)
            .enumerate()
            .filter(|(i, (_, config))| {
                if *i < vhdl {
                    !outlines.contains_key(&paths[*i])
                } else {
                    config.verilog.full()
                }
            })
            .map(|(i, ((syntax_errors, verilog), config))| FileSettings {
                file: i as FileId,
                syntax_errors,
                config,
                verilog,
                root: Some(&self.root.dir),
            })
            .collect();
        self.notices.clear();
        if settings.iter().any(|s| s.verilog.is_some()) {
            self.notices
                .push(vfl_config::VerilogConfig::IN_PROGRESS.to_owned());
        }
        let result = vfl_lint::lint_cached(&self.analysis, &settings, &mut self.lint_cache);
        self.lint = result.diagnostics;
        for (i, path) in paths[..vhdl].iter().enumerate() {
            if outlines.contains_key(path) {
                let config = configs[i];
                let size = self.project.files[i].text.len();
                self.lint.push(too_large(i as FileId, size, config));
            }
        }
        self.config_errors = result.config_errors;
        let files = self.config_file_errors.iter().map(ToString::to_string);
        self.config_errors
            .splice(0..0, self.load_error.iter().cloned().chain(files));
    }

    /// The VHDL files as the analysis takes them: those over their
    /// `max-file-size` as outlines, which an edit elsewhere does not
    /// build again. Every file's config must be loaded.
    fn analysis_inputs(&mut self) -> Vec<vfl_sema::FileInput> {
        let mut inputs = vfl_lint::analysis_inputs(&self.project);
        let mut previous = std::mem::take(&mut self.outlines);
        for (input, file) in inputs.iter_mut().zip(&self.project.files) {
            let config = &self.configs[&config_dir(&file.path)];
            if !config.project.max_file_size.exceeded_by(file.text.len()) {
                continue;
            }
            let outline = match previous.remove(&file.path) {
                Some(outline) if Arc::ptr_eq(&outline.source, &file.text) => outline,
                _ => Outline::new(file, config.standard()),
            };
            input.text = outline.text.clone();
            input.green = outline.green.clone();
            self.outlines.insert(file.path.clone(), outline);
        }
        inputs
    }

    /// Infers libraries again if what they depend on changed.
    fn infer(&mut self) {
        if self.sources.as_ref().is_none_or(LibrarySources::is_stale) {
            self.inferred_from.clear();
            match self.project.library_sources() {
                Ok(sources) => self.sources = Some(sources),
                Err(_) => {
                    // An invalid library pattern: analyze with what
                    // inference had.
                    self.sources = None;
                    self.project
                        .libraries
                        .resize(self.project.files.len(), Vec::new());
                    self.project
                        .verilog_libraries
                        .resize(self.project.verilog.len(), Vec::new());
                    return;
                }
            }
        }
        let project = &self.project;
        let files = project
            .files
            .iter()
            .map(|f| (&f.path, &f.summary))
            .chain(project.verilog.iter().map(|f| (&f.path, &f.summary)));
        let unchanged = self.inferred_from.len() == project.files.len() + project.verilog.len()
            && project.libraries.len() == project.files.len()
            && project.verilog_libraries.len() == project.verilog.len()
            && files
                .zip(&self.inferred_from)
                .all(|((p, s), (path, summary))| p == path && s == summary);
        if unchanged {
            return;
        }
        let Some(sources) = &self.sources else {
            return;
        };
        self.project.infer_with(sources);
        let project = &self.project;
        self.inferred_from = project
            .files
            .iter()
            .map(|f| (f.path.clone(), f.summary.clone()))
            .chain(
                project
                    .verilog
                    .iter()
                    .map(|f| (f.path.clone(), f.summary.clone())),
            )
            .collect();
    }

    /// The analyzed VHDL file at `path`. A Verilog file is analyzed only
    /// for its modules, with no VHDL tree to answer requests from.
    pub fn file_id(&self, path: &Path) -> Option<FileId> {
        let file = self.analysis.file_id(path)?;
        self.analysis
            .foreign_modules(file)
            .is_none()
            .then_some(file)
    }

    /// The analyzed file at `path`, VHDL or Verilog.
    pub fn any_file_id(&self, path: &Path) -> Option<FileId> {
        self.analysis.file_id(path)
    }

    /// Whether the Verilog file at `path` gets editor features: with
    /// `[verilog]` `mode = "full"` only.
    pub fn verilog_features(&self, path: &Path) -> bool {
        self.configs
            .get(&config_dir(path))
            .unwrap_or(&self.project.config)
            .verilog
            .full()
    }

    /// Whether an analyzed file is a VHDL one.
    pub fn is_vhdl(&self, file: FileId) -> bool {
        self.analysis.foreign_modules(file).is_none()
    }

    /// The text and line index of an analyzed file.
    pub fn doc(&self, file: FileId) -> Doc<'_> {
        let f = &self.analysis.files()[file as usize];
        Doc {
            text: &f.text,
            index: &f.line_index,
        }
    }
}

/// The code of the information diagnostic on a file over `max-file-size`.
pub const TOO_LARGE: &str = "vfl/max-file-size";

/// What a file over `max-file-size`, a generated netlist say, is analyzed
/// as: its text with the design units that hold an architecture or a
/// package body blanked out, so that its entities and packages are still
/// there for other files, at the same places, without the cost of
/// analyzing (and linting) the rest.
struct Outline {
    /// The text of the file the outline is of.
    source: Arc<str>,
    text: Arc<str>,
    green: vfl_syntax::GreenNode,
}

impl Outline {
    fn new(file: &SourceFile, standard: vfl_syntax::Standard) -> Outline {
        use vfl_syntax::SyntaxKind;
        let mut bytes = file.text.as_bytes().to_vec();
        for unit in file.parse.syntax().children() {
            let body = unit.kind() == SyntaxKind::DESIGN_UNIT
                && unit.children().any(|c| {
                    matches!(
                        c.kind(),
                        SyntaxKind::ARCHITECTURE_BODY | SyntaxKind::PACKAGE_BODY
                    )
                });
            if body {
                // Spaces keep every offset, and line breaks every line.
                let range = std::ops::Range::<usize>::from(unit.text_range());
                for b in &mut bytes[range] {
                    if *b != b'\n' && *b != b'\r' {
                        *b = b' ';
                    }
                }
            }
        }
        let text = String::from_utf8(bytes).unwrap_or_default();
        let green = vfl_syntax::parse(&text, standard).green().clone();
        Outline {
            source: file.text.clone(),
            text: text.into(),
            green,
        }
    }
}

/// The information diagnostic saying that a file is over its
/// `max-file-size`.
fn too_large(file: FileId, size: usize, config: &Config) -> vfl_lint::Diagnostic {
    vfl_lint::Diagnostic {
        rule: TOO_LARGE,
        level: vfl_config::Level::Info,
        file,
        range: vfl_syntax::TextRange::empty(TextSize::from(0)),
        message: format!(
            "This file ({:.1} MB) is over `max-file-size` ({}): only its entities and \
             packages are analyzed, and it is not linted. Raise `[project] max-file-size` \
             in vfl.toml to analyze it.",
            size as f64 / (1024.0 * 1024.0),
            config.project.max_file_size,
        ),
        fix: None,
        rename: None,
    }
}

/// The directory whose config applies to `path`; empty for a document
/// that is not saved anywhere.
fn config_dir(path: &Path) -> PathBuf {
    match path.parent() {
        Some(dir) if path.is_absolute() => dir.to_owned(),
        _ => PathBuf::new(),
    }
}

/// How loading a project goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Progress {
    /// `done` of the project's `total` files are read and parsed.
    Parsing { done: usize, total: usize },
    /// The project's files are read; their analysis starts.
    Analyzing { files: usize },
}

/// Loads the project. When its config is invalid, it is loaded with the
/// valid config files that apply (the editor's settings if none), and
/// `errors` says what was left out. When it fails otherwise, its sources
/// are still loaded, and the error is returned to be reported.
fn load(
    root: &ProjectRoot,
    defaults: &Config,
    errors: &mut Vec<vfl_config::FileError>,
    progress: &(dyn Fn(Progress) + Sync),
) -> (Project, Option<String>) {
    let (config, found) = vfl_config::load_for_dir_checked(&root.dir, defaults);
    add_errors(errors, found);
    let report = |done, total| progress(Progress::Parsing { done, total });
    match Project::load_with(root.clone(), config.clone(), &report) {
        Ok(project) => (project, None),
        Err(error) => {
            let project = collect_sources(&root.dir, &config.project)
                .ok()
                .and_then(|paths| Project::from_paths(root.clone(), config.clone(), paths).ok())
                .unwrap_or_else(|| empty(root, config));
            (project, Some(error.to_string()))
        }
    }
}

/// Adds the errors of `found` that `errors` does not have yet.
fn add_errors(errors: &mut Vec<vfl_config::FileError>, found: Vec<vfl_config::FileError>) {
    for error in found {
        if !errors.contains(&error) {
            errors.push(error);
        }
    }
}

fn empty(root: &ProjectRoot, config: Config) -> Project {
    Project {
        root: root.clone(),
        config,
        files: Vec::new(),
        libraries: Vec::new(),
        verilog: Vec::new(),
        verilog_libraries: Vec::new(),
        unreadable: Vec::new(),
        notes: Vec::new(),
        tool_files: Vec::new(),
    }
}

/// A text with its line index, for converting LSP positions (UTF-16).
#[derive(Clone, Copy)]
pub struct Doc<'a> {
    pub text: &'a str,
    pub index: &'a LineIndex,
}

impl Doc<'_> {
    pub fn position(&self, offset: TextSize) -> lsp_types::Position {
        let line = self.index.line(offset);
        let start = usize::from(self.index.line_start(line).unwrap_or_default());
        let character = self.text[start..usize::from(offset)].encode_utf16().count();
        lsp_types::Position::new(line, character as u32)
    }

    pub fn range(&self, range: vfl_syntax::TextRange) -> lsp_types::Range {
        lsp_types::Range::new(self.position(range.start()), self.position(range.end()))
    }

    pub fn offset(&self, position: lsp_types::Position) -> TextSize {
        offset(self.text, self.index, position)
    }
}

/// The offset of a UTF-16 `position` in `text`, clamped to the line, its
/// line break (`\r\n` or `\n`) excluded, and to the text.
pub fn offset(text: &str, index: &LineIndex, position: lsp_types::Position) -> TextSize {
    let Some(start) = index.line_start(position.line) else {
        return TextSize::of(text);
    };
    let start = usize::from(start);
    let line_end = match text[start..].find('\n') {
        Some(i) if text[start..start + i].ends_with('\r') => start + i - 1,
        Some(i) => start + i,
        None => text.len(),
    };
    let mut units = 0u32;
    for (i, c) in text[start..line_end].char_indices() {
        if units >= position.character {
            return TextSize::from((start + i) as u32);
        }
        units += c.len_utf16() as u32;
    }
    TextSize::from(line_end as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_config_keeps_sources_and_reports() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("vfl.toml"), "[format]\nindent = \"x\"\n").unwrap();
        std::fs::write(dir.path().join("a.vhd"), "entity a is\nend entity;\n").unwrap();
        let world = World::new(dir.path());
        assert_eq!(world.project.files.len(), 1);
        assert_eq!(world.config_errors.len(), 1, "{:?}", world.config_errors);
    }

    #[test]
    fn an_invalid_nested_config_falls_back_to_the_parent_one() {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        std::fs::write(root.join("vfl.toml"), "[format]\nindent = 3\n").unwrap();
        std::fs::create_dir(root.join("sub")).unwrap();
        std::fs::write(root.join("sub/vfl.toml"), "[format\nindent = 5\n").unwrap();
        std::fs::write(root.join("sub/b.vhd"), "entity b is\nend entity;\n").unwrap();
        let mut world = World::new(&root);
        assert_eq!(world.config(&root.join("sub/b.vhd")).format.indent, 3);
        assert_eq!(world.config_file_errors.len(), 1);
        assert_eq!(world.config_file_errors[0].path, root.join("sub/vfl.toml"));
        assert!(
            world.config_errors[0].contains("sub"),
            "{:?}",
            world.config_errors
        );

        // Fixed: the errors go once the configs are read again.
        std::fs::write(root.join("sub/vfl.toml"), "[format]\nindent = 5\n").unwrap();
        world.reload();
        world.refresh();
        assert!(world.config_file_errors.is_empty());
        assert!(world.config_errors.is_empty(), "{:?}", world.config_errors);
        assert_eq!(world.config(&root.join("sub/b.vhd")).format.indent, 5);
    }

    #[test]
    fn new_files_in_known_directories_join_without_a_walk() {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        std::fs::create_dir(root.join("rtl")).unwrap();
        std::fs::write(root.join("rtl/a.vhd"), TOP).unwrap();
        std::fs::write(root.join(".gitignore"), "*_netlist.vhd\n").unwrap();
        std::fs::write(
            root.join("vfl.toml"),
            "[project]\nexclude = [\"rtl/skip_*\"]\n",
        )
        .unwrap();
        let mut world = World::new(&root);
        let new = [
            root.join("rtl/b.vhd"),
            root.join("rtl/b_netlist.vhd"),
            root.join("rtl/skip_c.vhd"),
            root.join("rtl/sub/d.vhd"),
        ];
        for path in &new {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, PKG).unwrap();
        }
        // Known directories only: listed alone.
        assert_eq!(
            world.members(&new[..3]),
            FxHashSet::from_iter([root.join("rtl/b.vhd")])
        );
        // A new directory: one walk.
        assert_eq!(
            world.members(&new),
            FxHashSet::from_iter([root.join("rtl/b.vhd"), root.join("rtl/sub/d.vhd")])
        );
        world.reread_all(&new);
        assert_eq!(world.project.files.len(), 3);
    }

    #[test]
    fn files_over_the_size_limit_are_outlined() {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        std::fs::write(root.join("vfl.toml"), "[project]\nmax-file-size = 400\n").unwrap();
        let mut netlist = String::from(
            "library ieee;\nuse ieee.std_logic_1164.all;\n\
             entity net is\n  port (a : in std_logic; y : out std_logic);\nend entity;\n\
             architecture structure of net is\n",
        );
        for i in 0..20 {
            netlist.push_str(&format!("  signal \\n{i}~q\\ : std_logic;\n"));
        }
        netlist.push_str("begin\n  y <= undeclared;\nend architecture;\n");
        assert!(netlist.len() > 400);
        std::fs::write(root.join("net.vhd"), &netlist).unwrap();
        std::fs::write(
            root.join("top.vhd"),
            "library ieee;\nuse ieee.std_logic_1164.all;\nentity top is\nend entity;\n\
             architecture rtl of top is\n  signal a, y : std_logic;\nbegin\n\
             u : entity work.net port map (a => a, y => y);\nend architecture;\n",
        )
        .unwrap();
        let world = World::new(&root);
        let net = world.file_id(&root.join("net.vhd")).unwrap();
        // Only the information that says so, not the undeclared name.
        let found: Vec<&str> = world
            .lint
            .iter()
            .filter(|d| d.file == net)
            .map(|d| d.rule)
            .collect();
        assert_eq!(found, [TOO_LARGE]);
        // The entity is there for the instantiation, at its place.
        let top = world.file_id(&root.join("top.vhd")).unwrap();
        assert!(
            world
                .lint
                .iter()
                .all(|d| d.file != top || d.level != vfl_config::Level::Error),
            "{:?}",
            world.lint
        );
        let doc = world.doc(net);
        assert_eq!(doc.text.len(), netlist.len());
        assert!(doc.text.contains("entity net is"));
        assert!(!doc.text.contains("undeclared"));
        // The project keeps the text, for formatting.
        assert_eq!(
            &*world.project.files[world.project.file_index(&root.join("net.vhd")).unwrap()].text,
            netlist
        );
    }

    #[test]
    fn a_workspace_in_a_submodule_opens_the_superproject() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join(".git/modules/ip")).unwrap();
        std::fs::write(root.join("vfl.toml"), "[format]\nindent = 3\n").unwrap();
        std::fs::write(root.join("top.vhd"), "entity top is\nend entity;\n").unwrap();
        let ip = root.join("ip");
        std::fs::create_dir(&ip).unwrap();
        std::fs::write(ip.join(".git"), "gitdir: ../.git/modules/ip\n").unwrap();
        std::fs::write(ip.join("fifo.vhd"), "entity fifo is\nend entity;\n").unwrap();

        let mut world = World::new(&ip);
        assert_eq!(world.root.dir, root);
        assert_eq!(world.project.files.len(), 2);
        assert_eq!(world.config(&ip.join("fifo.vhd")).format.indent, 3);
    }

    #[test]
    fn unchanged_files_are_not_analyzed_again() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.vhd");
        let text = "entity a is\nend entity;\n";
        std::fs::write(&path, text).unwrap();
        let mut world = World::new(dir.path());
        assert!(!world.dirty);

        // Opening and closing a file as it is on disk.
        world.set_text(path.clone(), text.into());
        assert!(!world.dirty);
        world.close(&path);
        assert!(!world.dirty);

        // An edit, then closing without saving: back to the disk text.
        world.set_text(path.clone(), format!("{text}-- edit\n"));
        assert!(world.dirty);
        world.refresh();
        world.close(&path);
        assert!(world.dirty);
        assert_eq!(&*world.project.files[0].text, text);
        world.refresh();

        // A document from outside the project leaves it once closed.
        let other = tempfile::tempdir().unwrap();
        let outside = other.path().join("b.vhd");
        std::fs::write(&outside, text).unwrap();
        world.set_text(outside.clone(), text.into());
        assert_eq!(world.project.files.len(), 2);
        world.refresh();
        world.close(&outside);
        assert_eq!(world.project.files.len(), 1);
    }

    fn libraries(world: &World, name: &str) -> Vec<String> {
        let i = world
            .project
            .files
            .iter()
            .position(|f| f.path.ends_with(name))
            .unwrap();
        world.project.libraries[i]
            .iter()
            .map(|(l, _)| l.clone())
            .collect()
    }

    const PKG: &str = "package p is\nend package;\n";
    const TOP: &str = "entity top is\nend entity;\n";

    #[test]
    fn edits_that_name_a_library_infer_it() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("p.vhd"), PKG).unwrap();
        std::fs::write(dir.path().join("top.vhd"), TOP).unwrap();
        let mut world = World::new(dir.path());
        assert!(!libraries(&world, "p.vhd").contains(&"mylib".into()));
        let top = std::fs::canonicalize(dir.path().join("top.vhd")).unwrap();
        world.set_text(
            top.clone(),
            format!("library mylib;\nuse mylib.p.all;\n{TOP}"),
        );
        world.refresh();
        assert!(libraries(&world, "p.vhd").contains(&"mylib".into()));
        // An edit that changes no library name keeps the inference.
        world.set_text(
            top,
            format!("library mylib;\nuse mylib.p.all;\n{TOP}-- x\n"),
        );
        world.refresh();
        assert!(libraries(&world, "p.vhd").contains(&"mylib".into()));
    }

    #[test]
    fn new_files_join_inference() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("p.vhd"), PKG).unwrap();
        let mut world = World::new(dir.path());
        // Under the root as the world sees it (temp dirs can be symlinks).
        let top = world.root.dir.join("top.vhd");
        std::fs::write(&top, format!("library mylib;\nuse mylib.p.all;\n{TOP}")).unwrap();
        world.reread(&top);
        world.refresh();
        assert_eq!(world.project.libraries.len(), 2);
        assert!(libraries(&world, "p.vhd").contains(&"mylib".into()));
    }

    #[test]
    fn library_lists_are_read_again_when_they_change() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("p.vhd"), PKG).unwrap();
        let mut world = World::new(dir.path());
        assert!(!libraries(&world, "p.vhd").contains(&"fromtoml".into()));
        std::fs::write(
            dir.path().join("vhdl_ls.toml"),
            "[libraries]\nfromtoml.files = ['p.vhd']\n",
        )
        .unwrap();
        // Any edit refreshes; the new vhdl_ls.toml is noticed without a
        // file watcher.
        let p = std::fs::canonicalize(dir.path().join("p.vhd")).unwrap();
        world.set_text(p, format!("{PKG}-- x\n"));
        world.refresh();
        assert_eq!(libraries(&world, "p.vhd"), ["fromtoml"]);
    }
}

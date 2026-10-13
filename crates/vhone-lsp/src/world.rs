//! The server's view of the project: files on disk overlaid with open
//! editor buffers, their analysis and lint results.
//!
//! An edit re-parses only the edited file. Library inference runs again
//! only when the set of files, what a file declares or the library names
//! it refers to, or a library list (`vhdl_ls.toml`, tool files) changed.
//! The analysis then reuses the results of every design unit the edit
//! cannot have affected.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use vhone_config::Config;
use vhone_lint::FileSettings;
use vhone_project::{
    Encoding, FileSummary, LibrarySources, Project, ProjectRoot, RootKind, SourceFile,
    collect_sources, decode, find_root,
};
use vhone_sema::{Analysis, FileId};
use vhone_syntax::{LineIndex, TextSize};

pub struct World {
    pub root: ProjectRoot,
    pub project: Project,
    /// Open documents, by path.
    pub overlays: HashMap<PathBuf, String>,
    /// Open documents that are not files of the project.
    outside: HashSet<PathBuf>,
    pub analysis: Analysis,
    pub lint: Vec<vhone_lint::Diagnostic>,
    pub config_errors: Vec<String>,
    /// Why the project could not be loaded as configured.
    load_error: Option<String>,
    configs: HashMap<PathBuf, Config>,
    lint_cache: vhone_lint::LintCache,
    /// The library lists the libraries were last inferred from.
    sources: Option<LibrarySources>,
    /// The path and summary of each file when libraries were last
    /// inferred.
    inferred_from: Vec<(PathBuf, FileSummary)>,
    /// Files whose text changed since the last analysis.
    pub dirty: bool,
    /// The semantic tokens last sent, for answering with deltas.
    pub(crate) semantic: crate::semantic::Sent,
}

impl World {
    pub fn new(root: &Path) -> World {
        let root = find_root(root);
        let (project, load_error) = load(&root);
        let mut world = World {
            load_error,
            root,
            analysis: Analysis::new(Vec::new()),
            project,
            overlays: HashMap::new(),
            outside: HashSet::new(),
            lint: Vec::new(),
            config_errors: Vec::new(),
            configs: HashMap::new(),
            lint_cache: vhone_lint::LintCache::default(),
            sources: None,
            inferred_from: Vec::new(),
            dirty: true,
            semantic: Default::default(),
        };
        world.refresh();
        world
    }

    /// A world with no project, for documents that are not saved
    /// anywhere: only open documents are analyzed, with default settings.
    pub fn detached() -> World {
        let root = ProjectRoot {
            dir: PathBuf::new(),
            kind: RootKind::Fallback,
        };
        World {
            project: empty(&root, Config::default()),
            load_error: None,
            root,
            analysis: Analysis::new(Vec::new()),
            overlays: HashMap::new(),
            outside: HashSet::new(),
            lint: Vec::new(),
            config_errors: Vec::new(),
            configs: HashMap::new(),
            lint_cache: vhone_lint::LintCache::default(),
            sources: None,
            inferred_from: Vec::new(),
            dirty: true,
            semantic: Default::default(),
        }
    }

    /// Reloads every file from disk, keeping open documents.
    pub fn reload(&mut self) {
        self.configs.clear();
        self.root = find_root(&self.root.dir);
        (self.project, self.load_error) = load(&self.root);
        self.outside.clear();
        self.sources = None;
        self.inferred_from.clear();
        let overlays: Vec<(PathBuf, String)> = self.overlays.drain().collect();
        for (path, text) in overlays {
            self.set_text(path, text);
        }
        self.dirty = true;
    }

    /// The config for a file.
    /// The config for a file. A relative path names a document that is not
    /// saved anywhere, which gets the defaults.
    pub fn config(&mut self, path: &Path) -> &Config {
        let dir = config_dir(path);
        self.configs.entry(dir.clone()).or_insert_with(|| {
            if dir.as_os_str().is_empty() {
                Config::default()
            } else {
                vhone_config::load_for_dir(&dir).unwrap_or_default()
            }
        })
    }

    /// Sets the text of an open document. Opening a document that matches
    /// the file on disk changes nothing, so it does not analyze again.
    pub fn set_text(&mut self, path: PathBuf, text: String) {
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

    /// A document was closed: back to the file on disk, if any.
    pub fn close(&mut self, path: &Path) {
        self.overlays.remove(path);
        self.semantic.forget(path);
        self.reread(path);
    }

    /// Re-reads a file from disk (created, changed or deleted). A file
    /// whose text did not change is left as it is.
    pub fn reread(&mut self, path: &Path) {
        if self.overlays.contains_key(path) {
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
        let in_project = read.is_some()
            && (known
                || path.starts_with(&self.root.dir)
                    && collect_sources(&self.root.dir, &self.project.config.project)
                        .is_ok_and(|paths| paths.iter().any(|p| p == path)));
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
        self.analysis = vhone_lint::reanalyze(&self.project, &self.analysis);
        let paths: Vec<PathBuf> = self.project.files.iter().map(|f| f.path.clone()).collect();
        for path in &paths {
            self.config(path);
        }
        let configs: Vec<&Config> = paths
            .iter()
            .map(|p| &self.configs[&config_dir(p)])
            .collect();
        let settings: Vec<FileSettings> = self
            .project
            .files
            .iter()
            .zip(&configs)
            .enumerate()
            .map(|(i, (file, config))| FileSettings {
                file: i as FileId,
                syntax_errors: file.parse.errors(),
                config,
            })
            .collect();
        let result = vhone_lint::lint_cached(&self.analysis, &settings, &mut self.lint_cache);
        self.lint = result.diagnostics;
        self.config_errors = result.config_errors;
        if let Some(error) = &self.load_error {
            self.config_errors.insert(0, error.clone());
        }
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
                    return;
                }
            }
        }
        let files = &self.project.files;
        let unchanged = self.inferred_from.len() == files.len()
            && self.project.libraries.len() == files.len()
            && files
                .iter()
                .zip(&self.inferred_from)
                .all(|(file, (path, summary))| file.path == *path && file.summary == *summary);
        if unchanged {
            return;
        }
        let Some(sources) = &self.sources else {
            return;
        };
        self.project.infer_with(sources);
        self.inferred_from = self
            .project
            .files
            .iter()
            .map(|f| (f.path.clone(), f.summary.clone()))
            .collect();
    }

    pub fn file_id(&self, path: &Path) -> Option<FileId> {
        self.analysis.file_id(path)
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

/// The directory whose config applies to `path`; empty for a document
/// that is not saved anywhere.
fn config_dir(path: &Path) -> PathBuf {
    match path.parent() {
        Some(dir) if path.is_absolute() => dir.to_owned(),
        _ => PathBuf::new(),
    }
}

/// Loads the project. When that fails, an invalid `vhone.toml` say, the
/// sources are still loaded with the default settings, and the error is
/// returned to be reported.
fn load(root: &ProjectRoot) -> (Project, Option<String>) {
    match Project::load(root.clone()) {
        Ok(project) => (project, None),
        Err(error) => {
            let config = vhone_config::load_for_dir(&root.dir).unwrap_or_default();
            let project = collect_sources(&root.dir, &config.project)
                .ok()
                .and_then(|paths| Project::from_paths(root.clone(), config.clone(), paths).ok())
                .unwrap_or_else(|| empty(root, config));
            (project, Some(error.to_string()))
        }
    }
}

fn empty(root: &ProjectRoot, config: Config) -> Project {
    Project {
        root: root.clone(),
        config,
        files: Vec::new(),
        libraries: Vec::new(),
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
        let line = self.index.line_col(self.text, offset).line;
        let start = usize::from(self.index.line_start(line).unwrap_or_default());
        let character = self.text[start..usize::from(offset)].encode_utf16().count();
        lsp_types::Position::new(line, character as u32)
    }

    pub fn range(&self, range: vhone_syntax::TextRange) -> lsp_types::Range {
        lsp_types::Range::new(self.position(range.start()), self.position(range.end()))
    }

    pub fn offset(&self, position: lsp_types::Position) -> TextSize {
        offset(self.text, self.index, position)
    }
}

/// The offset of a UTF-16 `position` in `text`, clamped to the text.
pub fn offset(text: &str, index: &LineIndex, position: lsp_types::Position) -> TextSize {
    let Some(start) = index.line_start(position.line) else {
        return TextSize::of(text);
    };
    let start = usize::from(start);
    let line_end = text[start..].find('\n').map_or(text.len(), |i| start + i);
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
        std::fs::write(dir.path().join("vhone.toml"), "[format]\nindent = \"x\"\n").unwrap();
        std::fs::write(dir.path().join("a.vhd"), "entity a is\nend entity;\n").unwrap();
        let world = World::new(dir.path());
        assert_eq!(world.project.files.len(), 1);
        assert_eq!(world.config_errors.len(), 1, "{:?}", world.config_errors);
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

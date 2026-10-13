//! The server's view of the project: files on disk overlaid with open
//! editor buffers, their analysis and lint results.
//!
//! An edit re-parses only the edited file; library inference runs again
//! over the parsed project, then the analysis reuses the results of every
//! design unit the edit cannot have affected.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use vfl_config::Config;
use vfl_lint::FileSettings;
use vfl_project::{
    Encoding, Project, ProjectRoot, RootKind, SourceFile, collect_sources, decode, find_root,
};
use vfl_sema::{Analysis, FileId};
use vfl_syntax::{LineIndex, TextSize};

pub struct World {
    pub root: ProjectRoot,
    pub project: Project,
    /// Open documents, by path.
    pub overlays: HashMap<PathBuf, String>,
    /// Open documents that are not files of the project.
    outside: HashSet<PathBuf>,
    pub analysis: Analysis,
    pub lint: Vec<vfl_lint::Diagnostic>,
    pub config_errors: Vec<String>,
    /// Why the project could not be loaded as configured.
    load_error: Option<String>,
    configs: HashMap<PathBuf, Config>,
    lint_cache: vfl_lint::LintCache,
    /// Files whose text changed since the last analysis.
    pub dirty: bool,
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
            lint_cache: vfl_lint::LintCache::default(),
            dirty: true,
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
            lint_cache: vfl_lint::LintCache::default(),
            dirty: true,
        }
    }

    /// Reloads every file from disk, keeping open documents.
    pub fn reload(&mut self) {
        self.configs.clear();
        self.root = find_root(&self.root.dir);
        (self.project, self.load_error) = load(&self.root);
        self.outside.clear();
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
        let dir = match path.parent() {
            Some(dir) if path.is_absolute() => dir.to_owned(),
            _ => PathBuf::new(),
        };
        self.configs.entry(dir.clone()).or_insert_with(|| {
            if dir.as_os_str().is_empty() {
                Config::default()
            } else {
                vfl_config::load_for_dir(&dir).unwrap_or_default()
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
        if self.project.infer().is_err() {
            // An invalid library pattern: analyze with what inference had.
            self.project
                .libraries
                .resize(self.project.files.len(), Vec::new());
        }
        self.analysis = vfl_lint::reanalyze(&self.project, &self.analysis);
        let paths: Vec<PathBuf> = self.project.files.iter().map(|f| f.path.clone()).collect();
        let configs: Vec<Config> = paths.iter().map(|p| self.config(p).clone()).collect();
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
        let result = vfl_lint::lint_cached(&self.analysis, &settings, &mut self.lint_cache);
        self.lint = result.diagnostics;
        self.config_errors = result.config_errors;
        if let Some(error) = &self.load_error {
            self.config_errors.insert(0, error.clone());
        }
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

/// Loads the project. When that fails, an invalid `vfl.toml` say, the
/// sources are still loaded with the default settings, and the error is
/// returned to be reported.
fn load(root: &ProjectRoot) -> (Project, Option<String>) {
    match Project::load(root.clone()) {
        Ok(project) => (project, None),
        Err(error) => {
            let config = vfl_config::load_for_dir(&root.dir).unwrap_or_default();
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

    pub fn range(&self, range: vfl_syntax::TextRange) -> lsp_types::Range {
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
        std::fs::write(dir.path().join("vfl.toml"), "[format]\nindent = \"x\"\n").unwrap();
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
}

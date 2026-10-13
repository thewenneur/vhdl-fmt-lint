//! Formatting and linting editor buffers, independent of Notepad++.
//!
//! The engine keeps each project it has seen loaded, so linting a buffer
//! re-analyzes the project with the buffer's unsaved text in place of the
//! file on disk, and names resolve across files as with `vfl lint`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use vfl_config::{Config, Level};
use vfl_fmt::FormatError;
use vfl_lint::FileSettings;
use vfl_project::{Encoding, Project, ProjectRoot, RootKind, SourceFile, find_root};

/// How bad a problem is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Info,
    Warning,
    Error,
}

impl Severity {
    pub fn label(self) -> &'static str {
        match self {
            Severity::Info => "note",
            Severity::Warning => "warning",
            Severity::Error => "error",
        }
    }
}

/// A problem in a buffer. Offsets are byte offsets into the text given to
/// [`Engine::lint`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub start: usize,
    pub end: usize,
    pub severity: Severity,
    pub rule: &'static str,
    pub message: String,
}

/// The result of linting a buffer.
#[derive(Debug, Default)]
pub struct LintOutput {
    pub findings: Vec<Finding>,
    /// Problems with the configuration (unknown rules, bad options).
    pub config_errors: Vec<String>,
}

/// Why a buffer could not be formatted.
#[derive(Debug, PartialEq, Eq)]
pub struct FormatFailure {
    pub message: String,
    /// Where the first syntax error is, as a byte offset.
    pub offset: Option<usize>,
}

#[derive(Default)]
pub struct Engine {
    /// Loaded projects, by root directory.
    projects: HashMap<PathBuf, Project>,
    /// Effective configs, by directory.
    configs: HashMap<PathBuf, Config>,
}

impl Engine {
    /// Formats `text`, the content of the file at `path` (`None` for an
    /// unsaved buffer). Returns `None` when it is already formatted.
    pub fn format(
        &mut self,
        path: Option<&Path>,
        text: &str,
    ) -> Result<Option<String>, FormatFailure> {
        let config = self.config(path);
        match vfl_fmt::format(text, &config.format, config.standard()) {
            Ok(out) if out == text => Ok(None),
            Ok(out) => Ok(Some(out)),
            Err(FormatError::Syntax(errors)) => {
                let first = errors.first();
                Err(FormatFailure {
                    message: match first {
                        Some(e) => format!("syntax error: {}", e.message),
                        None => "syntax error".into(),
                    },
                    offset: first.map(|e| usize::from(e.range.start())),
                })
            }
            Err(err) => Err(FormatFailure {
                message: err.to_string(),
                offset: None,
            }),
        }
    }

    /// Lints `text`, the content of the file at `path` (`None` for an
    /// unsaved buffer), within its project.
    pub fn lint(&mut self, path: Option<&Path>, text: &str) -> LintOutput {
        let config = self.config(path).clone();
        let file_path = match path {
            Some(p) if p.is_absolute() => p.to_owned(),
            _ => PathBuf::from("untitled.vhd"),
        };
        let mut scratch;
        let project = match path.filter(|p| p.is_absolute()) {
            Some(path) => {
                let root = find_root(path);
                self.projects
                    .entry(root.dir.clone())
                    .or_insert_with(|| load(root))
            }
            None => {
                scratch = empty_project(
                    ProjectRoot {
                        dir: PathBuf::new(),
                        kind: RootKind::Fallback,
                    },
                    config.clone(),
                );
                &mut scratch
            }
        };
        let file = SourceFile::new(
            file_path.clone(),
            text.to_owned(),
            Encoding::Utf8,
            config.standard(),
        );
        // The buffer replaces the file on disk; a file outside the
        // project's sources joins it only for this run.
        let outsider = match project.file_index(&file_path) {
            Some(i) => {
                project.files[i] = file;
                false
            }
            None => {
                project.files.push(file);
                true
            }
        };
        if project.infer().is_err() {
            // An invalid library pattern: analyze with what inference had.
            project.libraries.resize(project.files.len(), Vec::new());
        }
        let analysis = vfl_lint::analyze(project);
        let mut output = LintOutput::default();
        if let Some(id) = analysis.file_id(&file_path) {
            let index = project
                .file_index(&file_path)
                .expect("the buffer was added");
            let settings = [FileSettings {
                file: id,
                syntax_errors: project.files[index].parse.errors(),
                config: &config,
            }];
            let result = vfl_lint::lint(&analysis, &settings);
            output.config_errors = result.config_errors;
            output.findings = result
                .diagnostics
                .into_iter()
                .filter(|d| d.file == id && d.level != Level::Off)
                .map(|d| Finding {
                    start: usize::from(d.range.start()),
                    end: usize::from(d.range.end()),
                    severity: match d.level {
                        Level::Error => Severity::Error,
                        Level::Warn => Severity::Warning,
                        _ => Severity::Info,
                    },
                    rule: d.rule,
                    message: d.message,
                })
                .collect();
        }
        if outsider {
            project.files.pop();
            project.libraries.truncate(project.files.len());
        }
        output
    }

    /// A buffer was closed or reverted: use the file on disk again.
    pub fn forget(&mut self, path: &Path) {
        for project in self.projects.values_mut() {
            let Some(i) = project.file_index(path) else {
                continue;
            };
            match SourceFile::read(path.to_owned(), project.config.standard()) {
                Ok(file) => project.files[i] = file,
                Err(_) => {
                    project.files.remove(i);
                    project.libraries.truncate(project.files.len());
                }
            }
        }
    }

    /// Drops every loaded project and config, to pick up changes made
    /// outside the editor.
    pub fn reload(&mut self) {
        self.projects.clear();
        self.configs.clear();
    }

    fn config(&mut self, path: Option<&Path>) -> &Config {
        let dir = path
            .filter(|p| p.is_absolute())
            .and_then(Path::parent)
            .map(Path::to_owned)
            .unwrap_or_default();
        self.configs.entry(dir.clone()).or_insert_with(|| {
            if dir.as_os_str().is_empty() {
                Config::default()
            } else {
                vfl_config::load_for_dir(&dir).unwrap_or_default()
            }
        })
    }
}

fn load(root: ProjectRoot) -> Project {
    match Project::load(root.clone()) {
        Ok(project) => project,
        Err(_) => {
            let config = vfl_config::load_for_dir(&root.dir).unwrap_or_default();
            empty_project(root, config)
        }
    }
}

fn empty_project(root: ProjectRoot, config: Config) -> Project {
    Project {
        root,
        config,
        files: Vec::new(),
        libraries: Vec::new(),
        unreadable: Vec::new(),
        notes: Vec::new(),
    }
}

/// Whether a file name looks like VHDL.
pub fn is_vhdl_path(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| {
        ["vhd", "vhdl", "vho", "vht"]
            .iter()
            .any(|v| e.eq_ignore_ascii_case(v))
    })
}

/// Whether saving `path` changes how projects load, so they need reloading.
pub fn is_project_file(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| matches!(n, "vfl.toml" | ".vfl.toml" | "vhdl_ls.toml"))
}

/// A buffer's bytes as text, and how to map offsets back.
///
/// Notepad++ buffers are UTF-8 or a single-byte code page. Single-byte
/// buffers are read as Latin-1, byte for byte, like `vfl` reads files.
pub struct BufferText {
    pub text: String,
    pub encoding: Encoding,
}

impl BufferText {
    pub fn new(bytes: &[u8], utf8: bool) -> BufferText {
        match (utf8, std::str::from_utf8(bytes)) {
            (true, Ok(text)) => BufferText {
                text: text.to_owned(),
                encoding: Encoding::Utf8,
            },
            _ => BufferText {
                text: bytes.iter().map(|&b| char::from(b)).collect(),
                encoding: Encoding::Latin1,
            },
        }
    }

    /// The buffer position of a byte offset into [`Self::text`].
    pub fn position(&self, offset: usize) -> usize {
        let offset = offset.min(self.text.len());
        match self.encoding {
            Encoding::Utf8 => offset,
            Encoding::Latin1 => self.text[..offset].chars().count(),
        }
    }

    /// Text in the buffer's encoding.
    pub fn encode(&self, text: &str) -> Vec<u8> {
        vfl_project::encode(text, self.encoding)
    }
}

/// The smallest edit turning `old` into `new`: the byte range of `old` to
/// replace, and its replacement.
pub fn minimal_edit<'a>(old: &[u8], new: &'a [u8]) -> (usize, usize, &'a [u8]) {
    let mut prefix = old.iter().zip(new).take_while(|(a, b)| a == b).count();
    // Keep multi-byte UTF-8 sequences whole.
    while prefix > 0 && (is_continuation(old.get(prefix)) || is_continuation(new.get(prefix))) {
        prefix -= 1;
    }
    let max_suffix = old.len().min(new.len()) - prefix;
    let mut suffix = old
        .iter()
        .rev()
        .zip(new.iter().rev())
        .take(max_suffix)
        .take_while(|(a, b)| a == b)
        .count();
    while suffix > 0
        && (is_continuation(old.get(old.len() - suffix))
            || is_continuation(new.get(new.len() - suffix)))
    {
        suffix -= 1;
    }
    (prefix, old.len() - suffix, &new[prefix..new.len() - suffix])
}

fn is_continuation(byte: Option<&u8>) -> bool {
    byte.is_some_and(|b| b & 0xC0 == 0x80)
}

#[cfg(test)]
mod tests {
    use super::*;

    const UNUSED: &str = "\
entity e is
end entity;

architecture a of e is
  signal s : bit;
begin
end architecture;
";

    #[test]
    fn formats_unsaved_buffers() {
        let mut engine = Engine::default();
        let out = engine
            .format(None, "ENTITY e IS\nEND ENTITY;\n")
            .expect("formats");
        assert!(out.is_some_and(|t| t.contains("entity e is")));
        assert_eq!(engine.format(None, "entity e is\nend entity;\n"), Ok(None));
    }

    #[test]
    fn reports_syntax_errors_when_formatting() {
        let mut engine = Engine::default();
        let err = engine
            .format(None, "entity e is\nend;\nfoo bar;\n")
            .unwrap_err();
        assert!(err.message.starts_with("syntax error"), "{}", err.message);
        assert!(err.offset.is_some());
    }

    #[test]
    fn lints_unsaved_buffers() {
        let mut engine = Engine::default();
        let output = engine.lint(None, UNUSED);
        let unused = output
            .findings
            .iter()
            .find(|f| f.rule == "sema/unused-signal")
            .expect("unused signal reported");
        assert_eq!(&UNUSED[unused.start..unused.end], "s");
        assert_eq!(unused.severity, Severity::Warning);
    }

    #[test]
    fn lints_buffers_within_their_project() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join("vfl.toml"), "").unwrap();
        std::fs::write(
            root.join("pkg.vhd"),
            "package pkg is\n  constant c : integer := 1;\nend package;\n",
        )
        .unwrap();
        let top = root.join("top.vhd");
        let text = "\
use work.pkg.all;
entity top is
end entity;
architecture a of top is
  signal s : integer := c;
begin
  s <= d;
end architecture;
";
        std::fs::write(&top, "").unwrap();
        let mut engine = Engine::default();
        let output = engine.lint(Some(&top), text);
        let messages: Vec<&str> = output.findings.iter().map(|f| f.message.as_str()).collect();
        // `c` resolves through the package on disk; `d` does not exist.
        assert!(messages.iter().all(|m| !m.contains("`c`")), "{messages:?}");
        assert!(messages.iter().any(|m| m.contains("`d`")), "{messages:?}");

        // Editing the package in the editor is seen by the next lint.
        let pkg = root.join("pkg.vhd");
        engine.lint(
            Some(&pkg),
            "package pkg is\n  constant d : integer := 1;\nend package;\n",
        );
        let output = engine.lint(Some(&top), text);
        let messages: Vec<&str> = output.findings.iter().map(|f| f.message.as_str()).collect();
        assert!(messages.iter().any(|m| m.contains("`c`")), "{messages:?}");

        // Closing it without saving goes back to the file on disk.
        engine.forget(&pkg);
        let output = engine.lint(Some(&top), text);
        let messages: Vec<&str> = output.findings.iter().map(|f| f.message.as_str()).collect();
        assert!(messages.iter().all(|m| !m.contains("`c`")), "{messages:?}");
    }

    #[test]
    fn files_outside_the_project_do_not_stay_in_it() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(
            root.join("vfl.toml"),
            "[project]\nexclude = [\"scratch.vhd\"]\n",
        )
        .unwrap();
        let outside = root.join("scratch.vhd");
        let mut engine = Engine::default();
        engine.lint(Some(&outside), UNUSED);
        let project = engine.projects.values().next().unwrap();
        assert!(project.file_index(&outside).is_none());
    }

    #[test]
    fn maps_latin1_offsets() {
        let buffer = BufferText::new(b"-- caf\xe9\nx", false);
        assert_eq!(buffer.text, "-- caf\u{e9}\nx");
        let x = buffer.text.find('x').unwrap();
        assert_eq!(buffer.position(x), 8);
        assert_eq!(buffer.encode("\u{e9}"), vec![0xe9]);

        let buffer = BufferText::new("é x".as_bytes(), true);
        assert_eq!(buffer.position(3), 3);
    }

    #[test]
    fn edits_only_what_changed() {
        assert_eq!(minimal_edit(b"abcXdef", b"abcYYdef"), (3, 4, &b"YY"[..]));
        assert_eq!(minimal_edit(b"same", b"same"), (4, 4, &b""[..]));
        assert_eq!(minimal_edit(b"aaa", b"aaaa"), (3, 3, &b"a"[..]));
        // é (C3 A9) to è (C3 A8): the whole character is replaced.
        let (start, end, with) = minimal_edit("é".as_bytes(), "è".as_bytes());
        assert_eq!((start, end, with), (0, 2, "è".as_bytes()));
    }

    #[test]
    fn recognizes_vhdl_files() {
        assert!(is_vhdl_path(Path::new("a/top.VHD")));
        assert!(is_vhdl_path(Path::new("tb.vhdl")));
        assert!(!is_vhdl_path(Path::new("top.v")));
        assert!(is_project_file(Path::new("x/vfl.toml")));
    }
}

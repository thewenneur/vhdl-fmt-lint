//! Formatting and linting editor buffers, independent of Notepad++.
//!
//! The engine keeps each project it has seen loaded, like the language
//! server does, with each buffer's unsaved text in place of the file on
//! disk. Names resolve across files as with `vfl lint`, and an edit
//! re-analyzes only the design units it can affect. Hover, parameter hints
//! and completion come from the language server's code.

use std::collections::HashMap;
use std::ops::Range;
use std::path::{Path, PathBuf};

use vfl_config::{Config, Level};
use vfl_fmt::FormatError;
use vfl_lsp::World;
use vfl_lsp::editor::{self, Completions, HoverInfo, SignatureHelp};
use vfl_project::{Encoding, find_root};

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
    /// Loaded projects with the open buffers' text, by root directory.
    worlds: HashMap<PathBuf, World>,
    /// Buffers that are not saved anywhere.
    untitled: Option<World>,
    /// Effective configs, by directory.
    configs: HashMap<PathBuf, Config>,
}

/// The name unsaved buffers are analyzed under.
const UNTITLED: &str = "untitled.vhd";

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
        let (world, file_path) = self.sync(path, text);
        world.refresh();
        let mut output = LintOutput {
            findings: Vec::new(),
            config_errors: world.config_errors.clone(),
        };
        if let Some(id) = world.file_id(&file_path) {
            output.findings = world
                .lint
                .iter()
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
                    message: d.message.clone(),
                })
                .collect();
        }
        output
    }

    /// What to show when hovering byte `offset` of `text`: the kind, name
    /// and type of what is named there, and its declaration.
    pub fn hover(&mut self, path: Option<&Path>, text: &str, offset: usize) -> Option<HoverInfo> {
        let (world, file_path) = self.sync(path, text);
        editor::hover(world, &file_path, offset)
    }

    /// The signatures of the call or port map around byte `offset`.
    pub fn signature_help(
        &mut self,
        path: Option<&Path>,
        text: &str,
        offset: usize,
    ) -> Option<SignatureHelp> {
        let (world, file_path) = self.sync(path, text);
        editor::signature_help(world, &file_path, offset)
    }

    /// The completions at byte `offset`.
    pub fn completion(
        &mut self,
        path: Option<&Path>,
        text: &str,
        offset: usize,
    ) -> Option<Completions> {
        let (world, file_path) = self.sync(path, text);
        editor::completion(world, &file_path, offset)
    }

    /// The world a buffer belongs to, with the buffer's text in place of
    /// the file on disk, and the path it is analyzed under.
    fn sync(&mut self, path: Option<&Path>, text: &str) -> (&mut World, PathBuf) {
        let (world, file_path) = match path.filter(|p| p.is_absolute()) {
            Some(path) => {
                let root = find_root(path);
                let world = self
                    .worlds
                    .entry(root.dir.clone())
                    .or_insert_with(|| World::new(&root.dir));
                (world, path.to_owned())
            }
            None => (
                self.untitled.get_or_insert_with(World::detached),
                PathBuf::from(UNTITLED),
            ),
        };
        if world.overlays.get(&file_path).map(String::as_str) != Some(text) {
            world.set_text(file_path.clone(), text.to_owned());
        }
        (world, file_path)
    }

    /// A buffer was closed or reverted: use the file on disk again. A file
    /// outside its project's sources leaves the project.
    pub fn forget(&mut self, path: &Path) {
        for world in self.worlds.values_mut() {
            if world.overlays.contains_key(path) {
                world.close(path);
            }
        }
    }

    /// Drops every loaded project and config, to pick up changes made
    /// outside the editor.
    pub fn reload(&mut self) {
        self.worlds.clear();
        self.untitled = None;
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

/// A hover's Markdown as plain text for a call tip: code blocks without
/// their fences, names without backticks, no blank lines between
/// paragraphs, and a rule between declarations.
pub fn plain_text(markdown: &str) -> String {
    let mut lines: Vec<String> = Vec::new();
    let mut in_code = false;
    for line in markdown.lines() {
        if line.starts_with("```") {
            in_code = !in_code;
            continue;
        }
        if in_code {
            lines.push(line.to_owned());
        } else if line.trim() == "---" {
            lines.push("\u{2014}".repeat(8));
        } else if !line.trim().is_empty() {
            lines.push(line.replace('`', ""));
        }
    }
    lines.join("\n")
}

/// How many signatures a parameter tip lists.
const MAX_SIGNATURES: usize = 8;

/// A parameter tip: one signature per line, and the byte range of the
/// parameter the cursor is in, to highlight.
pub fn signature_tip(help: &SignatureHelp) -> (String, Option<Range<usize>>) {
    let mut text = String::new();
    let mut highlight = None;
    for (i, signature) in help.signatures.iter().take(MAX_SIGNATURES).enumerate() {
        if i > 0 {
            text.push('\n');
        }
        if i == help.active
            && let Some(p) = signature.active_parameter
            && let Some(range) = signature.parameters.get(p)
        {
            highlight = Some(text.len() + range.start..text.len() + range.end);
        }
        text.push_str(&signature.label);
    }
    if help.signatures.len() > MAX_SIGNATURES {
        text.push_str(&format!(
            "\n({} more)",
            help.signatures.len() - MAX_SIGNATURES
        ));
    }
    (text, highlight)
}

/// Completion labels for Scintilla's list: unique ignoring case, without
/// labels that cannot be typed as they are.
pub fn completion_labels(completions: &Completions) -> Vec<&str> {
    let mut seen = std::collections::HashSet::new();
    let mut labels: Vec<&str> = completions
        .items
        .iter()
        .map(|c| c.label.as_str())
        .filter(|l| !l.is_empty() && !l.contains(['\n', '\0']))
        .filter(|l| seen.insert(l.to_ascii_lowercase()))
        .collect();
    labels.sort_by_key(|l| l.to_ascii_lowercase());
    labels
}

/// Whether typing `.` just before byte `offset` should offer completions:
/// after a name or a closing parenthesis, in code, and not in a number.
pub fn completes_after_dot(text: &str, offset: usize) -> bool {
    let Some(before) = text.get(..offset).and_then(|t| t.strip_suffix('.')) else {
        return false;
    };
    let word_start = before
        .rfind(|c: char| !(c.is_alphanumeric() || c == '_'))
        .map_or(0, |i| i + 1);
    let word = &before[word_start..];
    let after_name = word.chars().next().is_some_and(|c| !c.is_ascii_digit());
    (after_name || before.ends_with(')')) && in_code(text, offset - 1)
}

/// Whether byte `offset` is in code rather than in a comment, a string or
/// a character literal, judging from its line.
pub fn in_code(text: &str, offset: usize) -> bool {
    let Some(before) = text.get(..offset) else {
        return false;
    };
    let line = &before[before.rfind('\n').map_or(0, |i| i + 1)..];
    let chars: Vec<char> = line.chars().collect();
    let mut i = 0;
    let mut in_string = false;
    while i < chars.len() {
        match chars[i] {
            '"' => in_string = !in_string,
            '-' if !in_string && chars.get(i + 1) == Some(&'-') => return false,
            // A character literal: `'x'`, but not the tick of `a'length`.
            '\'' if !in_string
                && chars.get(i + 2) == Some(&'\'')
                && !i
                    .checked_sub(1)
                    .and_then(|p| chars.get(p))
                    .is_some_and(|c| c.is_alphanumeric() || *c == '_' || *c == ')') =>
            {
                i += 2;
            }
            _ => {}
        }
        i += 1;
    }
    !in_string
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
    path.file_name().and_then(|n| n.to_str()).is_some_and(|n| {
        matches!(
            n,
            "vfl.toml" | ".vfl.toml" | "vhdl_ls.toml" | ".vhdl_ls.toml"
        )
    }) || vfl_project::is_tool_file(path)
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

    /// The byte offset into [`Self::text`] of a buffer position.
    pub fn offset(&self, position: usize) -> usize {
        match self.encoding {
            Encoding::Utf8 => position.min(self.text.len()),
            Encoding::Latin1 => self
                .text
                .char_indices()
                .nth(position)
                .map_or(self.text.len(), |(i, _)| i),
        }
    }

    /// How many bytes `text` takes in the buffer's encoding.
    pub fn encoded_len(&self, text: &str) -> usize {
        match self.encoding {
            Encoding::Utf8 => text.len(),
            Encoding::Latin1 => text.chars().count(),
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
    fn files_outside_the_project_leave_it_when_closed() {
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
        engine.forget(&outside);
        let world = engine.worlds.values().next().unwrap();
        assert!(world.project.file_index(&outside).is_none());
    }

    const TYPES: &str = "\
entity e is
end entity;

architecture a of e is
  type rec is record
    valid : bit;
    count : integer;
  end record;
  signal r : rec;
  function scale(x : integer; factor : integer := 2) return integer is
  begin
    return x * factor;
  end function;
  signal n : integer;
begin
  n <= scale(r.count, 3);
end architecture;
";

    fn at(text: &str, needle: &str) -> usize {
        text.find(needle).expect("needle in text")
    }

    #[test]
    fn hovers_show_types() {
        let mut engine = Engine::default();
        let offset = at(TYPES, "r.count");
        let hover = engine.hover(None, TYPES, offset).expect("hover over r");
        assert_eq!(&TYPES[hover.range.clone()], "r");
        let text = plain_text(&hover.markdown);
        assert!(text.contains("type rec"), "{text}");
        assert!(text.contains("count : integer"), "{text}");
        assert!(!text.contains('`'), "{text}");

        // A record element has no declaration but still has a type.
        let hover = engine
            .hover(None, TYPES, at(TYPES, "count, 3"))
            .expect("hover over the element");
        assert!(plain_text(&hover.markdown).contains("element count of rec"));
    }

    #[test]
    fn shows_parameters_of_calls() {
        let mut engine = Engine::default();
        let offset = at(TYPES, ", 3") + 2;
        let help = engine
            .signature_help(None, TYPES, offset)
            .expect("signature help");
        let (tip, highlight) = signature_tip(&help);
        assert!(tip.starts_with("function scale("), "{tip}");
        assert_eq!(
            &tip[highlight.expect("a parameter")],
            "factor : in integer := 2"
        );
    }

    #[test]
    fn completes_record_elements() {
        let mut engine = Engine::default();
        let text = TYPES.replace("scale(r.count, 3)", "scale(r.co");
        let offset = at(&text, "r.co") + 4;
        let completions = engine.completion(None, &text, offset).expect("completions");
        assert_eq!(&text[completions.start..offset], "co");
        assert_eq!(completion_labels(&completions), ["count", "valid"]);

        // Elsewhere: what is visible, and keywords.
        let offset = at(TYPES, "n <=");
        let completions = engine.completion(None, TYPES, offset).unwrap();
        let labels = completion_labels(&completions);
        for label in ["scale", "r", "rec", "process", "integer"] {
            assert!(labels.contains(&label), "{label} missing");
        }
    }

    #[test]
    fn completes_only_after_names_in_code() {
        let ok = |text: &str| completes_after_dot(text, text.len());
        assert!(ok("x <= r."));
        assert!(ok("use ieee."));
        assert!(ok("x <= a(1)."));
        assert!(!ok("x := 1."));
        assert!(!ok("-- see r."));
        assert!(!ok("report \"r."));
        assert!(!ok("x <= ."));
        assert!(ok("x <= '1' when r."));
        assert!(ok("x <= a'length + r."));
    }

    #[test]
    fn markdown_becomes_plain_text() {
        let text = plain_text(
            "signal `s` in `work.e`\n\ntype `bit`\n\n```vhdl\nsignal s : bit;\n```\n\n---\n\nlibrary `ieee`",
        );
        assert_eq!(
            text,
            "signal s in work.e\ntype bit\nsignal s : bit;\n\u{2014}\u{2014}\u{2014}\u{2014}\u{2014}\u{2014}\u{2014}\u{2014}\nlibrary ieee"
        );
    }

    #[test]
    fn maps_latin1_offsets() {
        let buffer = BufferText::new(b"-- caf\xe9\nx", false);
        assert_eq!(buffer.text, "-- caf\u{e9}\nx");
        let x = buffer.text.find('x').unwrap();
        assert_eq!(buffer.position(x), 8);
        assert_eq!(buffer.encode("\u{e9}"), vec![0xe9]);

        assert_eq!(buffer.offset(8), x);
        assert_eq!(buffer.encoded_len("caf\u{e9}"), 4);

        let buffer = BufferText::new("é x".as_bytes(), true);
        assert_eq!(buffer.position(3), 3);
        assert_eq!(buffer.offset(3), 3);
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
        assert!(is_project_file(Path::new("x/sim/run.py")));
    }
}

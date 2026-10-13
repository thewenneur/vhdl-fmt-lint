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
use std::time::SystemTime;

use vfl_config::{Config, Level};
use vfl_fmt::FormatError;
use vfl_lsp::World;
use vfl_lsp::editor::{
    self, Completion, Completions, FileEdits, HoverInfo, Location, ProjectProblem, ProjectSymbol,
    QuickFix, SignatureHelp, Symbol, TokenKind, Use,
};
use vfl_lsp::generate::{self, EntityRef, Generated, Kind};
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

/// How a name is colored, by what it resolves to. Names of other kinds
/// keep the lexer's colors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NameColor {
    Signal,
    Port,
    /// A constant, generic or enumeration literal.
    Constant,
    /// A variable or subprogram parameter.
    Variable,
    Type,
    /// A function or procedure.
    Subprogram,
}

impl NameColor {
    pub const ALL: [NameColor; 6] = [
        NameColor::Signal,
        NameColor::Port,
        NameColor::Constant,
        NameColor::Variable,
        NameColor::Type,
        NameColor::Subprogram,
    ];

    fn of(kind: TokenKind) -> Option<NameColor> {
        Some(match kind {
            TokenKind::Signal => NameColor::Signal,
            TokenKind::Port => NameColor::Port,
            TokenKind::Constant | TokenKind::Generic | TokenKind::EnumMember => NameColor::Constant,
            TokenKind::Variable | TokenKind::Parameter => NameColor::Variable,
            TokenKind::Type | TokenKind::TypeParameter => NameColor::Type,
            TokenKind::Function | TokenKind::Procedure => NameColor::Subprogram,
            _ => return None,
        })
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

fn format_failure(err: FormatError) -> FormatFailure {
    match err {
        FormatError::Syntax(errors) => {
            let first = errors.first();
            FormatFailure {
                message: match first {
                    Some(e) => format!("syntax error: {}", e.message),
                    None => "syntax error".into(),
                },
                offset: first.map(|e| usize::from(e.range.start())),
            }
        }
        err => FormatFailure {
            message: err.to_string(),
            offset: None,
        },
    }
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
    /// When each file a project reads was last modified, by root
    /// directory, to notice changes made outside the editor.
    stamps: HashMap<PathBuf, Stamps>,
}

#[derive(Default)]
struct Stamps {
    sources: HashMap<PathBuf, SystemTime>,
    /// Configuration and other tools' project files.
    settings: HashMap<PathBuf, SystemTime>,
}

impl Stamps {
    /// The modification times of the files `world`'s project reads.
    fn of(world: &World) -> Option<Stamps> {
        let (sources, tool_files) =
            vfl_project::collect(&world.root.dir, &world.project.config.project).ok()?;
        let stamp = |paths: Vec<PathBuf>| -> HashMap<PathBuf, SystemTime> {
            paths
                .into_iter()
                .filter_map(|p| {
                    let modified = std::fs::metadata(&p).and_then(|m| m.modified()).ok()?;
                    Some((p, modified))
                })
                .collect()
        };
        let settings: Vec<PathBuf> = std::fs::read_dir(&world.root.dir)
            .into_iter()
            .flatten()
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| is_project_file(p))
            .chain(tool_files)
            .collect();
        Some(Stamps {
            sources: stamp(sources),
            settings: stamp(settings),
        })
    }
}

/// How many changed sources are re-read one by one before reloading the
/// whole project is cheaper.
const MAX_REREADS: usize = 16;

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
        let out =
            vfl_fmt::format(text, &config.format, config.standard()).map_err(format_failure)?;
        Ok((out != text).then_some(out))
    }

    /// Formats the lines of `text` that bytes `range` touch (an empty range
    /// stands for its line), and the statements and aligned lines they are
    /// part of. Returns `None` when they are already formatted.
    pub fn format_range(
        &mut self,
        path: Option<&Path>,
        text: &str,
        range: Range<usize>,
    ) -> Result<Option<String>, FormatFailure> {
        let config = self.config(path);
        let edits = vfl_fmt::format_range(text, range, &config.format, config.standard())
            .map_err(format_failure)?;
        Ok((!edits.is_empty()).then(|| vfl_fmt::apply_edits(text, &edits)))
    }

    /// Formats the line just finished after `ch` was typed at byte `cursor`
    /// of `text`: the line of a `;`, or the line above a new line. Returns
    /// `None` when there is nothing to change, `on_type` is off in
    /// `vfl.toml`, or the buffer does not parse.
    pub fn format_typed(
        &mut self,
        path: Option<&Path>,
        text: &str,
        cursor: usize,
        ch: char,
    ) -> Option<String> {
        let config = self.config(path);
        let edits =
            vfl_fmt::format_typed(text, cursor, ch, &config.format, config.standard()).ok()?;
        (!edits.is_empty()).then(|| vfl_fmt::apply_edits(text, &edits))
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

    /// Where what is named at byte `offset` is declared.
    pub fn definition(&mut self, path: Option<&Path>, text: &str, offset: usize) -> Vec<Location> {
        let (world, file_path) = self.sync(path, text);
        editor::definition(world, &file_path, offset)
    }

    /// Every place in the project that names what is named at `offset`.
    pub fn references(&mut self, path: Option<&Path>, text: &str, offset: usize) -> Vec<Location> {
        let (world, file_path) = self.sync(path, text);
        editor::references(world, &file_path, offset)
    }

    /// The places in this buffer that name what is named at `offset`.
    pub fn highlights(
        &mut self,
        path: Option<&Path>,
        text: &str,
        offset: usize,
    ) -> Vec<(Range<usize>, Use)> {
        let (world, file_path) = self.sync(path, text);
        editor::highlights(world, &file_path, offset)
    }

    /// The names in the buffer to color, as byte ranges, in order.
    pub fn name_colors(
        &mut self,
        path: Option<&Path>,
        text: &str,
    ) -> Vec<(Range<usize>, NameColor)> {
        let (world, file_path) = self.sync(path, text);
        editor::semantic_tokens(world, &file_path)
            .into_iter()
            .filter_map(|t| Some((t.range, NameColor::of(t.kind)?)))
            .collect()
    }

    /// The buffer's declarations, nested.
    pub fn outline(&mut self, path: Option<&Path>, text: &str) -> Vec<Symbol> {
        let (world, file_path) = self.sync(path, text);
        editor::outline(world, &file_path)
    }

    /// The declarations in the buffer's project whose name contains
    /// `query`.
    pub fn project_symbols(
        &mut self,
        path: Option<&Path>,
        text: &str,
        query: &str,
    ) -> Vec<ProjectSymbol> {
        let (world, _) = self.sync(path, text);
        editor::project_symbols(world, query)
    }

    /// The problems of every file of the buffer's project, by file and
    /// position.
    pub fn project_problems(&mut self, path: Option<&Path>, text: &str) -> Vec<ProjectProblem> {
        let (world, _) = self.sync(path, text);
        editor::problems(world)
    }

    /// The name a rename at `offset` would change, or why it cannot.
    pub fn prepare_rename(
        &mut self,
        path: Option<&Path>,
        text: &str,
        offset: usize,
    ) -> Result<Range<usize>, String> {
        let (world, file_path) = self.sync(path, text);
        editor::prepare_rename(world, &file_path, offset)
    }

    /// The edits renaming what is named at `offset`, by file.
    pub fn rename(
        &mut self,
        path: Option<&Path>,
        text: &str,
        offset: usize,
        new_name: &str,
    ) -> Result<Vec<FileEdits>, String> {
        let (world, file_path) = self.sync(path, text);
        let mut edits = editor::rename(world, &file_path, offset, new_name)?;
        if path.is_none_or(|p| !p.is_absolute()) {
            // Unsaved buffers are analyzed under a made-up name.
            edits.retain(|e| e.path == file_path);
        }
        Ok(edits)
    }

    /// The fixes of the problems touching bytes `start..end`.
    pub fn quick_fixes(
        &mut self,
        path: Option<&Path>,
        text: &str,
        start: usize,
        end: usize,
    ) -> Vec<QuickFix> {
        let (world, file_path) = self.sync(path, text);
        editor::quick_fixes(world, &file_path, start, end)
    }

    /// The buffer's text with every safe fix applied, and how many were.
    pub fn fix_all(&mut self, path: Option<&Path>, text: &str) -> Option<(String, usize)> {
        let (world, file_path) = self.sync(path, text);
        editor::fix_all(world, &file_path)
    }

    /// The entities of the buffer's project.
    pub fn entities(&mut self, path: Option<&Path>, text: &str) -> Vec<EntityRef> {
        let (world, _) = self.sync(path, text);
        generate::entities(world)
    }

    /// Code generated from `entity` (`name` or `library.name`) of the
    /// buffer's project.
    pub fn generate(
        &mut self,
        path: Option<&Path>,
        text: &str,
        kind: Kind,
        entity: &str,
    ) -> Result<Generated, String> {
        let (world, _) = self.sync(path, text);
        generate::generate(world, kind, entity)
    }

    /// Sets the text of a buffer without asking anything about it.
    pub fn update(&mut self, path: Option<&Path>, text: &str) {
        self.sync(path, text);
    }

    /// Re-reads what changed on disk since the last call in the projects
    /// loaded so far: sources added, changed or deleted outside the editor,
    /// and settings files. Returns whether anything did.
    pub fn rescan(&mut self) -> bool {
        let mut changed = false;
        for (root, world) in &mut self.worlds {
            let Some(now) = Stamps::of(world) else {
                continue;
            };
            let Some(before) = self.stamps.insert(root.clone(), now) else {
                continue;
            };
            let now = &self.stamps[root];
            if before.settings != now.settings {
                world.reload();
                changed = true;
                continue;
            }
            let mut differ: Vec<&PathBuf> = now
                .sources
                .iter()
                .filter(|(p, t)| before.sources.get(*p) != Some(t))
                .map(|(p, _)| p)
                .collect();
            differ.extend(
                before
                    .sources
                    .keys()
                    .filter(|p| !now.sources.contains_key(*p)),
            );
            if differ.len() > MAX_REREADS {
                world.reload();
            } else {
                for path in &differ {
                    world.reread(path);
                }
            }
            changed |= !differ.is_empty();
        }
        if changed {
            self.configs.clear();
        }
        changed
    }

    /// The text the engine analyzes for the file at `path`, if it has
    /// loaded it: the buffer's last text, or the file on disk.
    pub fn analyzed_text(&self, path: &Path) -> Option<&str> {
        let world = self.worlds.get(&find_root(path).dir)?;
        let index = world.project.file_index(path)?;
        Some(&world.project.files[index].text)
    }

    /// The world a buffer belongs to, with the buffer's text in place of
    /// the file on disk, and the path it is analyzed under.
    fn sync(&mut self, path: Option<&Path>, text: &str) -> (&mut World, PathBuf) {
        let (world, file_path) = match path.filter(|p| p.is_absolute()) {
            Some(path) => {
                let root = find_root(path);
                let world = self.worlds.entry(root.dir.clone()).or_insert_with(|| {
                    let world = World::new(&root.dir);
                    if let Some(stamps) = Stamps::of(&world) {
                        self.stamps.insert(root.dir.clone(), stamps);
                    }
                    world
                });
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
        self.stamps.clear();
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

/// Completions for Scintilla's list: unique labels ignoring case, sorted,
/// without labels that cannot be typed as they are.
pub fn completion_list(completions: &Completions) -> Vec<&Completion> {
    let mut seen = std::collections::HashSet::new();
    let mut items: Vec<&Completion> = completions
        .items
        .iter()
        .filter(|c| !c.label.is_empty() && !c.label.contains(['\n', '\0', '\x1e']))
        .filter(|c| seen.insert(c.label.to_ascii_lowercase()))
        .collect();
    items.sort_by_key(|c| c.label.to_ascii_lowercase());
    items
}

/// Whether `path` is a copy of a bundled library source (`ieee`, `std`)
/// that go to definition wrote for the editor to open.
pub fn is_library_copy(path: &Path) -> bool {
    path.starts_with(std::env::temp_dir().join(format!("vfl-{}", env!("CARGO_PKG_VERSION"))))
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
    fn formats_a_selection() {
        let mut engine = Engine::default();
        let text = "ENTITY e IS\nEND ENTITY;\nENTITY f IS\nEND ENTITY;\n";
        assert_eq!(
            engine.format_range(None, text, 0..3),
            Ok(Some(
                "entity e is\nEND ENTITY;\nENTITY f IS\nEND ENTITY;\n".into()
            ))
        );
        assert_eq!(
            engine.format_range(None, "entity e is\nend;\n", 0..0),
            Ok(None)
        );
    }

    #[test]
    fn formats_the_line_just_typed() {
        let mut engine = Engine::default();
        let text = "entity e is\nend;\narchitecture a of e is\nbegin\nx<=y;\nend;\n";
        let cursor = text.find("y;").unwrap() + 2;
        assert_eq!(
            engine.format_typed(None, text, cursor, ';'),
            Some("entity e is\nend;\narchitecture a of e is\nbegin\n  x <= y;\nend;\n".into())
        );
        let text = "entity e is\nend;\narchitecture a of e is\nbegin\nx<=y;\n  \nend;\n";
        let cursor = text.find("  \n").unwrap() + 2;
        assert_eq!(
            engine.format_typed(None, text, cursor, '\n'),
            Some("entity e is\nend;\narchitecture a of e is\nbegin\n  x <= y;\n  \nend;\n".into())
        );
        assert_eq!(engine.format_typed(None, text, cursor, 'x'), None);
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
    fn colors_names_by_meaning() {
        let mut engine = Engine::default();
        let colors: Vec<(&str, NameColor)> = engine
            .name_colors(None, TYPES)
            .into_iter()
            .map(|(range, color)| (&TYPES[range], color))
            .collect();
        for expected in [
            ("rec", NameColor::Type),
            ("r", NameColor::Signal),
            ("scale", NameColor::Subprogram),
            ("x", NameColor::Variable),
            ("integer", NameColor::Type),
            ("n", NameColor::Signal),
        ] {
            assert!(colors.contains(&expected), "{expected:?} in {colors:?}");
        }
        // Units and labels keep the lexer's colors.
        assert!(!colors.iter().any(|(name, _)| *name == "e" || *name == "a"));
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
        let labels: Vec<&str> = completion_list(&completions)
            .iter()
            .map(|c| c.label.as_str())
            .collect();
        assert_eq!(labels, ["count", "valid"]);

        // Elsewhere: what is visible, and keywords.
        let offset = at(TYPES, "n <=");
        let completions = engine.completion(None, TYPES, offset).unwrap();
        let labels: Vec<&str> = completion_list(&completions)
            .iter()
            .map(|c| c.label.as_str())
            .collect();
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

    /// A project with a package and a top level using it.
    fn project() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        std::fs::write(root.join("vfl.toml"), "").unwrap();
        let pkg = root.join("pkg.vhd");
        std::fs::write(
            &pkg,
            "package pkg is\n  constant width : integer := 8;\nend package;\n",
        )
        .unwrap();
        let top = root.join("top.vhd");
        std::fs::write(&top, TOP).unwrap();
        (dir, pkg, top)
    }

    const TOP: &str = "\
use work.pkg.all;
entity top is
end entity;
architecture a of top is
  signal n : integer;
  signal m : integer;
begin
  n <= width;
  m <= n + width;
end architecture;
";

    #[test]
    fn goes_to_definitions_and_references() {
        let (_dir, pkg, top) = project();
        let mut engine = Engine::default();
        let found = engine.definition(Some(&top), TOP, at(TOP, "width;"));
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].path, pkg);
        assert_eq!(found[0].line, 2);
        assert_eq!(found[0].line_text, "constant width : integer := 8;");

        let refs = engine.references(Some(&top), TOP, at(TOP, "width;"));
        let places: Vec<(&Path, u32)> = refs.iter().map(|l| (l.path.as_path(), l.line)).collect();
        assert_eq!(
            places,
            [(pkg.as_path(), 2), (top.as_path(), 8), (top.as_path(), 9)]
        );

        // `n` is written on line 8 and read on line 9.
        let uses = engine.highlights(Some(&top), TOP, at(TOP, "n <="));
        let kinds: Vec<Use> = uses.iter().map(|(_, u)| *u).collect();
        assert_eq!(kinds, [Use::Other, Use::Write, Use::Read]);
        assert!(uses.iter().all(|(r, _)| &TOP[r.clone()] == "n"));
    }

    #[test]
    fn lists_the_problems_of_the_project() {
        let (_dir, pkg, top) = project();
        let other = top.with_file_name("other.vhd");
        std::fs::write(&other, "entity other is\nend entity;\narchitecture a of other is\n  signal s : bit;\nbegin\nend;\n").unwrap();
        let mut engine = Engine::default();
        // The edited buffer counts, not the file on disk.
        let text = TOP.replace("m <= n + width;", "m <= n + wid;");
        let problems = engine.project_problems(Some(&top), &text);
        let found: Vec<(&Path, u32, &str)> = problems
            .iter()
            .map(|p| (p.location.path.as_path(), p.location.line, p.rule))
            .collect();
        assert!(
            found.contains(&(other.as_path(), 4, "sema/unused-signal")),
            "{found:?}"
        );
        assert!(
            found
                .iter()
                .any(|(path, line, _)| *path == top.as_path() && *line == 9),
            "{found:?}"
        );
        assert!(
            !found.iter().any(|(path, _, _)| *path == pkg.as_path()),
            "{found:?}"
        );
        assert!(
            found
                .windows(2)
                .all(|w| (w[0].0, w[0].1) <= (w[1].0, w[1].1))
        );
    }

    #[test]
    fn lists_symbols() {
        let (_dir, _pkg, top) = project();
        let mut engine = Engine::default();
        let outline: Vec<(String, usize)> = engine
            .outline(Some(&top), TOP)
            .into_iter()
            .map(|s| (s.name, s.depth))
            .collect();
        assert_eq!(
            outline,
            [
                ("top".to_owned(), 0),
                ("a".to_owned(), 0),
                ("n".to_owned(), 1),
                ("m".to_owned(), 1)
            ]
        );
        let found = engine.project_symbols(Some(&top), TOP, "WID");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "width");
        assert_eq!(found[0].container.as_deref(), Some("pkg"));
    }

    #[test]
    fn generates_from_entities() {
        let (_dir, _pkg, top) = project();
        let mut engine = Engine::default();
        let found = engine.entities(Some(&top), TOP);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].qualified(), "work.top");
        let instance = engine
            .generate(Some(&top), TOP, Kind::Instance, "top")
            .unwrap();
        assert_eq!(instance.text, "u_top : entity work.top;\n");
        assert_eq!(instance.path, None);
        let testbench = engine
            .generate(Some(&top), TOP, Kind::Testbench, "work.top")
            .unwrap();
        assert!(testbench.text.contains("dut : entity work.top;"));
        assert_eq!(testbench.path, Some(top.with_file_name("top_tb.vhd")));
        assert!(
            engine
                .generate(Some(&top), TOP, Kind::Component, "nope")
                .is_err()
        );
    }

    #[test]
    fn renames_across_files() {
        let (_dir, pkg, top) = project();
        let mut engine = Engine::default();
        let offset = at(TOP, "width;");
        assert_eq!(
            engine.prepare_rename(Some(&top), TOP, offset),
            Ok(offset..offset + 5)
        );
        let edits = engine.rename(Some(&top), TOP, offset, "bits").unwrap();
        let files: Vec<(&Path, usize)> = edits
            .iter()
            .map(|e| (e.path.as_path(), e.edits.len()))
            .collect();
        assert_eq!(files, [(pkg.as_path(), 1), (top.as_path(), 2)]);
        assert!(engine.rename(Some(&top), TOP, offset, "2bits").is_err());
        assert!(
            engine
                .prepare_rename(Some(&top), TOP, at(TOP, "integer"))
                .is_err()
        );
    }

    #[test]
    fn fixes_problems() {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        std::fs::write(
            root.join("vfl.toml"),
            "[lint.rules]\n\"style/trailing-whitespace\" = \"warn\"\n",
        )
        .unwrap();
        let path = root.join("e.vhd");
        let text = "entity e is   \nend entity;\n";
        std::fs::write(&path, text).unwrap();
        let mut engine = Engine::default();
        let all = engine.quick_fixes(Some(&path), text, 11, 11);
        let fixes: Vec<_> = all.iter().filter(|f| !f.silences).collect();
        assert_eq!(fixes.len(), 1, "{fixes:?}");
        assert_eq!(
            all.iter()
                .filter(|f| f.silences)
                .map(|f| f.title.as_str())
                .collect::<Vec<_>>(),
            [
                "Ignore `style/trailing-whitespace` on this line",
                "Ignore `style/trailing-whitespace` in this file"
            ]
        );
        assert_eq!(fixes[0].title, "Remove trailing whitespace");
        assert_eq!(fixes[0].edits, [(11..14, String::new())]);
        let (fixed, count) = engine.fix_all(Some(&path), text).expect("a fix");
        assert_eq!((fixed.as_str(), count), ("entity e is\nend entity;\n", 1));
        assert!(engine.fix_all(Some(&path), &fixed).is_none());
    }

    #[test]
    fn notices_changes_on_disk() {
        let (_dir, pkg, top) = project();
        let mut engine = Engine::default();
        let unresolved = |e: &mut Engine| {
            e.lint(Some(&top), TOP)
                .findings
                .iter()
                .any(|f| f.message.contains("`width`"))
        };
        assert!(!unresolved(&mut engine));
        assert!(!engine.rescan());
        // Make sure the modification time moves on coarse file systems.
        std::thread::sleep(std::time::Duration::from_millis(1100));
        std::fs::write(&pkg, "package pkg is\nend package;\n").unwrap();
        assert!(engine.rescan());
        assert!(unresolved(&mut engine));
        assert_eq!(
            engine.analyzed_text(&pkg),
            Some("package pkg is\nend package;\n")
        );
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

//! Configuration schema and loading for vhdl-fmt-lint.
//!
//! Configuration lives in `vfl.toml` (or `.vfl.toml`). Every key has a
//! default, so a missing or empty file is valid. For a given directory the
//! effective configuration is built by layering every config file from the
//! filesystem root down to that directory, nearest last, so nested configs
//! only need the keys they change. A file may also `extends` another file.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use vfl_syntax::Standard;

mod layering;

pub use layering::{
    FileError, check_file, config_chain, find_config, load_for_dir, load_for_dir_checked,
    load_for_dir_or, load_layered,
};

/// File names recognised as configuration, in priority order.
pub const CONFIG_FILE_NAMES: [&str; 2] = ["vfl.toml", ".vfl.toml"];

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("cannot read {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("invalid config {path}: {source}")]
    Parse {
        path: PathBuf,
        source: Box<toml::de::Error>,
    },
    #[error("invalid config: {0}")]
    Invalid(Box<toml::de::Error>),
    #[error("config {path} extends itself")]
    ExtendsCycle { path: PathBuf },
}

/// One config file as written: the schema of [`Config`], with the keys
/// that control loading. For checking a file on its own, where its text
/// gives each error a place.
// Only deserialized, to check the file: its sections are never read.
#[allow(dead_code)]
#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct ConfigFile {
    project: ProjectConfig,
    format: FormatConfig,
    lint: LintConfig,
    doc: DocConfig,
    generate: GenerateConfig,
    verilog: VerilogConfig,
    extends: Option<String>,
    pub root: bool,
}

impl Default for ConfigFile {
    fn default() -> Self {
        // Every field of `Config`: one added there must be added here.
        let Config {
            project,
            format,
            lint,
            doc,
            generate,
            verilog,
        } = Config::default();
        ConfigFile {
            project,
            format,
            lint,
            doc,
            generate,
            verilog,
            extends: None,
            root: false,
        }
    }
}

/// The effective configuration for a file or directory.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub project: ProjectConfig,
    pub format: FormatConfig,
    pub lint: LintConfig,
    pub doc: DocConfig,
    pub generate: GenerateConfig,
    pub verilog: VerilogConfig,
}

impl Config {
    /// Parses a single config file's text, without layering.
    pub fn from_toml_str(text: &str) -> Result<Self, ConfigError> {
        let table: toml::Table = text
            .parse()
            .map_err(|e| ConfigError::Invalid(Box::new(e)))?;
        Config::try_from(table).map_err(|e| ConfigError::Invalid(Box::new(e)))
    }

    /// Reads and parses a single config file, without layering.
    pub fn from_file(path: &Path) -> Result<Self, ConfigError> {
        let text = fs::read_to_string(path).map_err(|source| ConfigError::Io {
            path: path.to_owned(),
            source,
        })?;
        let parse_error = |e| ConfigError::Parse {
            path: path.to_owned(),
            source: Box::new(e),
        };
        let table: toml::Table = text.parse().map_err(parse_error)?;
        Config::try_from(table).map_err(parse_error)
    }

    /// The text of a config file setting every key to its value in `self`,
    /// under `comment` (each line becomes a `#` comment).
    pub fn file_text(&self, comment: &str) -> String {
        let mut text: String = comment
            .lines()
            .map(|l| match l {
                "" => "#\n".to_owned(),
                l => format!("# {l}\n"),
            })
            .collect();
        if !text.is_empty() {
            text.push('\n');
        }
        // The defaults of `self`'s format style: the keys it sets itself
        // are at their default too.
        let defaults = Config {
            format: FormatConfig {
                style: Style::Default,
                ..FormatConfig::preset(self.format.style)
            },
            ..Config::default()
        };
        // A `Config` always serializes, to a table; one value per line,
        // in the order of the schema.
        let defaults = match toml::Value::try_from(&defaults) {
            Ok(toml::Value::Table(table)) => table,
            _ => toml::Table::new(),
        };
        let serialized = toml::to_string(self).unwrap_or_default();
        text.push_str(&comment_defaults(&serialized, &defaults));
        text
    }

    /// The configured standard as the parser's type.
    pub fn standard(&self) -> Standard {
        self.project.standard.into()
    }
}

/// Comments out the `key = value` lines of `text`, a serialized config,
/// whose value is the same in `defaults`.
fn comment_defaults(text: &str, defaults: &toml::Table) -> String {
    let mut out = String::new();
    // The table of the current `[section]` in `defaults`.
    let mut section = Some(defaults);
    for line in text.lines() {
        // A header or a key line is a TOML document of its own.
        let parsed: Option<toml::Table> = toml::from_str(line).ok();
        if line.starts_with('[') {
            section = Some(defaults);
            let mut header = parsed.as_ref();
            while let Some((key, value)) = header.and_then(|h| h.iter().next()) {
                section = section
                    .and_then(|s| s.get(key))
                    .and_then(toml::Value::as_table);
                header = value.as_table().filter(|t| !t.is_empty());
            }
        } else if let Some((key, value)) = parsed.as_ref().and_then(|p| p.iter().next())
            && section.and_then(|s| s.get(key)) == Some(value)
        {
            out.push_str("# ");
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// `[project]`: what the project is made of.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ProjectConfig {
    pub standard: StandardSetting,
    /// File extensions treated as VHDL sources.
    pub extensions: Vec<String>,
    /// Gitignore-style globs, relative to the project root, to skip.
    pub exclude: Vec<String>,
    /// Explicit library assignments: library name to file globs. These
    /// override library inference and are only needed when it guesses wrong.
    pub libraries: BTreeMap<String, Vec<String>>,
    /// How rust_hdl's `vhdl_ls.toml` library lists are used.
    #[serde(rename = "vhdl-ls")]
    pub vhdl_ls: VhdlLsMode,
    /// The size above which the language server analyzes only a file's
    /// entities and packages, and does not lint it: generated netlists.
    #[serde(rename = "max-file-size")]
    pub max_file_size: FileSize,
    /// Whether the bundled declarations of vendor primitive libraries
    /// (`unisim`, `xpm`, `altera_mf`, Lattice's) are loaded for a library
    /// the design uses and the project does not hold.
    #[serde(rename = "vendor-stubs")]
    pub vendor_stubs: bool,
}

impl Default for ProjectConfig {
    fn default() -> Self {
        ProjectConfig {
            standard: StandardSetting::default(),
            extensions: vec!["vhd".into(), "vhdl".into()],
            exclude: Vec::new(),
            libraries: BTreeMap::new(),
            vhdl_ls: VhdlLsMode::default(),
            max_file_size: FileSize::DEFAULT_MAX,
            vendor_stubs: true,
        }
    }
}

/// A size in bytes, written as a number of bytes or as a string with a
/// `KB` or `MB` unit (of 1024 bytes and 1024 KB): `"2MB"`. 0 means no
/// limit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileSize(pub u64);

impl FileSize {
    pub const DEFAULT_MAX: FileSize = FileSize(2 * 1024 * 1024);

    /// Whether `len` bytes are over this limit.
    pub fn exceeded_by(self, len: usize) -> bool {
        self.0 != 0 && len as u64 > self.0
    }
}

impl std::fmt::Display for FileSize {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        const KB: u64 = 1024;
        match self.0 {
            0 => write!(f, "0"),
            n if n % (KB * KB) == 0 => write!(f, "{}MB", n / (KB * KB)),
            n if n % KB == 0 => write!(f, "{}KB", n / KB),
            n => write!(f, "{n} bytes"),
        }
    }
}

impl Serialize for FileSize {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self.to_string() {
            text if text.ends_with('B') => serializer.serialize_str(&text),
            _ => serializer.serialize_u64(self.0),
        }
    }
}

impl<'de> Deserialize<'de> for FileSize {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Written {
            Bytes(u64),
            Text(String),
        }
        let invalid = |text: &str| {
            serde::de::Error::custom(format!(
                "invalid size `{text}`: expected a number of bytes or a string such as \"2MB\" or \"512KB\""
            ))
        };
        match Written::deserialize(deserializer)? {
            Written::Bytes(n) => Ok(FileSize(n)),
            Written::Text(text) => {
                let upper = text.trim().to_ascii_uppercase();
                let (digits, unit) = match upper.strip_suffix("MB") {
                    Some(digits) => (digits, 1024 * 1024),
                    None => match upper.strip_suffix("KB") {
                        Some(digits) => (digits, 1024),
                        None => (upper.strip_suffix('B').unwrap_or(&upper), 1),
                    },
                };
                let n: u64 = digits.trim().parse().map_err(|_| invalid(&text))?;
                n.checked_mul(unit)
                    .map(FileSize)
                    .ok_or_else(|| invalid(&text))
            }
        }
    }
}

/// The `[project]` `vhdl-ls` key: how a `vhdl_ls.toml` in the project root
/// takes part in placing files in libraries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VhdlLsMode {
    /// Only for the files library inference places nowhere.
    #[default]
    Fallback,
    /// Over inference, as `[project.libraries]` entries are.
    Prefer,
    /// Not read.
    Off,
}

/// The `standard` key, spelled as in config files.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum StandardSetting {
    #[serde(rename = "93", alias = "1993")]
    Vhdl1993,
    #[default]
    #[serde(rename = "2008", alias = "08")]
    Vhdl2008,
    #[serde(rename = "2019", alias = "19")]
    Vhdl2019,
}

impl From<StandardSetting> for Standard {
    fn from(s: StandardSetting) -> Self {
        match s {
            StandardSetting::Vhdl1993 => Standard::Vhdl1993,
            StandardSetting::Vhdl2008 => Standard::Vhdl2008,
            StandardSetting::Vhdl2019 => Standard::Vhdl2019,
        }
    }
}

/// `[verilog]`: Verilog and SystemVerilog sources.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct VerilogConfig {
    /// How much of vfl works on Verilog and SystemVerilog files.
    pub mode: VerilogMode,
    /// File extensions of Verilog sources, parsed with `standard`.
    pub extensions: Vec<String>,
    /// The Verilog standard (keyword set) of files with `extensions`.
    pub standard: VerilogStandard,
    /// File extensions of SystemVerilog sources, parsed with `sv_standard`.
    pub sv_extensions: Vec<String>,
    /// The SystemVerilog standard of files with `sv_extensions`.
    pub sv_standard: VerilogStandard,
    /// Directories searched for `` `include `` files, relative to the
    /// project root, after the including file's directory.
    pub include_dirs: Vec<String>,
    /// Macros defined before every file, as `NAME` or `NAME=value`.
    pub defines: Vec<String>,
}

impl Default for VerilogConfig {
    fn default() -> Self {
        VerilogConfig {
            mode: VerilogMode::Names,
            extensions: vec!["v".into(), "vh".into()],
            standard: VerilogStandard::Verilog2005,
            sv_extensions: vec!["sv".into(), "svh".into()],
            sv_standard: VerilogStandard::Sv2017,
            include_dirs: Vec::new(),
            defines: Vec::new(),
        }
    }
}

impl VerilogConfig {
    /// What vfl says wherever it works on Verilog files in
    /// [`VerilogMode::Full`].
    pub const IN_PROGRESS: &'static str = "Verilog and SystemVerilog support is in progress: \
        expect gaps and false findings in their files ([verilog] mode = \"full\")";

    /// Whether Verilog files are read at all, for the modules VHDL
    /// instantiates.
    pub fn loaded(&self) -> bool {
        self.mode != VerilogMode::Off
    }

    /// Whether Verilog files are checked, formatted and linted.
    pub fn full(&self) -> bool {
        self.mode == VerilogMode::Full
    }

    /// The standard to parse a file with this extension with, if it is a
    /// Verilog or SystemVerilog source.
    pub fn standard_for_extension(&self, ext: &str) -> Option<VerilogStandard> {
        let has = |list: &[String]| list.iter().any(|e| e.eq_ignore_ascii_case(ext));
        if has(&self.sv_extensions) {
            Some(self.sv_standard)
        } else if has(&self.extensions) {
            Some(self.standard)
        } else {
            None
        }
    }
}

/// The `[verilog]` `mode`: how much of vfl works on Verilog and
/// SystemVerilog files.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VerilogMode {
    /// Verilog files are ignored.
    Off,
    /// Verilog files are read only for the modules VHDL instantiates:
    /// names resolve, nothing in the Verilog files is reported or formatted.
    #[default]
    Names,
    /// Everything: checking, formatting and linting Verilog files too.
    Full,
}

/// A Verilog or SystemVerilog standard, spelled as `` `begin_keywords ``
/// spells it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum VerilogStandard {
    #[serde(rename = "1364-1995", alias = "1995")]
    Verilog1995,
    #[serde(rename = "1364-2001", alias = "2001")]
    Verilog2001,
    #[serde(rename = "1364-2005", alias = "2005")]
    Verilog2005,
    #[serde(rename = "1800-2005", alias = "sv2005")]
    Sv2005,
    #[serde(rename = "1800-2009", alias = "sv2009")]
    Sv2009,
    #[serde(rename = "1800-2012", alias = "sv2012")]
    Sv2012,
    #[serde(rename = "1800-2017", alias = "sv2017")]
    Sv2017,
    #[serde(rename = "1800-2023", alias = "sv2023")]
    Sv2023,
}

impl VerilogStandard {
    /// The name used in config files, which the parser's `Standard` also parses.
    pub fn as_str(self) -> &'static str {
        match self {
            VerilogStandard::Verilog1995 => "1364-1995",
            VerilogStandard::Verilog2001 => "1364-2001",
            VerilogStandard::Verilog2005 => "1364-2005",
            VerilogStandard::Sv2005 => "1800-2005",
            VerilogStandard::Sv2009 => "1800-2009",
            VerilogStandard::Sv2012 => "1800-2012",
            VerilogStandard::Sv2017 => "1800-2017",
            VerilogStandard::Sv2023 => "1800-2023",
        }
    }
}

/// `[format]`: formatter settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct FormatConfig {
    /// A named set of defaults for the other keys; keys set explicitly
    /// override it. See [`Style`].
    pub style: Style,
    /// Lines longer than this get their lists broken up, one element per line.
    pub max_width: u32,
    /// Columns per indentation level (also the tab width with `use_tabs`).
    pub indent: u32,
    /// Columns added for a statement continued on the next line, outside
    /// parentheses. Defaults to `indent`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub continuation_indent: Option<u32>,
    /// Indent with tabs instead of spaces. Alignment always uses spaces.
    pub use_tabs: bool,
    pub keyword_case: Case,
    /// Case of basic identifiers. Extended identifiers (`\Foo\`) never change.
    pub identifier_case: Case,
    pub end_label: EndLabel,
    /// Whether `end` repeats the optional construct keyword
    /// (`end entity`, `end architecture`, `end function`, ...).
    pub end_keyword: EndKeyword,
    /// `lbl : process` rather than `lbl: process`.
    pub space_before_label_colon: bool,
    /// A space between `(` and the keyword before it: `process (clk)`,
    /// `port map (`, `if (` rather than `process(clk)`. Applies after
    /// `process`, `port`, `generic`, `map`, `parameter`, `array`, `block`,
    /// `if`, `elsif`, `while` and `until`. Calls and indexing never get a
    /// space: `f(x)`, `v(7 downto 0)`.
    pub space_after_keyword: bool,
    /// Longer runs of blank lines are shortened to this.
    pub max_blank_lines: u32,
    /// Layout of `generic (...)` and `port (...)` clauses.
    pub interface_list: ListLayout,
    /// Layout of `generic map (...)` and `port map (...)`.
    pub association_list: ListLayout,
    /// Where `)` goes when a list is laid out one element per line.
    pub close_paren: CloseParen,
    /// Where the `;` between the elements of `generic (...)`, `port (...)`
    /// and parameter lists goes when they are on separate lines.
    pub interface_separator: Separator,
    /// Where the `,` between the elements of `generic map (...)`,
    /// `port map (...)`, calls, aggregates and enumerations goes when they
    /// are on separate lines.
    pub association_separator: Separator,
    /// Whether `when x => stmt;` may keep its statement on the `when` line.
    pub case_alternative: CaseAlternative,
    /// Whether a line still longer than `max_width` once its lists are
    /// broken up is broken inside its expressions, at the operators that
    /// bind loosest.
    pub break_expressions: bool,
    /// Where the operator goes when an expression is broken over lines.
    pub operator_position: OperatorPosition,
    pub line_ending: LineEnding,
    /// Whether editors may format the line just typed (after `;` or a new
    /// line). Editors have their own switch too.
    pub on_type: bool,
    pub align: AlignConfig,
    /// The case of the identifiers declared as each kind of declaration,
    /// and of every reference to them in the same file. Kinds left out
    /// follow `identifier_case`.
    pub declaration_case: DeclarationCase,
    /// Blank lines around `begin`, `end`, processes and declaration groups.
    pub blank_lines: BlankLinesConfig,
}

impl Default for FormatConfig {
    fn default() -> Self {
        FormatConfig {
            style: Style::Default,
            max_width: 100,
            indent: 4,
            continuation_indent: None,
            use_tabs: false,
            keyword_case: Case::Lower,
            identifier_case: Case::Preserve,
            end_label: EndLabel::Preserve,
            end_keyword: EndKeyword::Preserve,
            space_before_label_colon: true,
            space_after_keyword: true,
            max_blank_lines: 2,
            interface_list: ListLayout::Expand,
            association_list: ListLayout::Auto,
            close_paren: CloseParen::OwnLine,
            interface_separator: Separator::Trailing,
            association_separator: Separator::Trailing,
            case_alternative: CaseAlternative::Preserve,
            break_expressions: true,
            operator_position: OperatorPosition::Preserve,
            line_ending: LineEnding::Auto,
            on_type: true,
            align: AlignConfig::default(),
            declaration_case: DeclarationCase::default(),
            blank_lines: BlankLinesConfig::default(),
        }
    }
}

impl FormatConfig {
    /// The settings of a [`Style`], before any key set explicitly.
    pub fn preset(style: Style) -> Self {
        let base = FormatConfig {
            style,
            ..FormatConfig::default()
        };
        match style {
            Style::Default => base,
            Style::LeadingComma => FormatConfig {
                association_list: ListLayout::Expand,
                interface_separator: Separator::Leading,
                association_separator: Separator::Leading,
                ..base
            },
            Style::Compact => FormatConfig {
                indent: 2,
                max_blank_lines: 1,
                interface_list: ListLayout::Auto,
                association_list: ListLayout::Auto,
                close_paren: CloseParen::Attached,
                align: AlignConfig {
                    port_colon: false,
                    port_mode: false,
                    generic_colon: false,
                    interface_default: false,
                    assignment: false,
                    association: false,
                    declaration: false,
                    case_choice: false,
                    when_clause: false,
                    conditional_continuation: false,
                    trailing_comment: true,
                    semicolon: false,
                    scope: AlignScope::Contiguous,
                    block_boundary: BlockBoundary::BlankLine,
                },
                ..base
            },
            Style::Classic => FormatConfig {
                indent: 4,
                keyword_case: Case::Upper,
                end_label: EndLabel::Add,
                end_keyword: EndKeyword::Add,
                association_list: ListLayout::Expand,
                case_alternative: CaseAlternative::Break,
                align: AlignConfig {
                    semicolon: false,
                    scope: AlignScope::Contiguous,
                    ..AlignConfig::default()
                },
                ..base
            },
        }
    }
}

/// A named set of `[format]` defaults, chosen with `style = "..."`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Style {
    /// The defaults of every key.
    Default,
    /// `,` and `;` start the next line instead of ending the line, and port
    /// and generic maps are laid out one element per line.
    LeadingComma,
    /// Lists stay on one line when they fit, `)` follows the last element,
    /// and only trailing comments are aligned.
    Compact,
    /// Upper case keywords, 4-column indentation, `end` repeating the
    /// construct keyword and name, maps one element per line.
    Classic,
}

impl Style {
    pub const ALL: [Style; 4] = [
        Style::Default,
        Style::LeadingComma,
        Style::Compact,
        Style::Classic,
    ];

    /// The name, as in config files.
    pub fn name(self) -> &'static str {
        match self {
            Style::Default => "default",
            Style::LeadingComma => "leading-comma",
            Style::Compact => "compact",
            Style::Classic => "classic",
        }
    }
}

/// Where the separator between the elements of a list goes when the
/// elements are on separate lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Separator {
    /// At the end of the line: `a => x,` then `b => y`.
    Trailing,
    /// At the start of the next line: `a => x` then `, b => y`. The
    /// separator sits left of the elements so that they line up.
    Leading,
}

/// Where the operator goes when an expression is broken over lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum OperatorPosition {
    /// At the end of the line: `a and` then `b`.
    Trailing,
    /// At the start of the next line: `a` then `and b`.
    Leading,
    /// Breaks written in the source stay on their side of the operator;
    /// breaks the formatter adds go after it.
    Preserve,
}

/// `[format.declaration_case]`: the case of the identifiers declared as
/// each kind, and of every reference to them in the same file. A kind left
/// out follows `identifier_case`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DeclarationCase {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entity: Option<Case>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub architecture: Option<Case>,
    /// Packages and package instantiations.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub package: Option<Case>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub component: Option<Case>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub generic: Option<Case>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port: Option<Case>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub constant: Option<Case>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signal: Option<Case>,
    /// Variables, shared variables included.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub variable: Option<Case>,
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub type_: Option<Case>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subtype: Option<Case>,
    /// The literals of enumeration types (`IDLE` in `(IDLE, RUN)`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enum_literal: Option<Case>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub function: Option<Case>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub procedure: Option<Case>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alias: Option<Case>,
    /// Statement labels: processes, instances, generate statements, blocks.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<Case>,
}

impl DeclarationCase {
    /// Whether no kind has a case of its own.
    pub fn is_empty(&self) -> bool {
        *self == DeclarationCase::default()
    }
}

/// `[format.blank_lines]`: how many blank lines go at some places. Each key
/// takes a number (exactly that many) or `{ min = .., max = .. }`; a key
/// left out keeps what is written, up to `max_blank_lines`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct BlankLinesConfig {
    /// Before the `begin` of an architecture, process, block, subprogram
    /// or generate statement.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub before_begin: Option<BlankLines>,
    /// After that `begin`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after_begin: Option<BlankLines>,
    /// Before the `end` of a construct (entity, architecture, process,
    /// `if`, `case`, loop, record...).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub before_end: Option<BlankLines>,
    /// Before a process, block or generate statement, and the comment
    /// lines just above it; not when it is the first statement after
    /// `begin`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub before_process: Option<BlankLines>,
    /// Between two declarations of different kinds (constants, then
    /// signals), above the comment lines just above the second.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub between_declaration_groups: Option<BlankLines>,
}

impl BlankLinesConfig {
    /// Whether no rule is set.
    pub fn is_empty(&self) -> bool {
        *self == BlankLinesConfig::default()
    }
}

/// A number of blank lines: exactly `n`, or a range.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum BlankLines {
    Exactly(u32),
    Range {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        min: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max: Option<u32>,
    },
}

impl BlankLines {
    /// `count` brought into the range.
    pub fn apply(self, count: u32) -> u32 {
        match self {
            BlankLines::Exactly(n) => n,
            BlankLines::Range { min, max } => {
                let count = count.max(min.unwrap_or(0));
                max.map_or(count, |max| count.min(max))
            }
        }
    }
}

/// Replaces `format.style` in a config table by the settings of that style,
/// under the `[format]` keys the table sets itself.
fn expand_style(table: &mut toml::Table) {
    let Some(toml::Value::Table(format)) = table.get_mut("format") else {
        return;
    };
    let Some(style) = format
        .get("style")
        .cloned()
        .and_then(|s| Style::deserialize(s).ok())
    else {
        // No style, or an invalid one, which the schema reports.
        return;
    };
    if style == Style::Default {
        return;
    }
    let Ok(toml::Value::Table(mut base)) = toml::Value::try_from(FormatConfig::preset(style))
    else {
        return;
    };
    merge_tables(&mut base, std::mem::take(format));
    *format = base;
}

/// Deep-merges `overlay` into `base`: tables merge key by key, any other
/// value (including arrays) replaces what was there.
pub(crate) fn merge_tables(base: &mut toml::Table, overlay: toml::Table) {
    for (key, value) in overlay {
        match (base.get_mut(&key), value) {
            (Some(toml::Value::Table(base_table)), toml::Value::Table(overlay_table)) => {
                merge_tables(base_table, overlay_table);
            }
            (_, value) => {
                base.insert(key, value);
            }
        }
    }
}

impl TryFrom<toml::Table> for Config {
    type Error = toml::de::Error;

    /// Applies the schema to a parsed config file (or several merged).
    fn try_from(mut table: toml::Table) -> Result<Self, Self::Error> {
        expand_style(&mut table);
        table.try_into()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Case {
    Lower,
    Upper,
    Preserve,
}

/// Whether `end` keywords repeat the construct's label or name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EndLabel {
    Add,
    Remove,
    Preserve,
}

/// Whether `end` repeats an optional construct keyword.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EndKeyword {
    Add,
    Remove,
    Preserve,
}

/// How a parenthesized list is laid out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ListLayout {
    /// Always one element per line, with `(` ending the first line.
    Expand,
    /// One element per line if the list spans several lines or does not
    /// fit in `max_width`; otherwise kept on one line.
    Auto,
    /// Keep the line breaks as written; expand only if it does not fit.
    Preserve,
}

/// Where the closing `)` of an expanded list goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CloseParen {
    /// On its own line, at the indentation of the line that opened the list.
    OwnLine,
    /// Right after the last element.
    Attached,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CaseAlternative {
    /// Every statement of a case alternative starts its own line.
    Break,
    /// A first statement written on the `when` line stays there.
    Preserve,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LineEnding {
    /// Whatever the file uses already (LF for new files).
    Auto,
    Lf,
    Crlf,
}

/// `[format.align]`: which columns the formatter lines up, and across
/// which lines ([`AlignScope`]).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AlignConfig {
    /// `:` in port clauses and subprogram parameter lists.
    pub port_colon: bool,
    /// The subtype after the mode (`in`, `out`, ...) in port clauses and
    /// parameter lists.
    pub port_mode: bool,
    /// `:` in generic clauses.
    pub generic_colon: bool,
    /// `:=` default values in generic and port clauses.
    pub interface_default: bool,
    /// `<=` and `:=` of assignment statements.
    pub assignment: bool,
    /// `=>` in port maps, generic maps and aggregates.
    pub association: bool,
    /// `:` and `:=` in object declarations and record elements, and `is` in
    /// type and subtype declarations.
    pub declaration: bool,
    /// `=>` of case alternatives: all of them with `scope = "block"`, else
    /// those whose statement is on the same line.
    pub case_choice: bool,
    /// `when` in conditional and selected assignments.
    pub when_clause: bool,
    /// Continuation lines of conditional and selected assignments start
    /// under the first value instead of one indentation level in.
    pub conditional_continuation: bool,
    pub trailing_comment: bool,
    /// The `;` ending a line whose statement, declaration or interface
    /// element has another aligned token.
    pub semicolon: bool,
    /// Which lines align together. See [`AlignScope`].
    pub scope: AlignScope,
    /// What ends a run of aligned lines with `scope = "contiguous"`.
    pub block_boundary: BlockBoundary,
}

impl Default for AlignConfig {
    fn default() -> Self {
        AlignConfig {
            port_colon: true,
            port_mode: true,
            generic_colon: true,
            interface_default: true,
            assignment: true,
            association: true,
            declaration: true,
            case_choice: true,
            when_clause: true,
            conditional_continuation: true,
            trailing_comment: true,
            semicolon: true,
            scope: AlignScope::Block,
            block_boundary: BlockBoundary::BlankLine,
        }
    }
}

/// Which lines align their tokens together.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AlignScope {
    /// Every line of the same enclosing block: a process, an architecture,
    /// a block or generate body, a subprogram, a package, an entity, a
    /// record, an interface list. Lines in between, blank lines and
    /// comments do not end the alignment; `block_boundary` is ignored.
    Block,
    /// Only consecutive lines of the same construct, up to the
    /// `block_boundary`.
    Contiguous,
}

/// What ends an alignment block.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BlockBoundary {
    /// A blank line starts a new block.
    BlankLine,
    /// A blank line or a full-line comment starts a new block.
    Comment,
    /// Blank lines and comments do not end a block; any other line without
    /// the aligned token does.
    Never,
}

/// `[generate]`: code generated from an entity (`vfl new`, and the
/// editor commands): instantiations, component declarations and
/// testbenches. The text is then laid out by the formatter.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct GenerateConfig {
    /// The label of an instance; `{entity}` is replaced by the entity name.
    pub instance_label: String,
    /// The testbench entity name; `{entity}` is replaced by the entity name.
    pub testbench_name: String,
    /// The name of the testbench architecture.
    pub testbench_architecture: String,
    /// The label of the design under test in a testbench.
    pub testbench_instance: String,
    /// The period of the generated clocks, a VHDL time expression.
    pub clock_period: String,
    /// Clock cycles the reset is held active at the start of a testbench.
    pub reset_cycles: u32,
    /// Regular expression for port names taken as clocks, when the
    /// architecture does not show it (`rising_edge(clk)`).
    pub clock_pattern: String,
    /// Regular expression for port names taken as resets.
    pub reset_pattern: String,
    /// Regular expression for reset names that are active low, when the
    /// architecture does not show it (`rst = '0'`).
    pub active_low_pattern: String,
}

impl Default for GenerateConfig {
    fn default() -> Self {
        GenerateConfig {
            instance_label: "u_{entity}".into(),
            testbench_name: "{entity}_tb".into(),
            testbench_architecture: "sim".into(),
            testbench_instance: "dut".into(),
            clock_period: "10 ns".into(),
            reset_cycles: 5,
            clock_pattern: r"(?i)^(.*_)?(clk|clock)\d*(_i|_in|_p)?$".into(),
            reset_pattern: r"(?i)^(.*_)?(a|s)?(rst|reset)\d*(n|_n|_b|_i|_in|_ni|_n_i)?$".into(),
            active_low_pattern: r"(?i)([^i]n|_b|_ni|_n_i)$".into(),
        }
    }
}

/// `[lint]`: linter settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LintConfig {
    pub preset: Preset,
    /// Per-rule overrides, keyed by rule id such as `naming/signal-prefix`.
    pub rules: BTreeMap<String, RuleSetting>,
    /// The baseline file of `vfl lint`, relative to the project root:
    /// findings it lists are not reported.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub baseline: Option<String>,
    /// `[[lint.waiver]]`: findings silenced from outside the sources.
    #[serde(rename = "waiver", skip_serializing_if = "Vec::is_empty")]
    pub waivers: Vec<Waiver>,
}

impl Default for LintConfig {
    fn default() -> Self {
        LintConfig {
            preset: Preset::Recommended,
            rules: BTreeMap::new(),
            baseline: None,
            waivers: Vec::new(),
        }
    }
}

/// A `[[lint.waiver]]`: silences the findings that match every key it
/// sets.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Waiver {
    /// Gitignore-style globs, relative to the project root, of the files
    /// (all files if empty).
    pub files: Vec<String>,
    /// Rule ids or categories (all rules if empty).
    pub rules: Vec<String>,
    /// One-based line numbers (`12`) or inclusive ranges (`"40:52"`) the
    /// finding starts on (any line if empty).
    pub lines: Vec<WaiverLines>,
    /// A regular expression the message must contain a match of.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// Why the findings are waived; not used by vfl.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// A line or a range of lines of a [`Waiver`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum WaiverLines {
    Line(u32),
    /// `"FIRST:LAST"`, or a single line as a string.
    Range(String),
}

impl WaiverLines {
    /// The one-based inclusive range, or an error for a bad range.
    pub fn range(&self) -> Result<(u32, u32), String> {
        let bad = || format!("invalid waiver line range `{self}`: expected `LINE` or `FIRST:LAST`");
        match self {
            WaiverLines::Line(line) => Ok((*line, *line)),
            WaiverLines::Range(text) => {
                let (first, last) = text.split_once(':').unwrap_or((text, text));
                let first: u32 = first.trim().parse().map_err(|_| bad())?;
                let last: u32 = last.trim().parse().map_err(|_| bad())?;
                if first == 0 || first > last {
                    return Err(bad());
                }
                Ok((first, last))
            }
        }
    }
}

impl std::fmt::Display for WaiverLines {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WaiverLines::Line(line) => write!(f, "{line}"),
            WaiverLines::Range(text) => f.write_str(text),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Preset {
    None,
    Recommended,
    Strict,
}

impl Preset {
    pub const ALL: [Preset; 3] = [Preset::None, Preset::Recommended, Preset::Strict];

    /// The name, as in config files.
    pub fn name(self) -> &'static str {
        match self {
            Preset::None => "none",
            Preset::Recommended => "recommended",
            Preset::Strict => "strict",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Level {
    Off,
    Info,
    Warn,
    Error,
}

/// A rule override: either just a level (`"warn"`), or a table with a level
/// and rule-specific options (`{ level = "warn", prefix = "s_" }`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RuleSetting {
    Level(Level),
    Detailed {
        level: Option<Level>,
        #[serde(flatten)]
        options: toml::Table,
    },
}

impl RuleSetting {
    pub fn level(&self) -> Option<Level> {
        match self {
            RuleSetting::Level(level) => Some(*level),
            RuleSetting::Detailed { level, .. } => *level,
        }
    }
}

/// `[doc]`: documentation and diagram generation (`vfl doc`, `vfl schema`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DocConfig {
    /// Where `vfl doc` writes, relative to the project root.
    pub out_dir: String,
    pub format: DocFormat,
    /// The title of the index page. Defaults to the project directory's name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Which comments document the declaration next to them.
    pub comments: DocComments,
    /// What follows `--` in a marked doc comment (`--!`).
    pub comment_marker: String,
    /// Gitignore-style globs, relative to the project root, of files whose
    /// units are left out of the documentation.
    pub exclude: Vec<String>,
    /// Document entities without ports (testbenches).
    pub testbenches: bool,
    pub signals: bool,
    pub constants: bool,
    pub types: bool,
    pub subprograms: bool,
    pub processes: bool,
    pub instances: bool,
    /// Detect finite state machines and document their states and transitions.
    pub fsm: bool,
    pub diagrams: DiagramConfig,
}

impl Default for DocConfig {
    fn default() -> Self {
        DocConfig {
            out_dir: "doc".into(),
            format: DocFormat::Html,
            title: None,
            comments: DocComments::Auto,
            comment_marker: "!".into(),
            exclude: Vec::new(),
            testbenches: true,
            signals: true,
            constants: true,
            types: true,
            subprograms: true,
            processes: true,
            instances: true,
            fsm: true,
            diagrams: DiagramConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DocFormat {
    Html,
    Markdown,
    Asciidoc,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DocComments {
    /// Marked comments in files that have any, every adjacent comment in
    /// the others.
    Auto,
    /// Only comments starting with the marker.
    Marked,
    /// Every comment directly above or at the end of the declaration's line.
    All,
    /// No comments.
    None,
}

/// `[doc.diagrams]`: which diagrams the documentation embeds.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DiagramConfig {
    /// A block symbol of each entity, with its generics and ports.
    pub symbol: bool,
    /// The instantiation tree of the design.
    pub hierarchy: bool,
    /// A block diagram of each architecture with instances: the instances
    /// wired by the signals between their ports.
    pub block: bool,
    /// On the index, the block diagram of each top-level entity (for a
    /// testbench, of the design it tests).
    pub top: bool,
    /// A diagram per instance: the instantiated block with the actual of
    /// each port and generic.
    pub port_map: bool,
    /// The data flow of each architecture with processes or concurrent
    /// assignments: those and its instances, wired by signal.
    pub dataflow: bool,
    /// The clock domains of each architecture with several clocks, with
    /// the signals that cross between them.
    pub clocks: bool,
    /// On the index, the tree of each reset of the top level down to the
    /// processes it resets.
    pub resets: bool,
    /// Which units use which packages.
    pub dependencies: bool,
    /// WaveDrom timing diagrams written in doc comments.
    pub waveforms: bool,
    /// Bit-field layouts of record types and of aliases of slices.
    pub fields: bool,
    /// A state diagram of each detected state machine.
    pub fsm: bool,
    /// Show port types next to the pins of a symbol.
    pub port_types: bool,
    /// Show generics in a symbol.
    pub generics: bool,
    /// How Markdown pages draw the hierarchy and state diagrams: SVG files,
    /// or Mermaid blocks that GitHub and GitLab render.
    pub markdown: MarkdownDiagrams,
}

impl Default for DiagramConfig {
    fn default() -> Self {
        DiagramConfig {
            symbol: true,
            hierarchy: true,
            block: true,
            top: true,
            port_map: true,
            dataflow: true,
            clocks: true,
            resets: true,
            dependencies: true,
            waveforms: true,
            fields: true,
            fsm: true,
            port_types: true,
            generics: true,
            markdown: MarkdownDiagrams::Svg,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MarkdownDiagrams {
    Svg,
    Mermaid,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_file_is_the_default_config() {
        assert_eq!(Config::from_toml_str("").unwrap(), Config::default());
    }

    #[test]
    fn design_doc_example_parses() {
        let config = Config::from_toml_str(
            r#"
            [project]
            standard = "2008"
            exclude  = ["sim/generated/**"]

            [project.libraries]
            osvvm = ["third_party/osvvm/**/*.vhd"]

            [format]
            max_width       = 100
            indent          = 2
            keyword_case    = "lower"
            identifier_case = "preserve"
            end_label       = "add"

            [format.align]
            port_colon       = true
            assignment       = false
            trailing_comment = true
            block_boundary   = "blank-line"

            [lint]
            preset = "recommended"

            [lint.rules]
            "naming/signal-prefix" = { level = "warn", prefix = "s_" }
            "style/no-buffer-ports" = "error"
            "sema/unused-signal" = "off"
            "#,
        )
        .unwrap();

        assert_eq!(config.standard(), Standard::Vhdl2008);
        assert_eq!(config.format.end_label, EndLabel::Add);
        assert!(!config.format.align.assignment);
        assert!(config.format.align.association);
        assert_eq!(config.project.libraries["osvvm"].len(), 1);

        let rules = &config.lint.rules;
        assert_eq!(rules["style/no-buffer-ports"].level(), Some(Level::Error));
        assert_eq!(rules["sema/unused-signal"].level(), Some(Level::Off));
        let RuleSetting::Detailed { level, options } = &rules["naming/signal-prefix"] else {
            panic!("expected a detailed rule setting");
        };
        assert_eq!(*level, Some(Level::Warn));
        assert_eq!(options["prefix"].as_str(), Some("s_"));
    }

    #[test]
    fn design_doc_config_example_parses() {
        // Git may check the doc out with CRLF line endings on Windows.
        let doc = include_str!("../../../docs/design.md").replace("\r\n", "\n");
        let section = &doc[doc.find("## Configuration").unwrap()..];
        let start = section.find("```toml\n").unwrap() + "```toml\n".len();
        let end = start + section[start..].find("```").unwrap();
        let config = Config::from_toml_str(&section[start..end]).unwrap();
        assert_eq!(config.format, FormatConfig::default());
    }

    #[test]
    fn standard_accepts_short_and_long_spellings() {
        for (text, std) in [
            ("93", Standard::Vhdl1993),
            ("1993", Standard::Vhdl1993),
            ("19", Standard::Vhdl2019),
        ] {
            let config = Config::from_toml_str(&format!("project.standard = \"{text}\"")).unwrap();
            assert_eq!(config.standard(), std);
        }
    }

    #[test]
    fn preset_and_style_names_are_the_config_spellings() {
        for preset in Preset::ALL {
            let config =
                Config::from_toml_str(&format!("lint.preset = \"{}\"", preset.name())).unwrap();
            assert_eq!(config.lint.preset, preset);
        }
        for style in Style::ALL {
            let config =
                Config::from_toml_str(&format!("format.style = \"{}\"", style.name())).unwrap();
            assert_eq!(config.format.style, style);
        }
    }

    #[test]
    fn file_sizes_take_units() {
        let size = |text: &str| {
            Config::from_toml_str(&format!("project.max-file-size = {text}"))
                .map(|c| c.project.max_file_size.0)
        };
        assert_eq!(size("1000").unwrap(), 1000);
        assert_eq!(size("\"2MB\"").unwrap(), 2 * 1024 * 1024);
        assert_eq!(size("\"512 kb\"").unwrap(), 512 * 1024);
        assert_eq!(size("0").unwrap(), 0);
        assert!(size("\"big\"").is_err());
        assert!(!FileSize(0).exceeded_by(usize::MAX));
        assert!(FileSize(10).exceeded_by(11));
        assert!(!FileSize(10).exceeded_by(10));
        for written in [
            FileSize(3 * 1024 * 1024),
            FileSize(5 * 1024),
            FileSize(1001),
        ] {
            let config = Config {
                project: ProjectConfig {
                    max_file_size: written,
                    ..ProjectConfig::default()
                },
                ..Config::default()
            };
            let text = toml::to_string(&config).unwrap();
            assert_eq!(Config::from_toml_str(&text).unwrap(), config);
        }
    }

    #[test]
    fn unknown_keys_are_rejected() {
        assert!(Config::from_toml_str("[format]\nindnet = 4").is_err());
    }

    #[test]
    fn default_config_round_trips_through_toml() {
        let text = toml::to_string(&Config::default()).unwrap();
        assert_eq!(Config::from_toml_str(&text).unwrap(), Config::default());
    }

    #[test]
    fn file_text_round_trips_with_a_style() {
        let config =
            Config::from_toml_str("[format]\nstyle = \"compact\"\n[lint]\npreset = \"none\"\n")
                .unwrap();
        let text = config.file_text("Generated.\n\nSecond paragraph.");
        assert!(
            text.starts_with("# Generated.\n#\n# Second paragraph.\n\n"),
            "{text}"
        );
        assert_eq!(Config::from_toml_str(&text).unwrap(), config);
        // Keys at their default, the style's included, are comments.
        assert!(text.contains("\n# max_width = 100\n"), "{text}");
        assert!(text.contains("\n# indent = 2\n"), "{text}");
        assert!(text.contains("\n# standard = \"2008\"\n"), "{text}");
        let active: Vec<&str> = text
            .lines()
            .filter(|l| !l.starts_with('#') && l.contains('='))
            .collect();
        assert_eq!(
            active,
            ["style = \"compact\"", "preset = \"none\""],
            "{text}"
        );

        let config = Config::from_toml_str(
            "[lint.rules]\n\"sema/unused-signal\" = \"off\"\n\"naming/signal\" = { level = \"warn\", prefix = \"s_\" }\n",
        )
        .unwrap();
        let text = config.file_text("");
        assert!(
            text.contains("\n\"sema/unused-signal\" = \"off\"\n"),
            "{text}"
        );
        assert_eq!(Config::from_toml_str(&text).unwrap(), config);
    }

    #[test]
    fn waivers_and_baseline_parse_and_round_trip() {
        let config = Config::from_toml_str(
            "[lint]\nbaseline = \"b.json\"\n\
             [[lint.waiver]]\nfiles = [\"ip/**\"]\nrules = [\"style\"]\nlines = [3, \"10:12\"]\n\
             message = \"x\"\nreason = \"vendor\"\n",
        )
        .unwrap();
        assert_eq!(config.lint.baseline.as_deref(), Some("b.json"));
        let waiver = &config.lint.waivers[0];
        assert_eq!(waiver.lines[0].range(), Ok((3, 3)));
        assert_eq!(waiver.lines[1].range(), Ok((10, 12)));
        assert!(WaiverLines::Range("4:2".into()).range().is_err());
        assert!(WaiverLines::Range("0".into()).range().is_err());
        let text = config.file_text("");
        assert_eq!(Config::from_toml_str(&text).unwrap(), config, "{text}");
        assert!(Config::from_toml_str("[[lint.waiver]]\nfile = \"a\"\n").is_err());
    }

    #[test]
    fn blank_line_rules_and_declaration_case_parse() {
        let config = Config::from_toml_str(
            "[format.blank_lines]\nafter_begin = 0\nbefore_process = { min = 1 }\n\
             before_end = { min = 1, max = 2 }\n\
             [format.declaration_case]\nconstant = \"upper\"\ntype = \"lower\"\n",
        )
        .unwrap();
        let blank = &config.format.blank_lines;
        assert_eq!(blank.after_begin.unwrap().apply(3), 0);
        assert_eq!(blank.before_process.unwrap().apply(0), 1);
        assert_eq!(blank.before_process.unwrap().apply(2), 2);
        assert_eq!(blank.before_end.unwrap().apply(5), 2);
        assert_eq!(blank.before_begin, None);
        assert_eq!(config.format.declaration_case.constant, Some(Case::Upper));
        assert_eq!(config.format.declaration_case.type_, Some(Case::Lower));
        let text = config.file_text("");
        assert_eq!(Config::from_toml_str(&text).unwrap(), config, "{text}");
        assert!(Config::from_toml_str("[format.blank_lines]\nafter_begin = \"x\"").is_err());
        assert!(Config::from_toml_str("[format.declaration_case]\nnope = \"upper\"").is_err());
    }

    #[test]
    fn every_key_is_documented() {
        // Git may check the doc out with CRLF line endings on Windows.
        let doc = include_str!("../../../docs/configuration.md").replace("\r\n", "\n");
        let mut config = Config::default();
        config.format.continuation_indent = Some(2);
        // Keys left out when unset.
        config.format.declaration_case = DeclarationCase {
            entity: Some(Case::Lower),
            architecture: Some(Case::Lower),
            package: Some(Case::Lower),
            component: Some(Case::Lower),
            generic: Some(Case::Upper),
            port: Some(Case::Lower),
            constant: Some(Case::Upper),
            signal: Some(Case::Lower),
            variable: Some(Case::Lower),
            type_: Some(Case::Lower),
            subtype: Some(Case::Lower),
            enum_literal: Some(Case::Upper),
            function: Some(Case::Lower),
            procedure: Some(Case::Lower),
            alias: Some(Case::Lower),
            label: Some(Case::Lower),
        };
        let one = Some(BlankLines::Exactly(1));
        config.format.blank_lines = BlankLinesConfig {
            before_begin: one,
            after_begin: one,
            before_end: one,
            before_process: one,
            between_declaration_groups: one,
        };
        config.doc.title = Some("Design".into());
        let toml::Value::Table(table) = toml::Value::try_from(&config).unwrap() else {
            panic!("the config is not a table");
        };
        fn walk(table: &toml::Table, path: &str, doc: &str) {
            for (key, value) in table {
                let section = if path.is_empty() {
                    format!("## `[{key}]`")
                } else {
                    format!("## `[{path}.{key}]`")
                };
                match value {
                    toml::Value::Table(inner) if !(path == "project" && key == "libraries") => {
                        assert!(
                            doc.contains(&section),
                            "docs/configuration.md has no {section}"
                        );
                        let path = if path.is_empty() {
                            key.clone()
                        } else {
                            format!("{path}.{key}")
                        };
                        walk(inner, &path, doc);
                    }
                    _ => assert!(
                        doc.contains(&format!("| `{key}` |")),
                        "docs/configuration.md does not describe {path}.{key}"
                    ),
                }
            }
        }
        walk(&table, "", &doc);
    }
}

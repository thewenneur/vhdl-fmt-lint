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

pub use layering::{config_chain, find_config, load_for_dir, load_layered};

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
        toml::from_str(text).map_err(|e| ConfigError::Invalid(Box::new(e)))
    }

    /// Reads and parses a single config file, without layering.
    pub fn from_file(path: &Path) -> Result<Self, ConfigError> {
        let text = fs::read_to_string(path).map_err(|source| ConfigError::Io {
            path: path.to_owned(),
            source,
        })?;
        toml::from_str(&text).map_err(|e| ConfigError::Parse {
            path: path.to_owned(),
            source: Box::new(e),
        })
    }

    /// The configured standard as the parser's type.
    pub fn standard(&self) -> Standard {
        self.project.standard.into()
    }
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
}

impl Default for ProjectConfig {
    fn default() -> Self {
        ProjectConfig {
            standard: StandardSetting::default(),
            extensions: vec!["vhd".into(), "vhdl".into()],
            exclude: Vec::new(),
            libraries: BTreeMap::new(),
        }
    }
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
            mode: VerilogMode::Off,
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
    #[default]
    Off,
    /// Verilog files are read only for the modules VHDL instantiates:
    /// names resolve, nothing in the Verilog files is reported or formatted.
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
    /// Whether `when x => stmt;` may keep its statement on the `when` line.
    pub case_alternative: CaseAlternative,
    pub line_ending: LineEnding,
    /// Whether editors may format the line just typed (after `;` or a new
    /// line). Editors have their own switch too.
    pub on_type: bool,
    pub align: AlignConfig,
}

impl Default for FormatConfig {
    fn default() -> Self {
        FormatConfig {
            max_width: 100,
            indent: 2,
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
            case_alternative: CaseAlternative::Preserve,
            line_ending: LineEnding::Auto,
            on_type: true,
            align: AlignConfig::default(),
        }
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

/// `[format.align]`: which columns the formatter lines up across
/// consecutive lines of the same shape.
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
    /// `<=` and `:=` in consecutive statements.
    pub assignment: bool,
    /// `=>` in port maps, generic maps and aggregates.
    pub association: bool,
    /// `:` and `:=` in object declarations and record elements, and `is` in
    /// type and subtype declarations.
    pub declaration: bool,
    /// `=>` of case alternatives whose statement is on the same line.
    pub case_choice: bool,
    /// `when` in conditional and selected assignments.
    pub when_clause: bool,
    /// Continuation lines of conditional and selected assignments start
    /// under the first value instead of one indentation level in.
    pub conditional_continuation: bool,
    pub trailing_comment: bool,
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
            block_boundary: BlockBoundary::BlankLine,
        }
    }
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
}

impl Default for LintConfig {
    fn default() -> Self {
        LintConfig {
            preset: Preset::Recommended,
            rules: BTreeMap::new(),
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
    fn unknown_keys_are_rejected() {
        assert!(Config::from_toml_str("[format]\nindnet = 4").is_err());
    }

    #[test]
    fn default_config_round_trips_through_toml() {
        let text = toml::to_string(&Config::default()).unwrap();
        assert_eq!(Config::from_toml_str(&text).unwrap(), Config::default());
    }

    #[test]
    fn every_key_is_documented() {
        // Git may check the doc out with CRLF line endings on Windows.
        let doc = include_str!("../../../docs/configuration.md").replace("\r\n", "\n");
        let mut config = Config::default();
        config.format.continuation_indent = Some(2);
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

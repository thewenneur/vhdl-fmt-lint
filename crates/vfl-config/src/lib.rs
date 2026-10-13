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

/// `[format]`: formatter settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct FormatConfig {
    pub max_width: u32,
    pub indent: u32,
    pub use_tabs: bool,
    pub keyword_case: Case,
    pub identifier_case: Case,
    pub end_label: EndLabel,
    pub align: AlignConfig,
}

impl Default for FormatConfig {
    fn default() -> Self {
        FormatConfig {
            max_width: 100,
            indent: 2,
            use_tabs: false,
            keyword_case: Case::Lower,
            identifier_case: Case::Preserve,
            end_label: EndLabel::Preserve,
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

/// `[format.align]`: which columns the formatter lines up across
/// consecutive lines of the same shape.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AlignConfig {
    pub port_colon: bool,
    pub port_mode: bool,
    pub generic_colon: bool,
    /// `<=` and `:=` in consecutive statements.
    pub assignment: bool,
    /// `=>` in port maps, generic maps and aggregates.
    pub association: bool,
    /// Names, `:` and types in declarative parts.
    pub declaration: bool,
    /// `=>` after `when` choices in case statements.
    pub case_choice: bool,
    pub trailing_comment: bool,
    pub block_boundary: BlockBoundary,
}

impl Default for AlignConfig {
    fn default() -> Self {
        AlignConfig {
            port_colon: true,
            port_mode: true,
            generic_colon: true,
            assignment: true,
            association: true,
            declaration: true,
            case_choice: true,
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
    /// Only a change of construct starts a new block.
    Never,
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
}

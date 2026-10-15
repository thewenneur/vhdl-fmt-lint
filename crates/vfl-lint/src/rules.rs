//! The rule registry: every rule's id, summary and default levels.

use vfl_config::{Level, LintConfig, Preset, RuleSetting};
use vfl_sema::codes;

/// A lint rule.
#[derive(Debug, Clone, Copy)]
pub struct Rule {
    /// `category/name`.
    pub id: &'static str,
    pub summary: &'static str,
    /// Level in the `recommended` preset.
    pub recommended: Level,
    /// Level in the `strict` preset.
    pub strict: Level,
    /// Whether the rule can fix what it reports.
    pub fixable: bool,
}

const fn rule(
    id: &'static str,
    summary: &'static str,
    recommended: Level,
    strict: Level,
    fixable: bool,
) -> Rule {
    Rule {
        id,
        summary,
        recommended,
        strict,
        fixable,
    }
}

use Level::{Error, Info, Off, Warn};

/// Every built-in rule except the `naming/*` family (see [`NAMING_KINDS`]).
pub const RULES: &[Rule] = &[
    rule(
        "syntax/error",
        "The file does not parse.",
        Error,
        Error,
        false,
    ),
    rule(
        codes::UNRESOLVED_NAME,
        "A name is not declared, or a port or generic does not exist.",
        Error,
        Error,
        false,
    ),
    rule(
        codes::UNRESOLVED_UNIT,
        "A library unit, entity or architecture does not exist.",
        Error,
        Error,
        false,
    ),
    rule(
        codes::UNKNOWN_LIBRARY,
        "A library is not part of the project, so names from it are not checked.",
        Info,
        Info,
        false,
    ),
    rule(
        codes::DUPLICATE_DECLARATION,
        "Two declarations of a region have the same name.",
        Error,
        Error,
        false,
    ),
    rule(
        codes::DUPLICATE_UNIT,
        "Two design units of a library have the same name.",
        Warn,
        Warn,
        false,
    ),
    rule(
        codes::UNASSOCIATED_PORT,
        "An input port or generic without a default is not associated.",
        Error,
        Error,
        false,
    ),
    rule(
        codes::TYPE_MISMATCH,
        "An expression does not have the type its context needs.",
        Error,
        Error,
        false,
    ),
    rule(
        codes::NO_MATCHING_OVERLOAD,
        "No subprogram or operator of that name accepts the arguments.",
        Error,
        Error,
        false,
    ),
    rule(
        codes::UNUSED_SIGNAL,
        "A signal is never used.",
        Warn,
        Warn,
        true,
    ),
    rule(
        codes::UNUSED_VARIABLE,
        "A variable is never used.",
        Warn,
        Warn,
        true,
    ),
    rule(
        codes::UNUSED_CONSTANT,
        "A local constant is never used.",
        Warn,
        Warn,
        true,
    ),
    rule(
        codes::UNUSED_PORT,
        "A port is not used by any architecture of its entity.",
        Off,
        Warn,
        false,
    ),
    rule(
        codes::UNUSED_GENERIC,
        "A generic is not used by its entity or any of its architectures.",
        Off,
        Warn,
        false,
    ),
    rule(
        codes::UNUSED_USE,
        "Nothing a use clause makes visible is used.",
        Off,
        Warn,
        true,
    ),
    rule(
        codes::UNREAD_SIGNAL,
        "A signal is assigned but never read.",
        Warn,
        Warn,
        false,
    ),
    rule(
        codes::UNDRIVEN_SIGNAL,
        "A signal is read but never assigned, or an output port is never assigned.",
        Warn,
        Warn,
        false,
    ),
    rule(
        codes::SENSITIVITY_LIST,
        "A process reads signals its sensitivity list does not name.",
        Warn,
        Warn,
        true,
    ),
    rule(
        codes::UNNEEDED_SENSITIVITY,
        "A sensitivity list names a signal the process never reads.",
        Off,
        Warn,
        true,
    ),
    rule(
        codes::WIDTH_MISMATCH,
        "An assignment, initial value or port association gives a vector a value with a different static number of elements.",
        Warn,
        Error,
        false,
    ),
    rule(
        codes::INFERRED_LATCH,
        "A combinational process assigns a signal on some paths but not all, which infers a latch.",
        Warn,
        Error,
        false,
    ),
    rule(
        codes::CLOCK_DOMAIN_CROSSING,
        "A signal registered on one clock is used in logic clocked by another without a synchronizer.",
        Info,
        Warn,
        false,
    ),
    rule(
        codes::OPEN_INPUT,
        "An instantiation associates an input port that has no default value with `open`.",
        Warn,
        Error,
        false,
    ),
    rule(
        codes::INOUT_PORT,
        "An `inout` port is only read or only driven by its architectures.",
        Info,
        Warn,
        false,
    ),
    rule(
        codes::MISSING_RESET,
        "A clocked process with a reset assigns a register its reset leaves out, so the reset acts as an enable on it.",
        Info,
        Warn,
        false,
    ),
    rule(
        codes::MIXED_RESET,
        "A reset signal resets one process synchronously and another asynchronously.",
        Warn,
        Warn,
        false,
    ),
    rule(
        codes::RESET_POLARITY,
        "A reset signal is tested active high in one process and active low in another, or its name says active low but it resets when high.",
        Warn,
        Error,
        false,
    ),
    rule(
        codes::FSM_UNREACHABLE_STATE,
        "A state machine has a state that no transition reaches.",
        Warn,
        Warn,
        false,
    ),
    rule(
        codes::FSM_STUCK_STATE,
        "A state machine has a state that nothing but a reset leaves.",
        Info,
        Warn,
        false,
    ),
    rule(
        "verilog/blocking-in-clocked",
        "A clocked `always` block assigns a signal with a blocking assignment (`=`).",
        Warn,
        Warn,
        true,
    ),
    rule(
        "verilog/nonblocking-in-combinational",
        "A combinational `always` block uses a non-blocking assignment (`<=`).",
        Warn,
        Warn,
        true,
    ),
    rule(
        "verilog/multiple-drivers",
        "A variable is assigned by more than one `always` block, or by a block and a continuous assignment.",
        Warn,
        Error,
        false,
    ),
    rule(
        "verilog/implicit-net",
        "A name nothing declares is connected to a port or assigned, which declares an implicit 1-bit wire.",
        Warn,
        Error,
        false,
    ),
    rule(
        "verilog/unconnected-input",
        "An instance leaves an input port of a project module without a default unconnected.",
        Off,
        Warn,
        false,
    ),
    rule(
        "verilog/default-nettype",
        "A module is not under `` `default_nettype none ``, so undeclared names become implicit wires.",
        Off,
        Warn,
        false,
    ),
    rule(
        "verilog/case-default",
        "A `case` in a combinational block has no `default` and does not list every value.",
        Off,
        Warn,
        false,
    ),
    rule(
        "style/no-buffer-ports",
        "A port has mode `buffer`.",
        Warn,
        Warn,
        true,
    ),
    rule(
        "style/no-std-logic-arith",
        "The non-standard Synopsys packages (`std_logic_arith`, `std_logic_unsigned`, `std_logic_signed`) are used instead of `numeric_std`.",
        Warn,
        Warn,
        true,
    ),
    rule(
        "style/line-length",
        "A line is longer than `max` columns (default 120).",
        Off,
        Warn,
        false,
    ),
    rule(
        "style/trailing-whitespace",
        "A line ends with spaces or tabs.",
        Off,
        Warn,
        true,
    ),
    rule(
        "style/tabs",
        "A line is indented with tabs.",
        Off,
        Warn,
        true,
    ),
    rule(
        "style/process-label",
        "A process has no label.",
        Off,
        Warn,
        false,
    ),
    rule(
        "style/positional-association",
        "An instantiation maps ports or generics by position.",
        Off,
        Warn,
        true,
    ),
    rule(
        "style/clock-edge",
        "A clock edge is tested with `clk'event` instead of `rising_edge` or `falling_edge`.",
        Off,
        Warn,
        true,
    ),
    rule(
        "style/file-name",
        "A file's name differs from the name of the primary unit it declares.",
        Off,
        Warn,
        false,
    ),
];

/// The kinds of declaration `naming/<kind>` rules apply to. They are off
/// until configured: setting options (`{ prefix = "s_" }`) turns one on
/// at `warn`.
pub const NAMING_KINDS: &[(&str, &str)] = &[
    ("entity", "entities"),
    ("architecture", "architectures"),
    ("package", "packages"),
    ("component", "components"),
    ("signal", "signals"),
    ("variable", "variables"),
    ("constant", "constants"),
    ("generic", "generics"),
    ("port", "ports (all modes)"),
    ("input-port", "ports of mode `in`"),
    ("output-port", "ports of mode `out` or `buffer`"),
    ("inout-port", "ports of mode `inout`"),
    ("type", "types"),
    ("subtype", "subtypes"),
    ("function", "functions"),
    ("procedure", "procedures"),
    ("alias", "aliases"),
    ("process", "process labels"),
    ("instance", "instance labels"),
    ("generate", "generate statement labels"),
    ("module", "Verilog modules, interfaces and programs"),
    ("parameter", "Verilog parameters"),
    ("task", "Verilog tasks"),
];

/// Looks up a rule by id.
pub fn find(id: &str) -> Option<&'static Rule> {
    RULES.iter().find(|r| r.id == id)
}

/// Whether `id` names a rule (including `naming/<kind>`).
pub fn exists(id: &str) -> bool {
    find(id).is_some()
        || id
            .strip_prefix("naming/")
            .is_some_and(|kind| NAMING_KINDS.iter().any(|(k, _)| *k == kind))
}

/// The configured level of rule `id`.
pub fn level(config: &LintConfig, id: &str) -> Level {
    if let Some(setting) = config.rules.get(id) {
        match setting {
            RuleSetting::Level(level) => return *level,
            RuleSetting::Detailed {
                level: Some(level), ..
            } => return *level,
            // Options without a level turn a rule on.
            RuleSetting::Detailed { level: None, .. } => {
                let default = preset_level(config.preset, id);
                return if default == Off { Warn } else { default };
            }
        }
    }
    preset_level(config.preset, id)
}

fn preset_level(preset: Preset, id: &str) -> Level {
    let Some(rule) = find(id) else {
        return Off;
    };
    match preset {
        Preset::None if id == "syntax/error" => Error,
        Preset::None => Off,
        Preset::Recommended => rule.recommended,
        Preset::Strict => rule.strict,
    }
}

/// The options table of rule `id`, if it has one.
pub fn options<'a>(config: &'a LintConfig, id: &str) -> Option<&'a toml::Table> {
    match config.rules.get(id)? {
        RuleSetting::Detailed { options, .. } => Some(options),
        RuleSetting::Level(_) => None,
    }
}

/// What a quick fix associates a port with when no object of its name is
/// declared: the `undeclared` option of `sema/unassociated-port`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Undeclared {
    /// `open` (the default).
    Open,
    /// The port's own name, to be replaced, for the ports that must be
    /// associated.
    Name,
}

/// The `undeclared` option of `sema/unassociated-port` in `config`, or what
/// is wrong with it.
pub fn undeclared(config: &LintConfig) -> Result<Undeclared, String> {
    let value =
        options(config, vfl_sema::codes::UNASSOCIATED_PORT).and_then(|o| o.get("undeclared"));
    match value.map(|v| v.as_str()) {
        None | Some(Some("open")) => Ok(Undeclared::Open),
        Some(Some("name")) => Ok(Undeclared::Name),
        _ => Err(format!(
            "`undeclared` of `{}` must be \"open\" or \"name\"",
            vfl_sema::codes::UNASSOCIATED_PORT
        )),
    }
}

/// Rule ids in `config` that do not exist.
pub fn unknown_rules(config: &LintConfig) -> Vec<String> {
    config
        .rules
        .keys()
        .filter(|id| !exists(id))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {

    #[test]
    fn the_undeclared_option_is_checked() {
        let config = |toml: &str| -> LintConfig { toml::from_str(toml).unwrap() };
        assert_eq!(undeclared(&config("")), Ok(Undeclared::Open));
        let named = config("[rules]\n\"sema/unassociated-port\" = { undeclared = \"name\" }");
        assert_eq!(undeclared(&named), Ok(Undeclared::Name));
        let bad = config("[rules]\n\"sema/unassociated-port\" = { undeclared = \"x\" }");
        assert!(undeclared(&bad).is_err());
    }

    use super::*;

    #[test]
    fn levels_follow_presets_and_overrides() {
        let mut config = LintConfig::default();
        assert_eq!(level(&config, codes::UNRESOLVED_NAME), Error);
        assert_eq!(level(&config, codes::UNUSED_PORT), Off);
        config.preset = Preset::Strict;
        assert_eq!(level(&config, codes::UNUSED_PORT), Warn);
        config.preset = Preset::None;
        assert_eq!(level(&config, codes::UNRESOLVED_NAME), Off);
        assert_eq!(level(&config, "syntax/error"), Error);
        config.rules.insert(
            "naming/signal".into(),
            RuleSetting::Detailed {
                level: None,
                options: toml::Table::new(),
            },
        );
        assert_eq!(level(&config, "naming/signal"), Warn);
        config
            .rules
            .insert("style/tabs".into(), RuleSetting::Level(Error));
        assert_eq!(level(&config, "style/tabs"), Error);
    }

    #[test]
    fn rule_ids_are_unique_and_well_formed() {
        for (i, rule) in RULES.iter().enumerate() {
            assert!(rule.id.contains('/'), "{}", rule.id);
            assert!(
                RULES[i + 1..].iter().all(|r| r.id != rule.id),
                "{}",
                rule.id
            );
        }
        assert!(exists("naming/signal"));
        assert!(!exists("naming/nothing"));
    }

    #[test]
    fn every_rule_is_documented_with_its_levels() {
        // Git may check the doc out with CRLF line endings on Windows.
        let doc = include_str!("../../../docs/lints.md").replace("\r\n", "\n");
        let name = |level: Level| format!("{level:?}").to_ascii_lowercase();
        for rule in RULES {
            assert!(
                doc.contains(&format!("\n### `{}`\n", rule.id)),
                "docs/lints.md has no section for {}",
                rule.id
            );
            let row = doc
                .lines()
                .find(|l| l.starts_with(&format!("| [`{}`]", rule.id)))
                .unwrap_or_else(|| panic!("docs/lints.md has no summary row for {}", rule.id));
            let cells: Vec<&str> = row.split('|').map(str::trim).collect();
            assert_eq!(cells[2], name(rule.recommended), "{}", rule.id);
            assert_eq!(cells[3], name(rule.strict), "{}", rule.id);
            assert_eq!(cells[4].is_empty(), !rule.fixable, "{}", rule.id);
        }
        for (kind, _) in NAMING_KINDS {
            assert!(
                doc.contains(&format!("| `naming/{kind}` |")),
                "docs/lints.md does not list naming/{kind}"
            );
        }
    }
}

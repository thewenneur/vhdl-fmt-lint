//! `naming/<kind>`: naming conventions for declarations.
//!
//! Options, all optional: `prefix`, `suffix`, `case` (`snake`, `upper`,
//! `lower`, `pascal`, `camel` or `any`) and `pattern` (a regular expression
//! the whole name must match). Port rules for a mode (`naming/input-port`)
//! apply on top of `naming/port`.

use regex::Regex;
use vfl_config::LintConfig;
use vfl_sema::{Decl, DeclKind, Mode};
use vfl_syntax::{SyntaxKind, SyntaxNode};

use crate::{Finding, rules};

/// A compiled naming convention.
#[derive(Debug, Clone, Default)]
pub struct Convention {
    prefix: Option<String>,
    suffix: Option<String>,
    case: Option<String>,
    pattern: Option<Regex>,
}

impl Convention {
    pub fn from_options(options: &toml::Table) -> Result<Self, String> {
        let text = |key: &str| options.get(key).and_then(|v| v.as_str()).map(str::to_owned);
        let case = text("case");
        if let Some(case) = &case
            && !matches!(
                case.as_str(),
                "snake" | "upper" | "lower" | "pascal" | "camel" | "any"
            )
        {
            return Err(format!(
                "unknown case `{case}` (expected snake, upper, lower, pascal, camel or any)"
            ));
        }
        let pattern = match text("pattern") {
            Some(p) => Some(Regex::new(&format!("^(?:{p})$")).map_err(|e| e.to_string())?),
            None => None,
        };
        Ok(Convention {
            prefix: text("prefix"),
            suffix: text("suffix"),
            case,
            pattern,
        })
    }

    /// What is wrong with `name`, if anything.
    pub fn check(&self, name: &str) -> Option<String> {
        if name.starts_with('\\') {
            return None;
        }
        let lower = name.to_ascii_lowercase();
        let mut core = name;
        if let Some(prefix) = &self.prefix {
            if !lower.starts_with(&prefix.to_ascii_lowercase()) {
                return Some(format!("should start with `{prefix}`"));
            }
            core = &core[prefix.len()..];
        }
        if let Some(suffix) = &self.suffix {
            if !lower.ends_with(&suffix.to_ascii_lowercase()) || core.len() < suffix.len() {
                return Some(format!("should end with `{suffix}`"));
            }
            core = &core[..core.len() - suffix.len()];
        }
        if let Some(case) = &self.case {
            let ok = match case.as_str() {
                "snake" | "lower" => !core.chars().any(|c| c.is_ascii_uppercase()),
                "upper" => !core.chars().any(|c| c.is_ascii_lowercase()),
                "pascal" => {
                    core.chars().next().is_none_or(|c| c.is_ascii_uppercase())
                        && !core.contains('_')
                }
                "camel" => {
                    core.chars().next().is_none_or(|c| c.is_ascii_lowercase())
                        && !core.contains('_')
                }
                _ => true,
            };
            if !ok {
                return Some(format!("should be {case} case"));
            }
        }
        if let Some(pattern) = &self.pattern
            && !pattern.is_match(name)
        {
            return Some(format!("should match `{}`", pattern.as_str()));
        }
        None
    }
}

/// The `naming/<kind>` rules that apply to a declaration, most general first.
pub fn kinds(decl: &Decl, root: &SyntaxNode) -> Vec<&'static str> {
    match decl.kind {
        DeclKind::Entity => vec!["entity"],
        DeclKind::Architecture => vec!["architecture"],
        DeclKind::Package | DeclKind::PackageInstance => vec!["package"],
        DeclKind::Component => vec!["component"],
        DeclKind::Signal => vec!["signal"],
        DeclKind::Variable => vec!["variable"],
        DeclKind::Constant => vec!["constant"],
        DeclKind::Generic { .. } => vec!["generic"],
        DeclKind::Port { mode, .. } => match mode {
            Mode::In => vec!["port", "input-port"],
            Mode::Out | Mode::Buffer => vec!["port", "output-port"],
            Mode::InOut => vec!["port", "inout-port"],
            _ => vec!["port"],
        },
        DeclKind::Type => vec!["type"],
        DeclKind::Subtype => vec!["subtype"],
        DeclKind::Function => vec!["function"],
        DeclKind::Procedure => vec!["procedure"],
        DeclKind::Alias => vec!["alias"],
        DeclKind::Label => {
            let statement = root
                .covering_element(decl.full_range)
                .into_node()
                .map(|n| n.kind());
            match statement {
                Some(SyntaxKind::PROCESS_STMT) => vec!["process"],
                Some(SyntaxKind::COMPONENT_INSTANTIATION_STMT) => vec!["instance"],
                Some(
                    SyntaxKind::FOR_GENERATE_STMT
                    | SyntaxKind::IF_GENERATE_STMT
                    | SyntaxKind::CASE_GENERATE_STMT,
                ) => vec!["generate"],
                _ => vec![],
            }
        }
        _ => vec![],
    }
}

/// Conventions compiled from a config, by kind. Invalid options are
/// returned as errors and the rule is skipped.
pub fn conventions(config: &LintConfig) -> (Vec<(&'static str, Convention)>, Vec<String>) {
    let mut out = Vec::new();
    let mut errors = Vec::new();
    for (kind, _) in rules::NAMING_KINDS {
        let id = format!("naming/{kind}");
        if rules::level(config, &id) == vfl_config::Level::Off {
            continue;
        }
        let Some(options) = rules::options(config, &id) else {
            continue;
        };
        match Convention::from_options(options) {
            Ok(c) => out.push((*kind, c)),
            Err(e) => errors.push(format!("{id}: {e}")),
        }
    }
    (out, errors)
}

pub fn check(
    decls: &[&Decl],
    text: &str,
    root: &SyntaxNode,
    conventions: &[(&'static str, Convention)],
    out: &mut Vec<Finding>,
) {
    if conventions.is_empty() {
        return;
    }
    for decl in decls {
        // Implicit declarations (GUARD) and operator symbols are skipped.
        if decl.spelling.is_empty()
            || decl.spelling.starts_with('"')
            || text.get(std::ops::Range::<usize>::from(decl.range)) != Some(decl.spelling.as_str())
        {
            continue;
        }
        for kind in kinds(decl, root) {
            let Some((_, convention)) = conventions.iter().find(|(k, _)| *k == kind) else {
                continue;
            };
            if let Some(problem) = convention.check(&decl.spelling) {
                out.push(Finding::new(
                    leak_id(kind),
                    decl.range,
                    format!("{} `{}` {problem}", decl.kind.describe(), decl.spelling),
                ));
                break;
            }
        }
    }
}

/// `naming/<kind>` as a static string.
fn leak_id(kind: &str) -> &'static str {
    const IDS: &[(&str, &str)] = &[
        ("entity", "naming/entity"),
        ("architecture", "naming/architecture"),
        ("package", "naming/package"),
        ("component", "naming/component"),
        ("signal", "naming/signal"),
        ("variable", "naming/variable"),
        ("constant", "naming/constant"),
        ("generic", "naming/generic"),
        ("port", "naming/port"),
        ("input-port", "naming/input-port"),
        ("output-port", "naming/output-port"),
        ("inout-port", "naming/inout-port"),
        ("type", "naming/type"),
        ("subtype", "naming/subtype"),
        ("function", "naming/function"),
        ("procedure", "naming/procedure"),
        ("alias", "naming/alias"),
        ("process", "naming/process"),
        ("instance", "naming/instance"),
        ("generate", "naming/generate"),
    ];
    IDS.iter()
        .find(|(k, _)| *k == kind)
        .map_or("naming/unknown", |(_, id)| id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn convention(toml: &str) -> Convention {
        Convention::from_options(&toml.parse().unwrap()).unwrap()
    }

    #[test]
    fn prefixes_suffixes_and_case() {
        let c = convention("prefix = \"s_\"\ncase = \"snake\"");
        assert_eq!(c.check("s_valid"), None);
        assert_eq!(c.check("S_valid"), None);
        assert_eq!(c.check("valid").as_deref(), Some("should start with `s_`"));
        assert_eq!(c.check("s_Valid").as_deref(), Some("should be snake case"));
        let c = convention("suffix = \"_t\"\ncase = \"lower\"");
        assert_eq!(c.check("state_t"), None);
        assert!(c.check("state").is_some());
        let c = convention("case = \"upper\"");
        assert_eq!(c.check("C_WIDTH"), None);
        assert!(c.check("c_width").is_some());
        let c = convention("pattern = \"[a-z]+_(i|o)\"");
        assert_eq!(c.check("clk_i"), None);
        assert!(c.check("clk").is_some());
    }

    #[test]
    fn all_kinds_have_ids() {
        for (kind, _) in rules::NAMING_KINDS {
            assert_eq!(leak_id(kind), format!("naming/{kind}"));
        }
    }
}

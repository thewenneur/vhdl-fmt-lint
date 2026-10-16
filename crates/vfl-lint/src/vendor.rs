//! Linting FPGA vendor files (`vendor/*` rules): Quartus settings, SDC
//! and XDC constraints, Tcl scripts. They are not part of the analysis:
//! each is linted on its own, from its text.

use std::path::Path;

use vfl_config::{Config, Level};
use vfl_sema::{Analysis, DeclKind, FileId};
use vfl_syntax::LineIndex;
use vfl_vendor::{Context, Kind, Ports};

use crate::suppress::Suppressions;
use crate::waiver::Waivers;
use crate::{Diagnostic, rules};

pub use vfl_vendor::{Kind as VendorKind, is_vendor_file};

/// The ports of every entity and Verilog module of `analysis`, for
/// `vendor/unknown-port`.
pub fn ports(analysis: &Analysis) -> Ports {
    let mut ports = Ports::default();
    for (unit, info) in analysis.units().iter().enumerate() {
        if info.kind != DeclKind::Entity {
            continue;
        }
        let names = analysis
            .unit_decls(unit as vfl_sema::UnitId)
            .iter()
            .filter(|d| matches!(d.kind, DeclKind::Port { .. }))
            .map(|d| d.name.as_str());
        ports.add(&info.name, names);
    }
    for file in 0..analysis.files().len() {
        for module in analysis.foreign_modules(file as FileId).unwrap_or(&[]) {
            ports.add(&module.name, module.ports.iter().map(|p| p.name.as_str()));
        }
    }
    ports
}

/// The diagnostics of the vendor file at `path`, with `text`, as file
/// `file` of the caller, and problems with the configuration. Nothing for
/// a file that is not a vendor file.
pub fn lint(
    path: &Path,
    text: &str,
    config: &Config,
    root: Option<&Path>,
    ports: Option<&Ports>,
    file: FileId,
) -> (Vec<Diagnostic>, Vec<String>) {
    let Some(kind) = Kind::of(path) else {
        return (Vec::new(), Vec::new());
    };
    let lint = &config.lint;
    let mut errors = Vec::new();
    let level = |id: &str| rules::level(lint, id);
    if rules::RULES
        .iter()
        .filter(|r| r.id.starts_with("vendor/"))
        .all(|r| level(r.id) == Level::Off)
    {
        return (Vec::new(), errors);
    }
    let strict_commands =
        rules::bool_option(lint, vfl_vendor::rules::UNKNOWN_COMMAND, "strict", false)
            .unwrap_or_else(|error| {
                errors.push(error);
                false
            });
    let cx = Context {
        root,
        ports: ports.filter(|_| level(vfl_vendor::rules::UNKNOWN_PORT) != Level::Off),
        strict_commands,
    };
    let findings = vfl_vendor::check(kind, path, text, &cx);
    if findings.is_empty() {
        return (Vec::new(), errors);
    }
    let index = LineIndex::new(text);
    let suppressions = Suppressions::hash_comments(&vfl_vendor::comments(text), text, &index);
    let waivers = Waivers::new(&lint.waivers, path, root, &mut errors);
    let diagnostics = findings
        .into_iter()
        .filter_map(|f| {
            let rule = rules::find(f.rule)?.id;
            let level = level(rule);
            let line = index.line(f.range.start());
            if level == Level::Off
                || suppressions.suppressed(rule, line)
                || waivers.waived(rule, line, &f.message)
            {
                return None;
            }
            Some(Diagnostic {
                rule,
                level,
                file,
                range: f.range,
                message: f.message,
                fix: None,
                rename: None,
            })
        })
        .collect();
    (diagnostics, errors)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_suppressions_and_waivers_apply() {
        let text = "set_global_assignment -name VHLD_FILE a.vhd\n\
                    set_location_assignment PIN_A1 -to a\n\
                    set_location_assignment PIN_A1 -to b ;# vfl-ignore vendor/pin-conflict\n\
                    set_instance_assignmnt -name X 1\n";
        let path = Path::new("/nowhere/top.qsf");
        let rules_of = |config: &Config| -> Vec<&'static str> {
            lint(path, text, config, None, None, 7)
                .0
                .iter()
                .map(|d| d.rule)
                .collect()
        };
        let config = Config::default();
        assert_eq!(
            rules_of(&config),
            ["vendor/misspelled-name", "vendor/unknown-command"]
        );
        let config = Config::from_toml_str(
            "[lint.rules]\n\"vendor/misspelled-name\" = \"off\"\n\
             [[lint.waiver]]\nrules = [\"vendor/unknown-command\"]\n",
        )
        .unwrap();
        assert!(rules_of(&config).is_empty());
        assert!(
            lint(Path::new("a.vhd"), text, &Config::default(), None, None, 0)
                .0
                .is_empty()
        );
    }
}

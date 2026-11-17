//! The hazard checks of `vfl-sema` with their options, the target device
//! and clocks read from the project's vendor files, and the two rules
//! that look at files rather than code: `project/layering` (what a
//! directory may use) and `doc/stale-reference` (comments that name what
//! is no longer there).

use std::path::{Path, PathBuf};

use regex::Regex;
use vfl_config::{Config, LintConfig};
use vfl_sema::{Analysis, Device, FileId, HazardChecks, Timing, codes};
use vfl_syntax::{SyntaxKind, SyntaxNode, TextRange};

use crate::{Finding, rules};

/// The rule of `project/layering`.
pub const LAYERING: &str = "project/layering";
/// The rule of `doc/stale-reference`.
pub const STALE_REFERENCE: &str = "doc/stale-reference";

/// What the vendor files of a project say about its target: the family
/// or part, the clocks and the timing exceptions.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Target {
    /// The family or part, as a vendor file names it: `Cyclone V`,
    /// `xc7a35tcpg236-1`.
    pub part: Option<String>,
    /// `create_clock` periods in ns, by clock name and by the ports they
    /// are on.
    pub clocks: Vec<(String, f64)>,
    /// `set_multicycle_path`: globs on the names of the objects, and the
    /// number of cycles.
    pub multicycle: Vec<(String, u32)>,
    /// `set_false_path`: globs on the names of the objects.
    pub false_paths: Vec<String>,
}

impl Target {
    /// Whether linting with `config` needs the target: `vendor/inference`
    /// or `sema/logic-depth` is on.
    pub fn wanted(config: &Config) -> bool {
        [codes::INFERENCE, codes::LOGIC_DEPTH]
            .iter()
            .any(|id| rules::level(&config.lint, id) != vfl_config::Level::Off)
    }

    /// The target the vendor files under `root` describe; unreadable files
    /// are skipped.
    pub fn load(root: &Path, config: &Config) -> Target {
        let files: Vec<PathBuf> =
            vfl_project::collect_lint_sources(root, &config.project, &config.verilog)
                .map(|(_, _, vendor)| vendor)
                .unwrap_or_default();
        Target::from_files(&files)
    }

    /// The target the vendor files at `paths` describe, with the Vivado
    /// and Quartus project files next to them.
    pub fn from_files(paths: &[PathBuf]) -> Target {
        let mut texts = Vec::new();
        for path in paths {
            if let Ok(text) = std::fs::read_to_string(path) {
                texts.push(text);
            }
        }
        let mut dirs: Vec<&Path> = paths.iter().filter_map(|p| p.parent()).collect();
        dirs.dedup();
        for dir in dirs {
            let Ok(entries) = std::fs::read_dir(dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                let extension = path
                    .extension()
                    .and_then(|e| e.to_str())
                    .map(str::to_ascii_lowercase);
                if matches!(
                    extension.as_deref(),
                    Some("xpr" | "qpf" | "ldf" | "gprj" | "prjx")
                ) && let Ok(text) = std::fs::read_to_string(&path)
                {
                    texts.push(text);
                }
            }
        }
        let texts: Vec<&str> = texts.iter().map(String::as_str).collect();
        Target::parse(&texts)
    }

    /// The target vendor file texts describe.
    pub fn parse(texts: &[&str]) -> Target {
        let mut target = Target::default();
        let part_patterns = [
            r#"(?im)-name\s+(?:FAMILY|DEVICE)\s+"?([^"\r\n]+?)"?\s*$"#,
            r#"(?i)-part\s+\{?([A-Za-z0-9-]+)"#,
            r#"(?i)set_property\s+(?:-name\s+)?PART\s+\{?([A-Za-z0-9-]+)"#,
            r#"(?i)Name="Part"\s+Val="([^"]+)""#,
            r#"(?i)\bdevice="([A-Za-z0-9-]+)""#,
            r#"(?i)<Device\s+name="([^"]+)""#,
        ];
        let parts: Vec<Regex> = part_patterns
            .iter()
            .filter_map(|p| Regex::new(p).ok())
            .collect();
        for text in texts {
            for regex in &parts {
                for capture in regex.captures_iter(text) {
                    let value = capture[1].trim().to_string();
                    // A family beats a device of the same project, but
                    // any known one beats none.
                    if target.part.is_none() && Device::named(&value).is_some() {
                        target.part = Some(value);
                    }
                }
            }
            for command in commands(text) {
                target.command(&command);
            }
        }
        target
    }

    fn command(&mut self, words: &[String]) {
        let Some(name) = words.first() else {
            return;
        };
        match name.as_str() {
            "create_clock" => {
                let Some(period) = option(words, "-period").and_then(|p| period_ns(&p)) else {
                    return;
                };
                if let Some(name) = option(words, "-name") {
                    self.clocks.push((name.to_ascii_lowercase(), period));
                }
                for object in objects(words) {
                    self.clocks.push((object.to_ascii_lowercase(), period));
                }
            }
            "set_multicycle_path" => {
                let cycles = words[1..]
                    .iter()
                    .find_map(|w| w.parse::<u32>().ok())
                    .unwrap_or(1);
                // Hold multicycles move the hold check only.
                if words.iter().any(|w| w == "-hold") || cycles < 2 {
                    return;
                }
                for object in objects(words) {
                    self.multicycle.push((object.to_ascii_lowercase(), cycles));
                }
            }
            "set_false_path" | "set_clock_groups" => {
                for object in objects(words) {
                    self.false_paths.push(object.to_ascii_lowercase());
                }
            }
            _ => {}
        }
    }
}

/// The Tcl commands of a constraints file as lists of words, with
/// `[get_ports {clk}]` kept as one word and line continuations joined.
fn commands(text: &str) -> Vec<Vec<String>> {
    let mut out = Vec::new();
    let joined = text.replace("\\\r\n", " ").replace("\\\n", " ");
    for line in joined.lines() {
        let line = line.trim();
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        for statement in line.split(';') {
            let mut words = Vec::new();
            let mut word = String::new();
            let mut depth = 0i32;
            let mut quoted = false;
            for c in statement.chars() {
                match c {
                    '"' => {
                        quoted = !quoted;
                        word.push(c);
                    }
                    c if quoted => word.push(c),
                    '[' | '{' => {
                        depth += 1;
                        word.push(c);
                    }
                    ']' | '}' => {
                        depth -= 1;
                        word.push(c);
                    }
                    '#' if depth == 0 && word.is_empty() => break,
                    c if c.is_whitespace() && depth <= 0 => {
                        if !word.is_empty() {
                            words.push(std::mem::take(&mut word));
                        }
                    }
                    c => word.push(c),
                }
            }
            if !word.is_empty() {
                words.push(word);
            }
            if !words.is_empty() {
                out.push(words);
            }
        }
    }
    out
}

/// The value after `-name` in a command, without braces or quotes.
fn option(words: &[String], name: &str) -> Option<String> {
    let i = words.iter().position(|w| w == name)?;
    Some(
        words
            .get(i + 1)?
            .trim_matches(|c| matches!(c, '{' | '}' | '"'))
            .to_string(),
    )
}

/// The names in the `[get_ports ...]`, `[get_clocks ...]`, `[get_cells
/// ...]` (and the like) of a command.
fn objects(words: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for word in words {
        let Some(inner) = word.strip_prefix('[').and_then(|w| w.strip_suffix(']')) else {
            continue;
        };
        let mut parts = inner.split_whitespace();
        let Some(getter) = parts.next() else {
            continue;
        };
        if !getter.starts_with("get_") && getter != "all_registers" {
            continue;
        }
        for part in parts {
            let part = part.trim_matches(|c| matches!(c, '{' | '}' | '"'));
            if part.is_empty() || part.starts_with('-') {
                continue;
            }
            out.push(part.to_string());
        }
    }
    out
}

/// A period in ns: `10`, `10.000`, `"100 MHz"`, `8ns`.
fn period_ns(text: &str) -> Option<f64> {
    let text = text.trim().trim_matches('"').to_ascii_lowercase();
    let number: String = text
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    let value: f64 = number.parse().ok()?;
    let unit = text[number.len()..].trim();
    let ns = match unit {
        "" | "ns" => value,
        "ps" => value / 1000.0,
        "us" => value * 1000.0,
        "mhz" => 1000.0 / value,
        "ghz" => 1.0 / value,
        "khz" => 1_000_000.0 / value,
        _ => return None,
    };
    (ns.is_finite() && ns > 0.0).then_some(ns)
}

/// A clock of the `clocks` option: a number is a frequency in MHz, a
/// string a period or frequency with its unit.
fn clock_period(value: &toml::Value) -> Option<f64> {
    match value {
        toml::Value::Integer(mhz) if *mhz > 0 => Some(1000.0 / *mhz as f64),
        toml::Value::Float(mhz) if *mhz > 0.0 => Some(1000.0 / mhz),
        toml::Value::String(text) => period_ns(text),
        _ => None,
    }
}

/// The device of a rule's `family` option, or of the target.
fn device(
    config: &LintConfig,
    id: &str,
    target: Option<&Target>,
    errors: &mut Vec<String>,
) -> Option<Device> {
    match rules::string_option(config, id, "family") {
        Ok(Some(family)) => {
            let device = Device::named(family);
            if device.is_none() {
                errors.push(format!(
                    "`family` of `{id}` is not a family or part vfl knows: `{family}`"
                ));
            }
            device
        }
        Ok(None) => target
            .and_then(|t| t.part.as_deref())
            .and_then(Device::named),
        Err(error) => {
            errors.push(error);
            None
        }
    }
}

/// Which hazard checks to run, with their options; a bad option is added
/// to `errors`.
pub(crate) fn checks(
    config: &LintConfig,
    enabled: &impl Fn(&str) -> bool,
    target: Option<&Target>,
    errors: &mut Vec<String>,
) -> HazardChecks {
    let inference =
        enabled(codes::INFERENCE).then(|| device(config, codes::INFERENCE, target, errors));
    let logic_depth = enabled(codes::LOGIC_DEPTH).then(|| {
        let device = device(config, codes::LOGIC_DEPTH, target, errors);
        let mut clocks: Vec<(String, f64)> = Vec::new();
        if let Some(table) =
            rules::options(config, codes::LOGIC_DEPTH).and_then(|o| o.get("clocks"))
        {
            match table.as_table() {
                Some(table) => {
                    for (name, value) in table {
                        match clock_period(value) {
                            Some(period) => clocks.push((name.to_ascii_lowercase(), period)),
                            None => errors.push(format!(
                                "clock `{name}` of `{}`: give a frequency in MHz (`100`) or a \
                                 string with its unit (`\"8 ns\"`, `\"125 MHz\"`)",
                                codes::LOGIC_DEPTH
                            )),
                        }
                    }
                }
                None => errors.push(format!(
                    "`clocks` of `{}` must be a table of clock names to frequencies",
                    codes::LOGIC_DEPTH
                )),
            }
        }
        if let Some(target) = target {
            for (name, period) in &target.clocks {
                if !clocks.iter().any(|(n, _)| n == name) {
                    clocks.push((name.clone(), *period));
                }
            }
        }
        let level_ns =
            match rules::options(config, codes::LOGIC_DEPTH).and_then(|o| o.get("level-delay")) {
                Some(value) => match value.as_float().or(value.as_integer().map(|i| i as f64)) {
                    Some(ns) if ns > 0.0 => ns,
                    _ => {
                        errors.push(format!(
                            "`level-delay` of `{}` must be a positive number of ns",
                            codes::LOGIC_DEPTH
                        ));
                        device.as_ref().map_or(0.6, |d| d.level_ns)
                    }
                },
                None => device.as_ref().map_or(0.6, |d| d.level_ns),
            };
        Timing {
            clocks,
            level_ns,
            device: device.map(|d| d.name),
            multicycle: target.map(|t| t.multicycle.clone()).unwrap_or_default(),
            false_paths: target.map(|t| t.false_paths.clone()).unwrap_or_default(),
        }
    });
    HazardChecks {
        integer_overflow: enabled(codes::INTEGER_OVERFLOW),
        dont_care_compare: enabled(codes::DONT_CARE_COMPARE),
        latency_mismatch: enabled(codes::LATENCY_MISMATCH),
        handshake: enabled(codes::HANDSHAKE),
        power_of_two_wrap: enabled(codes::POWER_OF_TWO_WRAP),
        init_reset_mismatch: enabled(codes::INIT_RESET_MISMATCH),
        broken_sequence: enabled(codes::BROKEN_SEQUENCE),
        branch_clone: enabled(codes::BRANCH_CLONE),
        signed_resize: enabled(codes::SIGNED_RESIZE),
        signed_logical_shift: enabled(codes::SIGNED_LOGICAL_SHIFT),
        conversion_overflow: enabled(codes::CONVERSION_OVERFLOW),
        unused_bits: enabled(codes::UNUSED_BITS),
        process_restarts: enabled(codes::PROCESS_RESTARTS),
        unbounded_wait: enabled(codes::UNBOUNDED_WAIT),
        redundant_conversion: enabled(codes::REDUNDANT_CONVERSION),
        swapped_association: enabled(codes::SWAPPED_ASSOCIATION),
        expensive_operator: enabled(codes::EXPENSIVE_OPERATOR),
        inference,
        logic_depth,
    }
}

// ---- project/layering ---------------------------------------------------------

/// The `deny` option of `project/layering`: for each pattern of paths,
/// what files there must not use.
fn layering_rules(
    config: &LintConfig,
    errors: &mut Vec<String>,
) -> Vec<(globset::GlobMatcher, Vec<String>)> {
    let Some(deny) = rules::options(config, LAYERING).and_then(|o| o.get("deny")) else {
        errors.push(format!(
            "`{LAYERING}` needs a `deny` table: `deny = {{ \"rtl/**\" = [\"osvvm\", \"tb/**\"] }}`"
        ));
        return Vec::new();
    };
    let Some(deny) = deny.as_table() else {
        errors.push(format!(
            "`deny` of `{LAYERING}` must be a table of path patterns"
        ));
        return Vec::new();
    };
    let mut out = Vec::new();
    for (pattern, denied) in deny {
        let glob = match globset::Glob::new(pattern) {
            Ok(g) => g.compile_matcher(),
            Err(e) => {
                errors.push(format!("`{pattern}` in `deny` of `{LAYERING}`: {e}"));
                continue;
            }
        };
        let Some(list) = denied.as_array() else {
            errors.push(format!(
                "`{pattern}` in `deny` of `{LAYERING}` must be a list of libraries, packages \
                 and path patterns"
            ));
            continue;
        };
        out.push((
            glob,
            list.iter()
                .filter_map(|v| v.as_str())
                .map(str::to_ascii_lowercase)
                .collect(),
        ));
    }
    out
}

/// The `project/layering` findings of a file.
pub(crate) fn layering(
    analysis: &Analysis,
    file: FileId,
    root: &SyntaxNode,
    project_root: Option<&Path>,
    config: &LintConfig,
    errors: &mut Vec<String>,
    findings: &mut Vec<Finding>,
) {
    let rules = layering_rules(config, errors);
    let path = &analysis.files()[file as usize].path;
    let relative = |p: &Path| -> PathBuf {
        project_root
            .and_then(|r| p.strip_prefix(r).ok())
            .unwrap_or(p)
            .to_path_buf()
    };
    let here = relative(path);
    let denied: Vec<&String> = rules
        .iter()
        .filter(|(glob, _)| glob.is_match(&here))
        .flat_map(|(_, d)| d)
        .collect();
    if denied.is_empty() {
        return;
    }
    let is_path = |d: &str| d.contains('/');
    // Libraries and packages a `library` or `use` clause names.
    for clause in vfl_syntax::green_walk::descendants_where(root, |k| {
        matches!(k, SyntaxKind::USE_CLAUSE | SyntaxKind::LIBRARY_CLAUSE)
    }) {
        for name in clause.children() {
            let text: String = name
                .text()
                .to_string()
                .to_ascii_lowercase()
                .split_whitespace()
                .collect();
            let parts: Vec<&str> = text.split('.').collect();
            let candidates: Vec<String> = (1..=parts.len().min(2))
                .map(|n| parts[..n].join("."))
                .collect();
            if let Some(d) = denied
                .iter()
                .find(|d| !is_path(d) && candidates.iter().any(|c| vfl_sema::glob(d, c)))
            {
                findings.push(Finding::new(
                    LAYERING,
                    name.text_range(),
                    format!(
                        "files under `{}` must not use `{d}`",
                        parent_pattern(&rules, &here)
                    ),
                ));
            }
        }
    }
    // Units an instance instantiates, by library and name and by path.
    for unit in analysis.file_units(file) {
        for r in analysis.unit_references(unit) {
            let Some(id) = r.targets.iter().find_map(|t| match t {
                vfl_sema::Target::Decl(id) => Some(*id),
                vfl_sema::Target::Library(_) => None,
            }) else {
                continue;
            };
            let Some(decl) = analysis.decl(id) else {
                continue;
            };
            if !matches!(
                decl.kind,
                vfl_sema::DeclKind::Entity | vfl_sema::DeclKind::Component
            ) {
                continue;
            }
            let Some(element) = root.covering_element(r.range).into_token() else {
                continue;
            };
            if !element
                .parent_ancestors()
                .any(|a| a.kind() == SyntaxKind::COMPONENT_INSTANTIATION_STMT)
            {
                continue;
            }
            let info = &analysis.units()[id.unit as usize];
            let qualified = format!("{}.{}", info.library, decl.name);
            let unit_path = relative(&analysis.files()[decl.file as usize].path);
            let hit = denied.iter().find(|d| {
                if is_path(d) {
                    globset::Glob::new(d)
                        .map(|g| g.compile_matcher().is_match(&unit_path))
                        .unwrap_or(false)
                } else {
                    vfl_sema::glob(d, &info.library) || vfl_sema::glob(d, &qualified)
                }
            });
            if let Some(d) = hit {
                findings.push(Finding::new(
                    LAYERING,
                    r.range,
                    format!(
                        "files under `{}` must not instantiate `{}` (`{d}`)",
                        parent_pattern(&rules, &here),
                        decl.spelling
                    ),
                ));
            }
        }
    }
}

/// The first pattern of `rules` a path matches, for messages.
fn parent_pattern(rules: &[(globset::GlobMatcher, Vec<String>)], path: &Path) -> String {
    rules
        .iter()
        .find(|(g, _)| g.is_match(path))
        .map(|(g, _)| g.glob().glob().to_string())
        .unwrap_or_default()
}

// ---- doc/stale-reference ---------------------------------------------------------

/// The `doc/stale-reference` findings of a file: header comments that name
/// another file or unit, and `@verifies` comments that name requirements
/// no `@req` comment of the project defines.
pub(crate) fn stale_references(
    analysis: &Analysis,
    file: FileId,
    root: &SyntaxNode,
    config: &Config,
    findings: &mut Vec<Finding>,
) {
    let analyzed = &analysis.files()[file as usize];
    static HEADER: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| {
        Regex::new(r"(?i)^--+\s*(file(?:\s*name)?|entity|module|unit)\s*[:=]\s*([A-Za-z_][\w.]*)")
            .expect("valid pattern")
    });
    let header = &*HEADER;
    let units = || -> Vec<String> {
        root.children()
            .filter(|n| n.kind() == SyntaxKind::DESIGN_UNIT)
            .flat_map(|u| {
                u.descendants()
                    .filter(|n| {
                        matches!(
                            n.kind(),
                            SyntaxKind::ENTITY_DECL
                                | SyntaxKind::ARCHITECTURE_BODY
                                | SyntaxKind::PACKAGE_DECL
                                | SyntaxKind::PACKAGE_BODY
                                | SyntaxKind::CONFIGURATION_DECL
                                | SyntaxKind::CONTEXT_DECL
                        )
                    })
                    .filter_map(|n| {
                        n.children_with_tokens()
                            .filter_map(|e| e.into_token())
                            .find(|t| t.kind() == SyntaxKind::IDENT)
                            .map(|t| t.text().to_ascii_lowercase())
                    })
                    .collect::<Vec<_>>()
            })
            .collect()
    };
    let mut file_units: Option<Vec<String>> = None;
    let file_name = analyzed
        .path
        .file_name()
        .map(|n| n.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    let stem = analyzed
        .path
        .file_stem()
        .map(|n| n.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    let verifies = &config.doc.spec.verifies_tag;
    let mut comments: Vec<(TextRange, String)> = Vec::new();
    // The comments before the first line of code.
    let text: &str = &analyzed.text;
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        let start = offset;
        let trimmed = line.trim();
        if !trimmed.is_empty() && !trimmed.starts_with("--") {
            break;
        }
        offset += line.len();
        if trimmed.is_empty() {
            continue;
        }
        let at = start + (line.len() - line.trim_start().len());
        let range = TextRange::at((at as u32).into(), (trimmed.len() as u32).into());
        comments.push((range, trimmed.to_string()));
    }
    let header_end = offset;
    // The `@verifies` comments after it, in a file that has any.
    if text[header_end.min(text.len())..].contains(verifies.as_str()) {
        vfl_syntax::green_walk::for_each_token(root, |kind, range, token| {
            if kind == SyntaxKind::LINE_COMMENT
                && usize::from(range.start()) >= header_end
                && token.contains(verifies.as_str())
            {
                comments.push((range, token.to_string()));
            }
        });
    }
    let mut requirements: Option<Vec<String>> = None;
    for (range, comment) in comments {
        if let Some(capture) = header.captures(&comment) {
            let kind = capture[1].to_ascii_lowercase();
            let value = capture[2].to_ascii_lowercase();
            let value = value.trim_end_matches('.');
            let stale = if kind.starts_with("file") {
                value != file_name && value != stem
            } else {
                let units = file_units.get_or_insert_with(units);
                !units.is_empty() && !units.iter().any(|u| u == value)
            };
            if stale {
                findings.push(Finding::new(
                    STALE_REFERENCE,
                    range,
                    format!(
                        "the header names {} `{}`, which is not {}: a copy left unchanged, or a \
                         rename",
                        if kind.starts_with("file") {
                            "file"
                        } else {
                            "unit"
                        },
                        &capture[2],
                        if kind.starts_with("file") {
                            "this file"
                        } else {
                            "a unit of this file"
                        }
                    ),
                ));
            }
            continue;
        }
        let Some(at) = comment.find(verifies.as_str()) else {
            continue;
        };
        let rest = comment[at + verifies.len()..].trim_start_matches(':');
        // `@verifies A, B: what the test does`.
        let rest = rest.split_once(':').map_or(rest, |(ids, _)| ids);
        let known = requirements
            .get_or_insert_with(|| requirement_ids(analysis, &config.doc.spec.requirement_tag));
        for id in rest
            .split([',', ' ', '\t'])
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            if !known.iter().any(|k| k == id) {
                findings.push(Finding::new(
                    STALE_REFERENCE,
                    range,
                    format!(
                        "no `{}` comment of the project defines requirement `{id}`",
                        config.doc.spec.requirement_tag
                    ),
                ));
            }
        }
    }
}

/// The ids of the requirements the project's comments define.
fn requirement_ids(analysis: &Analysis, tag: &str) -> Vec<String> {
    let mut out = Vec::new();
    for file in analysis.user_files() {
        for line in file.text.lines() {
            let Some(comment) = line.find("--").map(|i| &line[i + 2..]) else {
                continue;
            };
            let comment = comment.trim_start_matches(['-', '!']).trim_start();
            if let Some(rest) = comment.strip_prefix(tag) {
                let rest = rest.trim_start_matches(':').trim_start();
                if let Some(id) = rest
                    .split([' ', '\t', ','])
                    .next()
                    .filter(|s| !s.is_empty())
                {
                    out.push(id.trim_end_matches(':').to_string());
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn targets_from_vendor_files() {
        let xdc = "create_clock -period 10.000 -name sys_clk [get_ports {clk}]\n\
                   create_clock -name fast -period \"250 MHz\" [get_ports fast_clk]\n\
                   set_multicycle_path 2 -setup -to [get_cells {slow_sum_reg*}]\n\
                   set_multicycle_path 1 -hold -to [get_cells {slow_sum_reg*}]\n\
                   set_false_path -from [get_clocks a] -to [get_clocks b]\n";
        let tcl = "create_project p ./p -part xc7a35tcpg236-1\n";
        let target = Target::parse(&[xdc, tcl]);
        assert_eq!(target.part.as_deref(), Some("xc7a35tcpg236-1"));
        assert!(target.clocks.contains(&("clk".into(), 10.0)));
        assert!(target.clocks.contains(&("sys_clk".into(), 10.0)));
        assert!(target.clocks.contains(&("fast_clk".into(), 4.0)));
        assert_eq!(target.multicycle, [("slow_sum_reg*".to_string(), 2)]);
        assert_eq!(target.false_paths, ["a", "b"]);
        let qsf = "set_global_assignment -name FAMILY \"Cyclone V\"\n";
        assert_eq!(Target::parse(&[qsf]).part.as_deref(), Some("Cyclone V"));
    }

    #[test]
    fn periods() {
        assert_eq!(period_ns("10"), Some(10.0));
        assert_eq!(period_ns("\"100 MHz\""), Some(10.0));
        assert_eq!(period_ns("8ns"), Some(8.0));
        assert_eq!(period_ns("fast"), None);
    }
}

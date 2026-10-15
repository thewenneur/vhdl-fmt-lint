//! `[[lint.waiver]]`: suppressions written in `vfl.toml` instead of the
//! sources, by file, rule, line and message.

use std::path::Path;

use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use regex::Regex;
use vfl_config::Waiver;

/// The waivers that apply to one file, compiled.
#[derive(Default)]
pub(crate) struct Waivers {
    items: Vec<Compiled>,
}

struct Compiled {
    rules: Vec<String>,
    /// One-based inclusive ranges; any line if empty.
    lines: Vec<(u32, u32)>,
    message: Option<Regex>,
}

impl Waivers {
    /// The waivers of `config` whose files take `path`, a file of the
    /// project at `root`. Invalid ones are skipped with an error.
    pub(crate) fn new(
        config: &[Waiver],
        path: &Path,
        root: Option<&Path>,
        errors: &mut Vec<String>,
    ) -> Self {
        let mut items = Vec::new();
        if config.is_empty() {
            return Waivers { items };
        }
        let relative = root
            .and_then(|root| path.strip_prefix(root).ok())
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");
        for waiver in config {
            let mut invalid = |error: String| {
                let error = format!("[[lint.waiver]]: {error}");
                if !errors.contains(&error) {
                    errors.push(error);
                }
            };
            if !waiver.files.is_empty() {
                match globs(&waiver.files) {
                    Ok(set) if set.is_match(&relative) => {}
                    Ok(_) => continue,
                    Err(error) => {
                        invalid(error.to_string());
                        continue;
                    }
                }
            }
            let lines: Result<Vec<_>, _> = waiver.lines.iter().map(|l| l.range()).collect();
            let lines = match lines {
                Ok(lines) => lines,
                Err(error) => {
                    invalid(error);
                    continue;
                }
            };
            let message = match waiver.message.as_deref().map(Regex::new).transpose() {
                Ok(message) => message,
                Err(error) => {
                    invalid(format!("invalid message pattern: {error}"));
                    continue;
                }
            };
            items.push(Compiled {
                rules: waiver.rules.clone(),
                lines,
                message,
            });
        }
        Waivers { items }
    }

    /// Whether a finding of `rule` starting on zero-based `line` with
    /// `message` is waived.
    pub(crate) fn waived(&self, rule: &str, line: u32, message: &str) -> bool {
        let line = line + 1;
        self.items.iter().any(|w| {
            crate::suppress::matches(&w.rules, rule)
                && (w.lines.is_empty()
                    || w.lines
                        .iter()
                        .any(|&(first, last)| first <= line && line <= last))
                && w.message.as_ref().is_none_or(|m| m.is_match(message))
        })
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

/// Gitignore-style patterns: one without a `/` matches at any depth, and
/// a directory covers what is under it.
fn globs(patterns: &[String]) -> Result<GlobSet, globset::Error> {
    let mut set = GlobSetBuilder::new();
    for pattern in patterns {
        let pattern = pattern.replace('\\', "/");
        let pattern = pattern.trim_start_matches("./").trim_start_matches('/');
        let pattern = if pattern.trim_end_matches('/').contains('/') {
            pattern.to_owned()
        } else {
            format!("**/{pattern}")
        };
        set.add(GlobBuilder::new(&pattern).literal_separator(true).build()?);
        set.add(
            GlobBuilder::new(&format!("{}/**", pattern.trim_end_matches('/')))
                .literal_separator(true)
                .build()?,
        );
    }
    set.build()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn waivers(toml: &str, path: &str) -> (Waivers, Vec<String>) {
        let config: vfl_config::LintConfig = toml::from_str(toml).unwrap();
        let mut errors = Vec::new();
        let w = Waivers::new(
            &config.waivers,
            Path::new(path),
            Some(Path::new("/p")),
            &mut errors,
        );
        (w, errors)
    }

    #[test]
    fn matches_files_rules_lines_and_messages() {
        let toml = "[[waiver]]\nfiles = [\"legacy/**\"]\nrules = [\"style\"]\n\
                    [[waiver]]\nfiles = [\"top.vhd\"]\nrules = [\"sema/unused-signal\"]\n\
                    lines = [3, \"10:12\"]\nmessage = \"`spare\"\n";
        let (w, errors) = waivers(toml, "/p/legacy/a/old.vhd");
        assert!(errors.is_empty(), "{errors:?}");
        assert!(w.waived("style/tabs", 0, ""));
        assert!(!w.waived("sema/unused-signal", 2, "signal `spare` is never used"));

        let (w, _) = waivers(toml, "/p/src/top.vhd");
        assert!(!w.waived("style/tabs", 0, ""));
        let unused = "signal `spare_1` is never used";
        assert!(w.waived("sema/unused-signal", 2, unused));
        assert!(w.waived("sema/unused-signal", 10, unused));
        assert!(!w.waived("sema/unused-signal", 12, unused));
        assert!(!w.waived("sema/unused-signal", 2, "signal `x` is never used"));

        let (w, _) = waivers(toml, "/p/other.vhd");
        assert!(w.is_empty());
    }

    #[test]
    fn invalid_waivers_are_reported_and_skipped() {
        let (w, errors) = waivers(
            "[[waiver]]\nlines = [\"9:3\"]\n[[waiver]]\nmessage = \"(\"\n[[waiver]]\nrules = [\"naming\"]\n",
            "/p/a.vhd",
        );
        assert_eq!(errors.len(), 2, "{errors:?}");
        assert!(w.waived("naming/signal", 0, ""));
        assert!(!w.waived("style/tabs", 0, ""));
    }
}

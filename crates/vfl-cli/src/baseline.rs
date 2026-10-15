//! Baseline files: the findings `vfl lint` already knew about, which it
//! does not report again.
//!
//! A finding is identified by its [`fingerprint`](vfl_lint::report::fingerprint):
//! its file relative to the project root, its rule, its message without
//! numbers and the text of its line, but not the line number, so that
//! editing other parts of a file keeps its findings baselined.

use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

/// One finding of a baseline.
pub struct Entry {
    pub file: String,
    pub rule: String,
    pub message: String,
    pub fingerprint: String,
}

/// The findings of a baseline file not matched yet, by fingerprint.
#[derive(Default)]
pub struct Baseline {
    counts: HashMap<String, usize>,
}

impl Baseline {
    pub fn read(path: &Path) -> Result<Baseline> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading the baseline {}", path.display()))?;
        let value: Value = serde_json::from_str(&text)
            .with_context(|| format!("invalid baseline {}", path.display()))?;
        let Some(findings) = value.get("findings").and_then(Value::as_array) else {
            bail!("invalid baseline {}: no `findings` list", path.display());
        };
        let mut counts: HashMap<String, usize> = HashMap::new();
        for finding in findings {
            let Some(fingerprint) = finding.get("fingerprint").and_then(Value::as_str) else {
                bail!(
                    "invalid baseline {}: a finding has no `fingerprint`",
                    path.display()
                );
            };
            *counts.entry(fingerprint.to_owned()).or_default() += 1;
        }
        Ok(Baseline { counts })
    }

    /// Whether the baseline has a finding with `fingerprint` left; it is
    /// then used up, so that a second identical finding is new.
    pub fn take(&mut self, fingerprint: &str) -> bool {
        match self.counts.get_mut(fingerprint) {
            Some(n) if *n > 0 => {
                *n -= 1;
                true
            }
            _ => false,
        }
    }
}

/// Writes `entries` to `path`, in a stable order.
pub fn write(path: &Path, mut entries: Vec<Entry>) -> Result<()> {
    entries.sort_by(|a, b| {
        (&a.file, &a.rule, &a.fingerprint, &a.message).cmp(&(
            &b.file,
            &b.rule,
            &b.fingerprint,
            &b.message,
        ))
    });
    let findings: Vec<Value> = entries
        .iter()
        .map(|e| {
            json!({
                "file": e.file,
                "rule": e.rule,
                "message": e.message,
                "fingerprint": e.fingerprint,
            })
        })
        .collect();
    let text = serde_json::to_string_pretty(&json!({
        "version": 1,
        "findings": findings,
    }))?;
    std::fs::write(path, text + "\n").with_context(|| format!("writing {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_counts_repeats() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("baseline.json");
        let entry = |fingerprint: &str| Entry {
            file: "a.vhd".into(),
            rule: "style/tabs".into(),
            message: "tab".into(),
            fingerprint: fingerprint.into(),
        };
        write(&path, vec![entry("f2"), entry("f1"), entry("f1")]).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.find("\"f1\"").unwrap() < text.find("\"f2\"").unwrap());
        let mut baseline = Baseline::read(&path).unwrap();
        assert!(baseline.take("f1"));
        assert!(baseline.take("f1"));
        assert!(!baseline.take("f1"));
        assert!(baseline.take("f2"));
        assert!(!baseline.take("f3"));
        std::fs::write(&path, "{}").unwrap();
        assert!(Baseline::read(&path).is_err());
    }
}

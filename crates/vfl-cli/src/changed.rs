//! `vfl lint --changed-since`: the files, and lines, git says changed.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};

/// The files changed since a revision, by canonical path, with their
/// added or modified lines (one-based, inclusive), or `None` for every
/// line of a new file.
pub struct Changed {
    files: HashMap<PathBuf, Option<Vec<(u32, u32)>>>,
    canonical: RefCell<HashMap<PathBuf, Option<PathBuf>>>,
}

fn git(dir: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "core.quotePath=false"])
        .args(args)
        .output()
        .context("running git")?;
    if !out.status.success() {
        bail!(
            "git {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

impl Changed {
    /// What changed in the working tree of the repository holding `dir`
    /// since `rev`, untracked files included; with `lines`, which lines.
    pub fn since(dir: &Path, rev: &str, lines: bool) -> Result<Changed> {
        let top = PathBuf::from(git(dir, &["rev-parse", "--show-toplevel"])?.trim_end());
        let top = top.canonicalize().unwrap_or(top);
        git(
            &top,
            &[
                "rev-parse",
                "--verify",
                "--quiet",
                &format!("{rev}^{{commit}}"),
            ],
        )
        .with_context(|| format!("`{rev}` is not a git revision"))?;
        let mut files: HashMap<PathBuf, Option<Vec<(u32, u32)>>> = HashMap::new();
        let names = git(
            &top,
            &["diff", "--name-only", "-z", "--no-renames", rev, "--"],
        )?;
        for name in names.split('\0').filter(|n| !n.is_empty()) {
            files.insert(top.join(name), (lines).then(Vec::new));
        }
        if lines {
            let diff = git(
                &top,
                &[
                    "diff",
                    "-U0",
                    "--no-color",
                    "--no-ext-diff",
                    "--no-renames",
                    rev,
                    "--",
                ],
            )?;
            for (name, ranges) in added_lines(&diff) {
                files.insert(top.join(name), Some(ranges));
            }
        }
        let untracked = git(&top, &["ls-files", "--others", "--exclude-standard", "-z"])?;
        for name in untracked.split('\0').filter(|n| !n.is_empty()) {
            files.insert(top.join(name), None);
        }
        Ok(Changed {
            files,
            canonical: RefCell::default(),
        })
    }

    /// Whether one-based `line` of `path` changed.
    pub fn contains(&self, path: &Path, line: u32) -> bool {
        let mut canonical = self.canonical.borrow_mut();
        let canonical = canonical
            .entry(path.to_owned())
            .or_insert_with(|| path.canonicalize().ok());
        let Some(ranges) = canonical.as_ref().and_then(|p| self.files.get(p)) else {
            return false;
        };
        ranges.as_ref().is_none_or(|ranges| {
            ranges
                .iter()
                .any(|&(first, last)| first <= line && line <= last)
        })
    }
}

/// The added lines of each file of a `git diff -U0`.
fn added_lines(diff: &str) -> Vec<(String, Vec<(u32, u32)>)> {
    let mut out: Vec<(String, Vec<(u32, u32)>)> = Vec::new();
    for line in diff.lines() {
        if let Some(name) = line.strip_prefix("+++ ") {
            let name = unquote(name);
            if let Some(name) = name.strip_prefix("b/") {
                out.push((name.to_owned(), Vec::new()));
            }
        } else if let Some(hunk) = line.strip_prefix("@@ ") {
            // `@@ -a,b +c,d @@`: `d` lines from `c` (one if no `d`).
            let Some(added) = hunk.split(' ').find_map(|p| p.strip_prefix('+')) else {
                continue;
            };
            let (start, count) = added.split_once(',').unwrap_or((added, "1"));
            let (Ok(start), Ok(count)) = (start.parse::<u32>(), count.parse::<u32>()) else {
                continue;
            };
            if let Some((_, ranges)) = out.last_mut()
                && count > 0
            {
                ranges.push((start, start + count - 1));
            }
        }
    }
    out
}

/// A path as git writes it in a diff header, quoted when it has special
/// characters.
fn unquote(name: &str) -> String {
    let Some(inner) = name.strip_prefix('"').and_then(|n| n.strip_suffix('"')) else {
        return name.to_owned();
    };
    let mut out = String::new();
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('t') => out.push('\t'),
            Some('n') => out.push('\n'),
            Some(c) => out.push(c),
            None => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hunks() {
        let diff = "diff --git a/src/a.vhd b/src/a.vhd\nindex 1..2 100644\n--- a/src/a.vhd\n\
                    +++ b/src/a.vhd\n@@ -3,0 +4,2 @@ entity\n+x\n+y\n@@ -10 +12 @@\n-a\n+b\n\
                    @@ -20,2 +21,0 @@\n-c\n-d\ndiff --git a/gone.vhd b/gone.vhd\n--- a/gone.vhd\n\
                    +++ /dev/null\n@@ -1 +0,0 @@\n-x\n--- a/\"q\\\"t.vhd\"\n+++ \"b/q\\\"t.vhd\"\n\
                    @@ -1 +1 @@\n";
        assert_eq!(
            added_lines(diff),
            vec![
                ("src/a.vhd".to_owned(), vec![(4, 5), (12, 12)]),
                ("q\"t.vhd".to_owned(), vec![(1, 1)]),
            ]
        );
    }
}

//! Explicit library assignments: `[project.libraries]` in `vfl.toml`, and
//! library lists of other tools found in the tree (rust_hdl's
//! `vhdl_ls.toml`), so that migrating needs no configuration.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use globset::{GlobSet, GlobSetBuilder};

use crate::infer::Reason;

/// File names of rust_hdl's config, looked up in the project root.
pub(crate) const VHDL_LS_FILES: [&str; 2] = ["vhdl_ls.toml", ".vhdl_ls.toml"];

/// One source of explicit assignments: library name to glob set.
pub struct LibraryGlobs {
    source: String,
    /// Library names, as identifier keys.
    names: Vec<String>,
    /// Every pattern, relative to the root, with the index of its library.
    patterns: Vec<(usize, String)>,
    /// The patterns, in that order, matched at once.
    set: GlobSet,
    literal_separator: bool,
}

impl LibraryGlobs {
    /// Builds matchers from `library -> patterns`, patterns relative to `base`.
    pub fn new(
        source: String,
        base: &Path,
        root: &Path,
        libraries: &BTreeMap<String, Vec<String>>,
    ) -> Result<Self, globset::Error> {
        Self::build(source, base, root, libraries, false)
    }

    /// The same, with `*` not matching `/` (only `**` crosses directories),
    /// as in the tools whose files are imported.
    pub fn new_strict(
        source: String,
        base: &Path,
        root: &Path,
        libraries: &BTreeMap<String, Vec<String>>,
    ) -> Result<Self, globset::Error> {
        Self::build(source, base, root, libraries, true)
    }

    fn build(
        source: String,
        base: &Path,
        root: &Path,
        libraries: &BTreeMap<String, Vec<String>>,
        literal_separator: bool,
    ) -> Result<Self, globset::Error> {
        let mut names = Vec::new();
        let mut all = Vec::new();
        for (library, patterns) in libraries {
            names.push(vfl_syntax::identifier_key(library));
            for pattern in patterns {
                all.push((names.len() - 1, relative_pattern(base, root, pattern)));
            }
        }
        Ok(LibraryGlobs {
            source,
            names,
            set: glob_set(all.iter().map(|(_, p)| p), literal_separator)?,
            patterns: all,
            literal_separator,
        })
    }

    /// Checks the patterns against the sources of the project (`files`,
    /// relative to `root`). A pattern that matches none, as when the list
    /// was written for another directory or the tree moved since, is
    /// read without its leading directories when that makes it match
    /// (`old/rtl/*.vhd` as `rtl/*.vhd`), if `rebase`. Returns what is
    /// worth telling the user.
    pub fn check(&mut self, root: &Path, files: &[String], rebase: bool) -> Vec<String> {
        let known: HashSet<&str> = files.iter().map(String::as_str).collect();
        let mut matched = vec![false; self.patterns.len()];
        let mut hits = Vec::new();
        for file in files {
            self.set.matches_into(file.as_str(), &mut hits);
            for &i in &hits {
                matched[i] = true;
            }
        }
        let mut rebased: Vec<(String, String)> = Vec::new();
        let mut unmatched: Vec<String> = Vec::new();
        let literal_separator = self.literal_separator;
        for ((_, pattern), matched) in self.patterns.iter_mut().zip(matched) {
            if matched {
                continue;
            }
            let literal = !is_glob(pattern);
            // A file of the tree the project does not hold (ignored, or of
            // another language) is not missing.
            if literal && root.join(&*pattern).exists() {
                continue;
            }
            let found = if rebase {
                rerooted(pattern, |candidate| {
                    if literal {
                        known.contains(candidate)
                    } else {
                        // Only files under its plain leading directories can match.
                        let fixed = &candidate[..candidate.find(is_meta).unwrap_or(0)];
                        let prefix = &fixed[..fixed.rfind('/').map_or(0, |i| i + 1)];
                        let mut under = files.iter().filter(|f| f.starts_with(prefix)).peekable();
                        under.peek().is_some()
                            && globset::GlobBuilder::new(candidate)
                                .literal_separator(literal_separator)
                                .build()
                                .map(|g| g.compile_matcher())
                                .is_ok_and(|m| under.any(|f| m.is_match(f.as_str())))
                    }
                })
            } else {
                None
            };
            match found {
                Some(new) => {
                    rebased.push((pattern.clone(), new.clone()));
                    *pattern = new;
                }
                None => unmatched.push(pattern.clone()),
            }
        }
        if !rebased.is_empty()
            && let Ok(set) = glob_set(self.patterns.iter().map(|(_, p)| p), literal_separator)
        {
            self.set = set;
        }
        let mut notes = Vec::new();
        if let Some((old, new)) = rebased.first() {
            notes.push(format!(
                "{}: {} no source as written; they were read without their leading \
                 directories (`{old}` as `{new}`{}).",
                self.source,
                entries(rebased.len()),
                if rebased.len() > 1 { ", ..." } else { "" },
            ));
        }
        unmatched.sort();
        unmatched.dedup();
        if let Some(first) = unmatched.first() {
            notes.push(format!(
                "{}: {} no source ({first}{}); its other entries are used.",
                self.source,
                entries(unmatched.len()),
                if unmatched.len() > 1 { ", ..." } else { "" },
            ));
        }
        notes
    }

    /// The libraries `path` (relative to the root, `/`-separated) is listed in.
    pub fn assign(&self, relative: &str) -> Option<Vec<(String, Reason)>> {
        let mut hits: Vec<usize> = self
            .set
            .matches(relative)
            .into_iter()
            .map(|i| self.patterns[i].0)
            .collect();
        if hits.is_empty() {
            return None;
        }
        hits.sort_unstable();
        hits.dedup();
        let reason = Reason::Explicit {
            source: self.source.clone(),
        };
        Some(
            hits.into_iter()
                .map(|l| (self.names[l].clone(), reason.clone()))
                .collect(),
        )
    }
}

fn entries(n: usize) -> String {
    if n == 1 {
        "1 entry matches".to_owned()
    } else {
        format!("{n} entries match")
    }
}

fn glob_set<'a>(
    patterns: impl IntoIterator<Item = &'a String>,
    literal_separator: bool,
) -> Result<GlobSet, globset::Error> {
    let mut set = GlobSetBuilder::new();
    for pattern in patterns {
        set.add(
            globset::GlobBuilder::new(pattern)
                .literal_separator(literal_separator)
                .build()?,
        );
    }
    set.build()
}

fn is_meta(c: char) -> bool {
    matches!(c, '*' | '?' | '[' | '{')
}

fn is_glob(pattern: &str) -> bool {
    pattern.contains(is_meta)
}

/// `pattern` without its first directories, as few as make it match
/// (`matches`). What is left must still start with a plain directory, so
/// that it cannot match everywhere.
fn rerooted(pattern: &str, matches: impl Fn(&str) -> bool) -> Option<String> {
    let segments: Vec<&str> = pattern.split('/').collect();
    (1..segments.len().saturating_sub(1))
        .filter(|&k| !is_glob(segments[k]) && segments[k] != "..")
        .map(|k| segments[k..].join("/"))
        .find(|candidate| matches(candidate))
}

/// Turns a pattern written relative to `base` into one relative to `root`.
fn relative_pattern(base: &Path, root: &Path, pattern: &str) -> String {
    let pattern = pattern.replace('\\', "/");
    let pattern = pattern.strip_prefix("./").unwrap_or(&pattern);
    if Path::new(pattern).is_absolute() {
        return match Path::new(pattern).strip_prefix(root) {
            Ok(rest) => path_to_slash(rest),
            Err(_) => pattern.to_owned(),
        };
    }
    let prefix = base
        .strip_prefix(root)
        .map(path_to_slash)
        .unwrap_or_default();
    if prefix.is_empty() {
        normalize(pattern)
    } else {
        normalize(&format!("{prefix}/{pattern}"))
    }
}

/// Resolves `.` and `..` segments: `a/b/../c` is `a/c`. Leading `..` (out
/// of the root) are kept.
fn normalize(pattern: &str) -> String {
    let mut segments: Vec<&str> = Vec::new();
    for segment in pattern.split('/') {
        match segment {
            "." | "" => {}
            ".." if segments.last().is_some_and(|s| *s != ".." && *s != "**") => {
                segments.pop();
            }
            _ => segments.push(segment),
        }
    }
    segments.join("/")
}

pub(crate) fn path_to_slash(path: &Path) -> String {
    path.components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

/// Reads `vhdl_ls.toml` from `root`, if there is one: its `[libraries]`
/// table maps names to `files` lists.
pub fn vhdl_ls_libraries(root: &Path) -> Option<(PathBuf, BTreeMap<String, Vec<String>>)> {
    let path = VHDL_LS_FILES
        .iter()
        .map(|name| root.join(name))
        .find(|p| p.is_file())?;
    let table: toml::Table = std::fs::read_to_string(&path).ok()?.parse().ok()?;
    let libraries = table.get("libraries")?.as_table()?;
    let mut out = BTreeMap::new();
    for (name, entry) in libraries {
        let files = entry
            .get("files")
            .and_then(|f| f.as_array())
            .map(|files| {
                files
                    .iter()
                    .filter_map(|f| f.as_str())
                    .map(expand_env)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        out.insert(name.clone(), files);
    }
    Some((path, out))
}

/// Expands `$VAR` and `${VAR}` as rust_hdl does; unknown variables are kept.
fn expand_env(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(i) = rest.find('$') {
        out.push_str(&rest[..i]);
        rest = &rest[i + 1..];
        let (name, after) = if let Some(braced) = rest.strip_prefix('{') {
            match braced.find('}') {
                Some(end) => (&braced[..end], &braced[end + 1..]),
                None => ("", rest),
            }
        } else {
            let end = rest
                .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                .unwrap_or(rest.len());
            (&rest[..end], &rest[end..])
        };
        match std::env::var(name) {
            Ok(value) if !name.is_empty() => out.push_str(&value),
            _ => {
                out.push('$');
                out.push_str(&rest[..rest.len() - after.len()]);
            }
        }
        rest = after;
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patterns_are_rebased_on_the_root() {
        let root = Path::new("/p");
        assert_eq!(
            relative_pattern(Path::new("/p/sub"), root, "./rtl/*.vhd"),
            "sub/rtl/*.vhd"
        );
        assert_eq!(relative_pattern(root, root, "rtl/**"), "rtl/**");
        assert_eq!(
            relative_pattern(Path::new("/p/a/b"), root, "../../c/*.vhd"),
            "c/*.vhd"
        );
    }

    #[test]
    fn globs_assign_libraries() {
        let mut libraries = BTreeMap::new();
        libraries.insert(
            "OSVVM".to_string(),
            vec!["third_party/osvvm/**/*.vhd".into()],
        );
        let globs = LibraryGlobs::new(
            "vfl.toml".into(),
            Path::new("/p"),
            Path::new("/p"),
            &libraries,
        )
        .unwrap();
        let libs = globs.assign("third_party/osvvm/src/a.vhd").unwrap();
        assert_eq!(libs[0].0, "osvvm");
        assert!(globs.assign("rtl/top.vhd").is_none());
    }

    #[test]
    fn unknown_variables_are_kept() {
        assert_eq!(expand_env("$NO_SUCH_VAR_VFL/x"), "$NO_SUCH_VAR_VFL/x");
        assert_eq!(
            expand_env("a/${NO_SUCH_VAR_VFL}/x"),
            "a/${NO_SUCH_VAR_VFL}/x"
        );
    }
}

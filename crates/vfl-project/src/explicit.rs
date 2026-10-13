//! Explicit library assignments: `[project.libraries]` in `vfl.toml`, and
//! library lists of other tools found in the tree (rust_hdl's
//! `vhdl_ls.toml`), so that migrating needs no configuration.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use globset::{Glob, GlobSet, GlobSetBuilder};

use crate::infer::Reason;

/// File names of rust_hdl's config, looked up in the project root.
const VHDL_LS_FILES: [&str; 2] = ["vhdl_ls.toml", ".vhdl_ls.toml"];

/// One source of explicit assignments: library name to glob set.
pub struct LibraryGlobs {
    source: String,
    libraries: Vec<(String, GlobSet)>,
}

impl LibraryGlobs {
    /// Builds matchers from `library -> patterns`, patterns relative to `base`.
    pub fn new(
        source: String,
        base: &Path,
        root: &Path,
        libraries: &BTreeMap<String, Vec<String>>,
    ) -> Result<Self, globset::Error> {
        let mut out = Vec::new();
        for (library, patterns) in libraries {
            let mut set = GlobSetBuilder::new();
            for pattern in patterns {
                set.add(Glob::new(&relative_pattern(base, root, pattern))?);
            }
            out.push((vfl_syntax::identifier_key(library), set.build()?));
        }
        Ok(LibraryGlobs {
            source,
            libraries: out,
        })
    }

    /// The libraries `path` (relative to the root, `/`-separated) is listed in.
    pub fn assign(&self, relative: &str) -> Option<Vec<(String, Reason)>> {
        let libs: Vec<(String, Reason)> = self
            .libraries
            .iter()
            .filter(|(_, set)| set.is_match(relative))
            .map(|(lib, _)| {
                (
                    lib.clone(),
                    Reason::Explicit {
                        source: self.source.clone(),
                    },
                )
            })
            .collect();
        (!libs.is_empty()).then_some(libs)
    }
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
        pattern.to_owned()
    } else {
        format!("{prefix}/{pattern}")
    }
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

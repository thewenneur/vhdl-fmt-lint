//! Finding config files and merging them into one effective config.

use std::fs;
use std::path::{Path, PathBuf};

use crate::{CONFIG_FILE_NAMES, Config, ConfigError};

/// Keys that control loading and are removed before the schema is applied.
const EXTENDS_KEY: &str = "extends";
const ROOT_KEY: &str = "root";

/// The config file directly in `dir`, if any.
pub fn find_config(dir: &Path) -> Option<PathBuf> {
    CONFIG_FILE_NAMES
        .iter()
        .map(|name| dir.join(name))
        .find(|path| path.is_file())
}

/// Every config file that applies to `dir`, outermost first.
///
/// Walks up from `dir` and stops after a file that sets `root = true`.
pub fn config_chain(dir: &Path) -> Result<Vec<PathBuf>, ConfigError> {
    let mut chain = Vec::new();
    for ancestor in dir.ancestors() {
        if let Some(path) = find_config(ancestor) {
            let is_root = read_table(&path)?
                .get(ROOT_KEY)
                .and_then(toml::Value::as_bool)
                .unwrap_or(false);
            chain.push(path);
            if is_root {
                break;
            }
        }
    }
    chain.reverse();
    Ok(chain)
}

/// The effective config for files in `dir`.
pub fn load_for_dir(dir: &Path) -> Result<Config, ConfigError> {
    load_layered(&config_chain(dir)?)
}

/// The effective config for files in `dir`, or `defaults` when no config
/// file applies. Editors pass their own settings as `defaults`; a config
/// file replaces them all, so that a project is linted and formatted the
/// same in every editor and on the command line.
pub fn load_for_dir_or(dir: &Path, defaults: &Config) -> Result<Config, ConfigError> {
    let chain = config_chain(dir)?;
    if chain.is_empty() {
        return Ok(defaults.clone());
    }
    load_layered(&chain)
}

/// Merges `files` in order (later files override earlier ones) and applies
/// the schema to the result.
pub fn load_layered(files: &[PathBuf]) -> Result<Config, ConfigError> {
    let mut merged = toml::Table::new();
    for path in files {
        let table = resolve_extends(path, &mut Vec::new())?;
        crate::merge_tables(&mut merged, table);
    }
    merged.remove(ROOT_KEY);
    Config::try_from(merged).map_err(|e| ConfigError::Invalid(Box::new(e)))
}

/// Reads `path` with its `extends` base merged underneath it.
fn resolve_extends(path: &Path, stack: &mut Vec<PathBuf>) -> Result<toml::Table, ConfigError> {
    let canonical = path.canonicalize().unwrap_or_else(|_| path.to_owned());
    if stack.contains(&canonical) {
        return Err(ConfigError::ExtendsCycle { path: canonical });
    }
    stack.push(canonical);

    let mut table = read_table(path)?;
    let result = match table.remove(EXTENDS_KEY) {
        Some(toml::Value::String(base)) => {
            let base_path = path.parent().unwrap_or(Path::new(".")).join(base);
            let mut base = resolve_extends(&base_path, stack)?;
            base.remove(ROOT_KEY);
            crate::merge_tables(&mut base, table);
            base
        }
        Some(_) => {
            return Err(ConfigError::Invalid(Box::new(serde::de::Error::custom(
                format!("`extends` in {} must be a path string", path.display()),
            ))));
        }
        None => table,
    };

    stack.pop();
    Ok(result)
}

fn read_table(path: &Path) -> Result<toml::Table, ConfigError> {
    let text = fs::read_to_string(path).map_err(|source| ConfigError::Io {
        path: path.to_owned(),
        source,
    })?;
    text.parse().map_err(|e| ConfigError::Parse {
        path: path.to_owned(),
        source: Box::new(e),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Case, Level};

    fn write(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    #[test]
    fn nested_config_overrides_only_the_keys_it_sets() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(
            &root.join("vfl.toml"),
            "root = true\n[format]\nindent = 4\nkeyword_case = \"upper\"\n",
        );
        write(&root.join("sub/vfl.toml"), "[format]\nindent = 3\n");

        let config = load_for_dir(&root.join("sub")).unwrap();
        assert_eq!(config.format.indent, 3);
        assert_eq!(config.format.keyword_case, Case::Upper);

        let config = load_for_dir(root).unwrap();
        assert_eq!(config.format.indent, 4);
    }

    #[test]
    fn root_key_stops_the_upward_walk() {
        let dir = tempfile::tempdir().unwrap();
        let outer = dir.path();
        write(&outer.join("vfl.toml"), "[format]\nindent = 8\n");
        write(&outer.join("proj/.vfl.toml"), "root = true\n");

        let chain = config_chain(&outer.join("proj")).unwrap();
        assert_eq!(chain, vec![outer.join("proj/.vfl.toml")]);
        assert_eq!(load_for_dir(&outer.join("proj")).unwrap().format.indent, 4);
    }

    #[test]
    fn extends_is_merged_underneath() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(
            &root.join("shared/team.toml"),
            "[lint.rules]\n\"a/b\" = \"error\"\n\"c/d\" = \"warn\"\n",
        );
        write(
            &root.join("vfl.toml"),
            "root = true\nextends = \"shared/team.toml\"\n[lint.rules]\n\"c/d\" = \"off\"\n",
        );

        let config = load_for_dir(root).unwrap();
        assert_eq!(config.lint.rules["a/b"].level(), Some(Level::Error));
        assert_eq!(config.lint.rules["c/d"].level(), Some(Level::Off));
    }

    #[test]
    fn extends_cycle_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(&root.join("a.toml"), "extends = \"vfl.toml\"\n");
        write(
            &root.join("vfl.toml"),
            "root = true\nextends = \"a.toml\"\n",
        );

        assert!(matches!(
            load_for_dir(root),
            Err(ConfigError::ExtendsCycle { .. })
        ));
    }

    #[test]
    fn editor_defaults_apply_only_without_a_config_file() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(&root.join("proj/.git/HEAD"), "");
        let defaults = Config::from_toml_str("[lint]\npreset = \"strict\"\n").unwrap();
        if config_chain(root).unwrap().is_empty() {
            let config = load_for_dir_or(&root.join("proj"), &defaults).unwrap();
            assert_eq!(config, defaults);
        }
        write(&root.join("proj/vfl.toml"), "[format]\nindent = 2\n");
        let config = load_for_dir_or(&root.join("proj"), &defaults).unwrap();
        assert_eq!(config.format.indent, 2);
        assert_eq!(config.lint.preset, crate::Preset::Recommended);
    }

    #[test]
    fn no_config_anywhere_gives_defaults() {
        let dir = tempfile::tempdir().unwrap();
        // The temp dir's ancestors could hold a stray vfl.toml, so only check
        // the chain is empty when it really is.
        if config_chain(dir.path()).unwrap().is_empty() {
            assert_eq!(load_for_dir(dir.path()).unwrap(), Config::default());
        }
    }
}

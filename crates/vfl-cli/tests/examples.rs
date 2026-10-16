//! The configuration guide in `examples/config` stays in sync with what vfl does.
//!
//! Each example has a `vfl.toml`, a `before` file and an `after` file. The `after` file
//! of a formatting example is `vfl fmt` run on `before`; that of a fix example is
//! `vfl lint --fix` (and `--unsafe-fixes`) run on it. A lint example also commits the
//! findings on `before` in `findings.txt`. Examples fixed by hand must lint clean.
//!
//! To update the generated files after a change to vfl:
//! `VFL_BLESS=1 cargo test -p vfl-cli --test examples`.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[derive(Clone, Copy)]
enum Kind {
    /// `after` is `vfl fmt` run on `before`.
    Format,
    /// `after` is `vfl lint --fix` run on `before`.
    Fix,
    /// `after` is `vfl lint --fix --unsafe-fixes` run on `before`.
    UnsafeFix,
    /// `after` is fixed by hand, and has no findings.
    Manual,
}

const EXAMPLES: &[(&str, Kind)] = &[
    ("format/01-defaults", Kind::Format),
    ("format/02-case-and-end-labels", Kind::Format),
    ("format/03-indentation", Kind::Format),
    ("format/04-list-layouts", Kind::Format),
    ("format/05-leading-separators", Kind::Format),
    ("format/06-alignment-scope", Kind::Format),
    ("format/07-alignment-tokens", Kind::Format),
    ("format/08-style-compact", Kind::Format),
    ("format/09-style-classic", Kind::Format),
    ("format/10-width-case-blank-lines", Kind::Format),
    ("format/11-in-source-regions", Kind::Format),
    ("format/12-verilog", Kind::Format),
    ("format/13-long-expressions", Kind::Format),
    ("format/14-declaration-case-and-blank-lines", Kind::Format),
    ("lint/01-presets-and-levels", Kind::Manual),
    ("lint/02-safe-fixes", Kind::Fix),
    ("lint/03-unsafe-fixes", Kind::UnsafeFix),
    ("lint/04-naming", Kind::Manual),
    ("lint/05-suppressions", Kind::Manual),
];

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/config")
}

fn bless() -> bool {
    std::env::var_os("VFL_BLESS").is_some()
}

/// Reads a file as committed; Git may check it out with CRLF line ends.
fn read(path: &Path) -> String {
    std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("{}: {e}", path.display()))
        .replace("\r\n", "\n")
}

fn vfl(dir: &Path, args: &[&str], stdin: Option<&str>) -> String {
    let mut child = Command::new(env!("CARGO_BIN_EXE_vfl"))
        .args(args)
        .current_dir(dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let input = stdin.unwrap_or_default().to_owned();
    let mut pipe = child.stdin.take().unwrap();
    let writer = std::thread::spawn(move || {
        use std::io::Write;
        pipe.write_all(input.as_bytes()).unwrap();
    });
    let out = child.wait_with_output().unwrap();
    writer.join().unwrap();
    String::from_utf8(out.stdout).unwrap()
}

/// The `before` file of an example: `before.vhd`, `before.sv`, ...
fn before(dir: &Path) -> PathBuf {
    std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p.file_stem().is_some_and(|s| s == "before"))
        .unwrap_or_else(|| panic!("{}: no before file", dir.display()))
}

/// A scratch project holding the example's `vfl.toml` and one of its files.
fn scratch(dir: &Path, file: &Path) -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("vfl.toml"), read(&dir.join("vfl.toml"))).unwrap();
    std::fs::write(tmp.path().join(file.file_name().unwrap()), read(file)).unwrap();
    tmp
}

/// Compares `fresh` with the committed file, or writes it with `VFL_BLESS`.
fn check(path: &Path, fresh: &str, stale: &mut Vec<String>) {
    if bless() {
        std::fs::write(path, fresh).unwrap();
    } else if std::fs::read_to_string(path)
        .map(|s| s.replace("\r\n", "\n"))
        .ok()
        != Some(fresh.to_owned())
    {
        stale.push(path.display().to_string());
    }
}

#[test]
fn every_example_is_checked() {
    let mut found = Vec::new();
    for group in ["format", "lint"] {
        for entry in std::fs::read_dir(root().join(group)).unwrap() {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_dir() {
                found.push(format!("{group}/{}", entry.file_name().to_string_lossy()));
            }
        }
    }
    found.sort();
    for name in &found {
        let config = read(&root().join(name).join("vfl.toml"));
        assert!(
            config.lines().any(|line| line.starts_with("root = true")),
            "{name}: vfl.toml needs `root = true`, to keep the example a project of its own"
        );
    }
    let listed: Vec<_> = EXAMPLES.iter().map(|(name, _)| name.to_string()).collect();
    assert_eq!(found, listed, "add new examples to EXAMPLES in this file");
}

#[test]
fn examples_are_up_to_date() {
    let mut stale = Vec::new();
    let mut unclean = Vec::new();
    for &(name, kind) in EXAMPLES {
        let dir = root().join(name);
        assert!(dir.join("README.md").is_file(), "{name}: no README.md");
        let before = before(&dir);
        let file_name = before.file_name().unwrap().to_str().unwrap();
        let ext = before.extension().unwrap().to_str().unwrap();
        let after = dir.join(format!("after.{ext}"));
        match kind {
            Kind::Format => {
                let fresh = vfl(
                    &dir,
                    &["fmt", "-", "--stdin-filename", file_name],
                    Some(&read(&before)),
                );
                assert!(!fresh.is_empty(), "{name}: vfl fmt printed nothing");
                // Formatting the result again changes nothing.
                let again = vfl(
                    &dir,
                    &["fmt", "-", "--stdin-filename", file_name],
                    Some(&fresh),
                );
                assert_eq!(again, fresh, "{name}: formatting is not idempotent");
                check(&after, &fresh, &mut stale);
            }
            Kind::Fix | Kind::UnsafeFix | Kind::Manual => {
                let tmp = scratch(&dir, &before);
                let findings = vfl(
                    tmp.path(),
                    &["lint", "--output-format", "concise", file_name],
                    None,
                );
                check(&dir.join("findings.txt"), &findings, &mut stale);
                if let Kind::Manual = kind {
                    let tmp = scratch(&dir, &after);
                    let after_name = format!("after.{ext}");
                    let left = vfl(
                        tmp.path(),
                        &["lint", "--output-format", "concise", &after_name],
                        None,
                    );
                    if !left.is_empty() {
                        unclean.push(format!("{name}:\n{left}"));
                    }
                } else {
                    let mut args = vec!["lint", "--fix"];
                    if let Kind::UnsafeFix = kind {
                        args.push("--unsafe-fixes");
                    }
                    args.push(file_name);
                    vfl(tmp.path(), &args, None);
                    check(&after, &read(&tmp.path().join(file_name)), &mut stale);
                }
            }
        }
    }
    assert!(
        stale.is_empty(),
        "out of date, run `VFL_BLESS=1 cargo test -p vfl-cli --test examples`: {stale:?}"
    );
    assert!(
        unclean.is_empty(),
        "hand-fixed examples still have findings: {unclean:?}"
    );
}

/// `vfl lint` run in `examples/` checks the UART design alone, and finds nothing:
/// the lessons of `config/` are projects of their own.
#[test]
fn examples_dir_lints_clean() {
    let examples = root().join("..");
    let findings = vfl(&examples, &["lint", "--output-format", "concise"], None);
    assert_eq!(findings, "", "vfl lint in examples/ reports findings");
}

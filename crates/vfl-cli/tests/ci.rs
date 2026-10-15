//! End-to-end checks of what CI uses: waivers, baselines, `--changed-since`,
//! `--fix-only`, the JUnit and GitLab formats, `vfl fmt --diff` and
//! `vfl rules --explain`.

use std::path::Path;
use std::process::Command;

fn vfl(dir: &Path, args: &[&str]) -> (bool, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_vfl"))
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn project(files: &[(&str, &str)]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for (name, text) in files {
        write(dir.path(), name, text);
    }
    dir
}

fn write(dir: &Path, name: &str, text: &str) {
    let path = dir.join(name);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// An architecture with an unused signal per name.
fn unused(entity: &str, signals: &[&str]) -> String {
    let mut text = format!("entity {entity} is end;\narchitecture rtl of {entity} is\n");
    for s in signals {
        text.push_str(&format!("  signal {s} : bit;\n"));
    }
    text.push_str("begin\nend;\n");
    text
}

#[test]
fn waivers_silence_by_file_rule_line_and_message() {
    let config = "[[lint.waiver]]\nfiles = [\"vendor/**\"]\n\
                  [[lint.waiver]]\nfiles = [\"src/top.vhd\"]\nrules = [\"sema\"]\nmessage = \"`spare\"\n\
                  [[lint.waiver]]\nfiles = [\"src/top.vhd\"]\nlines = [\"5:5\"]\n";
    let dir = project(&[
        ("vfl.toml", config),
        ("vendor/ip.vhd", &unused("ip", &["a"])),
        (
            "src/top.vhd",
            &unused("top", &["spare_1", "used", "other", "kept"]),
        ),
    ]);
    let (ok, stdout, _) = vfl(dir.path(), &["lint", "--output-format", "concise"]);
    assert!(ok);
    assert_eq!(
        stdout,
        "src/top.vhd:4:10: warning[sema/unused-signal] signal `used` is never used\n\
         src/top.vhd:6:10: warning[sema/unused-signal] signal `kept` is never used\n"
    );

    write(
        dir.path(),
        "vfl.toml",
        "[[lint.waiver]]\nmessage = \"(\"\nlines = [\"x\"]\n",
    );
    let (_, stdout, stderr) = vfl(dir.path(), &["lint", "--output-format", "concise"]);
    assert!(stderr.contains("warning: [[lint.waiver]]"), "{stderr}");
    assert_eq!(stdout.lines().count(), 5, "{stdout}");
}

#[test]
fn baselines_report_only_new_findings() {
    let dir = project(&[
        ("vfl.toml", "[lint]\nbaseline = \"vfl-baseline.json\"\n"),
        ("a.vhd", &unused("a", &["x", "y"])),
    ]);
    let (ok, _, stderr) = vfl(
        dir.path(),
        &["lint", "--write-baseline", "vfl-baseline.json"],
    );
    assert!(ok, "{stderr}");
    assert!(stderr.contains("wrote 2 findings"), "{stderr}");

    let (ok, stdout, stderr) = vfl(dir.path(), &["lint", "--output-format", "concise"]);
    assert!(ok);
    assert_eq!(stdout, "");
    assert!(stderr.contains("0 warnings, 2 in the baseline"), "{stderr}");

    // Lines added above the baselined findings, and a new finding.
    let shifted = format!("-- header\n\n{}", unused("a", &["x", "y", "z"]));
    write(dir.path(), "a.vhd", &shifted);
    let (_, stdout, stderr) = vfl(dir.path(), &["lint", "--output-format", "concise"]);
    assert_eq!(
        stdout,
        "a.vhd:7:10: warning[sema/unused-signal] signal `z` is never used\n"
    );
    assert!(stderr.contains("1 warning, 2 in the baseline"), "{stderr}");

    // `--baseline` overrides the config; a missing file is an error.
    let (ok, _, stderr) = vfl(dir.path(), &["lint", "--baseline", "nothere.json"]);
    assert!(!ok);
    assert!(stderr.contains("nothere.json"), "{stderr}");
}

fn git(dir: &Path, args: &[&str]) -> bool {
    Command::new("git")
        .args([
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .current_dir(dir)
        .output()
        .is_ok_and(|o| o.status.success())
}

#[test]
fn changed_since_reports_changed_files_and_lines() {
    let dir = project(&[
        ("vfl.toml", ""),
        ("a.vhd", &unused("a", &["x"])),
        ("b.vhd", &unused("b", &["y"])),
    ]);
    if !git(dir.path(), &["init", "-q"]) {
        eprintln!("git is not available; skipped");
        return;
    }
    assert!(git(dir.path(), &["add", "-A"]));
    assert!(git(dir.path(), &["commit", "-q", "-m", "init"]));
    write(dir.path(), "b.vhd", &unused("b", &["y", "z"]));
    write(dir.path(), "c.vhd", &unused("c", &["w"]));

    let args = [
        "lint",
        "--output-format",
        "concise",
        "--changed-since",
        "HEAD",
    ];
    let (_, stdout, stderr) = vfl(dir.path(), &args);
    assert_eq!(
        stdout,
        "b.vhd:3:10: warning[sema/unused-signal] signal `y` is never used\n\
         b.vhd:4:10: warning[sema/unused-signal] signal `z` is never used\n\
         c.vhd:3:10: warning[sema/unused-signal] signal `w` is never used\n",
        "{stderr}"
    );
    let (_, stdout, _) = vfl(dir.path(), &[&args[..], &["--changed-lines"]].concat());
    assert_eq!(
        stdout,
        "b.vhd:4:10: warning[sema/unused-signal] signal `z` is never used\n\
         c.vhd:3:10: warning[sema/unused-signal] signal `w` is never used\n"
    );
    let (ok, _, stderr) = vfl(dir.path(), &["lint", "--changed-since", "no-such-ref"]);
    assert!(!ok);
    assert!(stderr.contains("no-such-ref"), "{stderr}");
}

#[test]
fn fix_only_applies_the_fixes_of_the_given_rules() {
    let dir = project(&[
        ("vfl.toml", "[lint]\npreset = \"strict\"\n"),
        (
            "e.vhd",
            "entity e is end;  \narchitecture rtl of e is\nbegin\n\tnull;\nend;\n",
        ),
    ]);
    let (_, _, stderr) = vfl(dir.path(), &["lint", "--fix-only", "style/tabs"]);
    let text = std::fs::read_to_string(dir.path().join("e.vhd")).unwrap();
    assert_eq!(
        text, "entity e is end;  \narchitecture rtl of e is\nbegin\n    null;\nend;\n",
        "{stderr}"
    );
    let (ok, _, stderr) = vfl(dir.path(), &["lint", "--fix-only", "style/nope"]);
    assert!(!ok);
    assert!(stderr.contains("unknown rule `style/nope`"), "{stderr}");
}

#[test]
fn junit_and_gitlab_reports() {
    let dir = project(&[("vfl.toml", ""), ("a.vhd", &unused("a", &["x"]))]);
    let (_, stdout, _) = vfl(dir.path(), &["lint", "--output-format", "junit"]);
    assert!(stdout.starts_with("<?xml"), "{stdout}");
    assert!(
        stdout.contains("<testcase classname=\"a.vhd\" name=\"a.vhd:3:10: sema/unused-signal\">"),
        "{stdout}"
    );
    let (_, stdout, _) = vfl(dir.path(), &["lint", "--output-format", "gitlab"]);
    assert!(
        stdout.contains("\"check_name\": \"sema/unused-signal\""),
        "{stdout}"
    );
    assert!(stdout.contains("\"path\": \"a.vhd\""), "{stdout}");
}

#[test]
fn fmt_diff_prints_a_patch_and_fails() {
    let dir = project(&[("vfl.toml", ""), ("e.vhd", "ENTITY e IS END;\n")]);
    let (ok, stdout, _) = vfl(dir.path(), &["fmt", "--diff", "e.vhd"]);
    assert!(!ok);
    assert_eq!(
        stdout,
        "--- a/e.vhd\n+++ b/e.vhd\n@@ -1 +1,2 @@\n-ENTITY e IS END;\n+entity e is\n+end;\n"
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("e.vhd")).unwrap(),
        "ENTITY e IS END;\n"
    );
    vfl(dir.path(), &["fmt"]);
    let (ok, stdout, _) = vfl(dir.path(), &["fmt", "--diff"]);
    assert!(ok);
    assert_eq!(stdout, "");
}

#[test]
fn rules_explain_prints_the_description() {
    let dir = project(&[]);
    let (ok, stdout, _) = vfl(dir.path(), &["rules", "--explain", "style/line-length"]);
    assert!(ok);
    assert!(stdout.starts_with("style/line-length: "), "{stdout}");
    assert!(
        stdout.contains("recommended: off, strict: warn, fix: no"),
        "{stdout}"
    );
    assert!(stdout.contains("format.max_width"), "{stdout}");
    // Only this rule's section.
    assert!(!stdout.contains("### "), "{stdout}");
    let (ok, stdout, _) = vfl(dir.path(), &["rules", "--explain", "naming/signal"]);
    assert!(ok);
    assert!(stdout.contains("`naming/signal` | signals"), "{stdout}");
    let (ok, _, stderr) = vfl(dir.path(), &["rules", "--explain", "nope/nope"]);
    assert!(!ok);
    assert!(stderr.contains("unknown rule"), "{stderr}");
}

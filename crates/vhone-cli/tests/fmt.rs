//! End-to-end checks of `vhone fmt`.

use std::process::Command;

fn vhone(dir: &std::path::Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_vhone"))
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap()
}

#[test]
fn fmt_check_then_rewrite() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("vhone.toml"), "[format]\nindent = 4\n").unwrap();
    let file = dir.path().join("e.vhd");
    std::fs::write(&file, "ENTITY e IS END;\n").unwrap();

    let check = vhone(dir.path(), &["fmt", "--check"]);
    assert!(!check.status.success());
    assert!(String::from_utf8_lossy(&check.stdout).contains("would reformat"));
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "ENTITY e IS END;\n"
    );

    assert!(vhone(dir.path(), &["fmt"]).status.success());
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "entity e is\nend;\n"
    );

    assert!(vhone(dir.path(), &["fmt", "--check"]).status.success());
}

#[test]
fn fmt_reports_syntax_errors_and_leaves_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("bad.vhd");
    std::fs::write(&file, "entity e is port (a : in bit) end;\n").unwrap();
    let out = vhone(dir.path(), &["fmt", "bad.vhd"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("syntax error"));
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "entity e is port (a : in bit) end;\n"
    );
}

#[test]
fn fmt_keeps_latin1_files_latin1() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("l.vhd");
    std::fs::write(&file, b"-- caf\xe9\nENTITY e IS END;\n").unwrap();
    assert!(vhone(dir.path(), &["fmt", "l.vhd"]).status.success());
    assert_eq!(
        std::fs::read(&file).unwrap(),
        b"-- caf\xe9\nentity e is\nend;\n"
    );
}

#[test]
fn fmt_lines_formats_only_those_lines() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("a.vhd");
    let text = "ENTITY e IS END;\n\nARCHITECTURE a OF e IS\nBEGIN\nEND;\n";
    std::fs::write(&file, text).unwrap();

    let check = vhone(dir.path(), &["fmt", "--check", "--lines", "3:4", "a.vhd"]);
    assert!(!check.status.success());
    assert_eq!(std::fs::read_to_string(&file).unwrap(), text);

    assert!(
        vhone(dir.path(), &["fmt", "--lines", "3:4", "a.vhd"])
            .status
            .success()
    );
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "ENTITY e IS END;\n\narchitecture a of e is\nbegin\nEND;\n"
    );

    let bad = vhone(dir.path(), &["fmt", "--lines", "4:3", "a.vhd"]);
    assert!(!bad.status.success());
    let many = vhone(dir.path(), &["fmt", "--lines", "1", "."]);
    assert!(!many.status.success());
}

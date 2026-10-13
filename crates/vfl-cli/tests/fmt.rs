//! End-to-end checks of `vfl fmt`.

use std::process::Command;

fn vfl(dir: &std::path::Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_vfl"))
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap()
}

#[test]
fn fmt_check_then_rewrite() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("vfl.toml"), "[format]\nindent = 4\n").unwrap();
    let file = dir.path().join("e.vhd");
    std::fs::write(&file, "ENTITY e IS END;\n").unwrap();

    let check = vfl(dir.path(), &["fmt", "--check"]);
    assert!(!check.status.success());
    assert!(String::from_utf8_lossy(&check.stdout).contains("would reformat"));
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "ENTITY e IS END;\n"
    );

    assert!(vfl(dir.path(), &["fmt"]).status.success());
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "entity e is\nend;\n"
    );

    assert!(vfl(dir.path(), &["fmt", "--check"]).status.success());
}

#[test]
fn fmt_reports_syntax_errors_and_leaves_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("bad.vhd");
    std::fs::write(&file, "entity e is port (a : in bit) end;\n").unwrap();
    let out = vfl(dir.path(), &["fmt", "bad.vhd"]);
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
    assert!(vfl(dir.path(), &["fmt", "l.vhd"]).status.success());
    assert_eq!(
        std::fs::read(&file).unwrap(),
        b"-- caf\xe9\nentity e is\nend;\n"
    );
}

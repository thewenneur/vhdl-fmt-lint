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

#[test]
fn fmt_lines_formats_only_those_lines() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("a.vhd");
    let text = "ENTITY e IS END;\n\nARCHITECTURE a OF e IS\nBEGIN\nEND;\n";
    std::fs::write(&file, text).unwrap();

    let check = vfl(dir.path(), &["fmt", "--check", "--lines", "3:4", "a.vhd"]);
    assert!(!check.status.success());
    assert_eq!(std::fs::read_to_string(&file).unwrap(), text);

    assert!(
        vfl(dir.path(), &["fmt", "--lines", "3:4", "a.vhd"])
            .status
            .success()
    );
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "ENTITY e IS END;\n\narchitecture a of e is\nbegin\nEND;\n"
    );

    let bad = vfl(dir.path(), &["fmt", "--lines", "4:3", "a.vhd"]);
    assert!(!bad.status.success());
    let many = vfl(dir.path(), &["fmt", "--lines", "1", "."]);
    assert!(!many.status.success());
}

#[test]
fn fmt_formats_verilog_by_extension() {
    let dir = tempfile::tempdir().unwrap();
    let sv = dir.path().join("top.sv");
    std::fs::write(&sv, "module top;assign a=b;endmodule\n").unwrap();
    // A VHDL file of the same project, formatted in the same run.
    std::fs::write(dir.path().join("e.vhd"), "ENTITY e IS END;\n").unwrap();
    assert!(vfl(dir.path(), &["fmt"]).status.success());
    assert_eq!(
        std::fs::read_to_string(&sv).unwrap(),
        "module top;\n  assign a = b;\nendmodule\n"
    );

    // Standard input, named so that it is read as SystemVerilog.
    let mut child = Command::new(env!("CARGO_BIN_EXE_vfl"))
        .args(["fmt", "-", "--stdin-filename", "x.sv"])
        .current_dir(dir.path())
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    use std::io::Write;
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"module m;wire  a;endmodule\n")
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success());
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "module m;\n  wire a;\nendmodule\n"
    );
}

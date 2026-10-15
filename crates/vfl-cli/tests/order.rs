//! End-to-end checks of `vfl order`.

use std::process::Command;

fn vfl(dir: &std::path::Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_vfl"))
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap()
}

fn project() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::write(root.join("vfl.toml"), "[project]\nstandard = \"2008\"\n").unwrap();
    std::fs::create_dir_all(root.join("rtl")).unwrap();
    std::fs::create_dir_all(root.join("my lib")).unwrap();
    std::fs::write(root.join("my lib/pkg.vhd"), "package pkg is end;").unwrap();
    std::fs::write(
        root.join("rtl/core.vhd"),
        "library util; use util.pkg.all;\nentity core is end;\narchitecture rtl of core is begin end;",
    )
    .unwrap();
    std::fs::write(
        root.join("rtl/a_tb.vhd"),
        "entity a_tb is end;\narchitecture sim of a_tb is begin\n  dut : entity work.core;\nend;",
    )
    .unwrap();
    std::fs::write(root.join("rtl/other.vhd"), "entity other is end;").unwrap();
    dir
}

#[test]
fn formats() {
    let dir = project();
    let out = vfl(dir.path(), &["order"]);
    assert!(out.status.success(), "{out:?}");
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "util my lib/pkg.vhd\nwork rtl/core.vhd\nwork rtl/a_tb.vhd\nwork rtl/other.vhd\n"
    );

    let out = vfl(dir.path(), &["order", "--top", "a_tb", "-f", "ghdl"]);
    let script = String::from_utf8_lossy(&out.stdout);
    assert!(script.starts_with("#!/bin/sh\n"), "{script}");
    assert!(
        script.ends_with(
            "set -e\n\
             ghdl -a --std=08 --work=util 'my lib/pkg.vhd'\n\
             ghdl -a --std=08 --work=work rtl/core.vhd\n\
             ghdl -a --std=08 --work=work rtl/a_tb.vhd\n"
        ),
        "{script}"
    );

    let out = vfl(dir.path(), &["order", "--top", "core", "-f", "questa"]);
    let script = String::from_utf8_lossy(&out.stdout);
    assert!(
        script.ends_with(
            "vlib util\nvmap util util\nvlib work\nvmap work work\n\
             vcom -2008 -work util {my lib/pkg.vhd}\n\
             vcom -2008 -work work rtl/core.vhd\n"
        ),
        "{script}"
    );

    let out = vfl(dir.path(), &["order", "--top", "work.core", "-f", "vivado"]);
    let script = String::from_utf8_lossy(&out.stdout);
    assert!(
        script.ends_with(
            "read_vhdl -library util -vhdl2008 {my lib/pkg.vhd}\n\
             read_vhdl -library work -vhdl2008 rtl/core.vhd\n"
        ),
        "{script}"
    );

    let out = vfl(dir.path(), &["order", "-f", "json", "--top", "core"]);
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["standard"], "2008");
    assert_eq!(json["libraries"], serde_json::json!(["util", "work"]));
    assert_eq!(json["files"][1]["path"], "rtl/core.vhd");
    assert_eq!(json["files"][1]["language"], "vhdl");

    let out = vfl(dir.path(), &["order", "--top", "missing"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("no entity or configuration `missing`"));
}

#[test]
fn cycles_fail() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("vfl.toml"), "").unwrap();
    std::fs::write(
        dir.path().join("a.vhd"),
        "use work.b.all; package a is end;",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("b.vhd"),
        "use work.a.all; package b is end;",
    )
    .unwrap();
    let out_file = dir.path().join("order.txt");
    let out = vfl(dir.path(), &["order", "-o", out_file.to_str().unwrap()]);
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("a.vhd (work), b.vhd (work)"), "{stderr}");
    assert_eq!(
        std::fs::read_to_string(&out_file).unwrap(),
        "work a.vhd\nwork b.vhd\n"
    );
}

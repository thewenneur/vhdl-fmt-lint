//! End-to-end checks of `vhone new`.

use std::process::Command;

fn vhone(dir: &std::path::Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_vhone"))
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap()
}

const ALU: &str = "\
library ieee;
use ieee.std_logic_1164.all;

entity alu is
  port (
    clk : in  std_logic;
    rst : in  std_logic;
    y   : out std_logic
  );
end entity;
";

#[test]
fn testbench_and_templates() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("vhone.toml"),
        "[format]\nkeyword_case = \"upper\"\n[generate]\ntestbench_name = \"tb_{entity}\"\n",
    )
    .unwrap();
    std::fs::create_dir(dir.path().join("rtl")).unwrap();
    std::fs::write(dir.path().join("rtl/alu.vhd"), ALU).unwrap();

    let out = vhone(dir.path(), &["new", "tb", "alu"]);
    assert!(out.status.success(), "{out:?}");
    let tb = std::fs::read_to_string(dir.path().join("rtl/tb_alu.vhd")).unwrap();
    assert!(tb.contains("ENTITY tb_alu IS"), "{tb}");
    assert!(tb.contains("dut : ENTITY work.alu"), "{tb}");
    assert!(tb.contains("SIGNAL rst : std_logic := '1';"), "{tb}");

    // An existing file is kept unless forced.
    let again = vhone(dir.path(), &["new", "tb", "alu"]);
    assert!(!again.status.success());
    assert!(String::from_utf8_lossy(&again.stderr).contains("--force"));
    assert!(
        vhone(dir.path(), &["new", "tb", "alu", "--force"])
            .status
            .success()
    );

    let instance = vhone(dir.path(), &["new", "instance", "work.alu"]);
    assert!(instance.status.success(), "{instance:?}");
    assert!(String::from_utf8_lossy(&instance.stdout).starts_with("u_alu : ENTITY work.alu\n"));

    let component = vhone(dir.path(), &["new", "component", "ALU"]);
    assert!(String::from_utf8_lossy(&component.stdout).starts_with("COMPONENT alu IS\n"));

    let missing = vhone(dir.path(), &["new", "tb", "nope"]);
    assert!(!missing.status.success());
    assert!(String::from_utf8_lossy(&missing.stderr).contains("no entity `nope`"));
}

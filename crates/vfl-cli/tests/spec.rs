//! End-to-end checks of `vfl spec`.

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
        let path = dir.path().join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    dir
}

const COUNTER: &str = "\
library ieee;
use ieee.std_logic_1164.all;

--! Counts enabled cycles.
--! @req CNT-1: Counts up by one in each cycle `en` is high.
--! @spec Wraps around to zero
--! after the largest value.
entity counter is
  port (
    clk   : in  std_logic;
    rst   : in  std_logic;
    en    : in  std_logic; --! Count enable
    -- @req CNT-2 `count` is zero after a reset.
    count : out natural
  );
end;

architecture rtl of counter is
  signal value : natural; --! The count
begin
  -- @spec Synchronous reset, active high.
  tick : process (clk) begin
    if rising_edge(clk) then
      if rst = '1' then
        value <= 0;
      elsif en = '1' then
        value <= value + 1;
      end if;
    end if;
  end process;
  count <= value;
end;
";

const TB: &str = "\
entity tb is
end;

architecture sim of tb is
begin
  -- @verifies CNT-1, CNT-9: counting
  check : process begin
    wait;
  end process;
end;
";

#[test]
fn writes_a_specification_per_entity() {
    let dir = project(&[
        ("vfl.toml", "[doc]\nformat = \"markdown\"\n"),
        ("src/counter.vhd", COUNTER),
        ("tb/tb.vhd", TB),
    ]);
    let (ok, stdout, stderr) = vfl(dir.path(), &["spec", "--page-for", "src/counter.vhd"]);
    assert!(ok, "{stderr}");
    assert!(stdout.trim_end().ends_with("work.counter.md"), "{stdout}");
    assert!(
        stderr.contains("verifies unknown requirement CNT-9"),
        "{stderr}"
    );
    let spec = dir.path().join("spec");
    let page = std::fs::read_to_string(spec.join("work.counter.md")).unwrap();
    // The spec comments, where they apply.
    assert!(
        page.contains("- `CNT-1` Counts up by one in each cycle `en` is high."),
        "{page}"
    );
    assert!(
        page.contains("Wraps around to zero\nafter the largest value."),
        "{page}"
    );
    assert!(
        page.contains("| `count` | out | `natural` | combinational |  | `CNT-2` |"),
        "{page}"
    );
    assert!(page.contains("Process `tick`"), "{page}");
    assert!(page.contains("Synchronous reset, active high."), "{page}");
    // Read off the code.
    assert!(
        page.contains("| `en` | in | `std_logic` | sampled on `clk` | Count enable |"),
        "{page}"
    );
    assert!(
        page.contains("| `rst` | in | `std_logic` | reset (synchronous, active high, on `clk`) |"),
        "{page}"
    );
    assert!(page.contains("| `value` | `natural` | `clk` | `rst` (synchronous, active high) | `tick` | The count |"), "{page}");
    assert!(page.contains("| `CNT-1` | Counts up by one in each cycle `en` is high. | entity | `src/counter.vhd:5` | `tb.check` `tb/tb.vhd:6` |"), "{page}");
    // No page for the testbench; the index lists every requirement.
    assert!(!spec.join("work.tb.md").exists());
    let index = std::fs::read_to_string(spec.join("index.md")).unwrap();
    assert!(index.contains("2 requirements, 1 verified"), "{index}");
    assert!(
        index.contains("verifies unknown requirement CNT-9"),
        "{index}"
    );

    // `--check` fails on the unknown requirement.
    let (ok, _, _) = vfl(dir.path(), &["spec", "--check"]);
    assert!(!ok);

    // The spec comments stay out of the documentation.
    let (ok, _, stderr) = vfl(dir.path(), &["doc"]);
    assert!(ok, "{stderr}");
    let doc = std::fs::read_to_string(dir.path().join("doc/work.counter.md")).unwrap();
    assert!(doc.contains("Counts enabled cycles."), "{doc}");
    assert!(
        !doc.contains("CNT-1") && !doc.contains("Wraps around"),
        "{doc}"
    );
}

#[test]
fn sections_and_tags_are_configurable() {
    let config = "\
[doc]\nformat = \"markdown\"\n\
[doc.spec]\nout_dir = \"out\"\nrequirement_tag = \"REQ\"\nsections = [\"requirements\", \"overview\"]\n\
title = \"Counter\"\n";
    let counter = COUNTER.replace("@req CNT-1", "REQ CNT-1");
    let dir = project(&[("vfl.toml", config), ("src/counter.vhd", &counter)]);
    let (ok, _, stderr) = vfl(dir.path(), &["spec", "--format", "html"]);
    assert!(ok, "{stderr}");
    let page = std::fs::read_to_string(dir.path().join("out/work.counter.html")).unwrap();
    let requirements = page.find("1 Requirements").expect(&page);
    let overview = page.find("2 Overview").expect(&page);
    assert!(requirements < overview);
    assert!(!page.contains("Interface"), "{page}");
    assert!(page.contains("<code>CNT-1</code>"), "{page}");
    // `@req` is not a tag any more.
    assert!(!page.contains("CNT-2"), "{page}");
    let index = std::fs::read_to_string(dir.path().join("out/index.html")).unwrap();
    assert!(
        index.contains("<title>Counter - Counter</title>"),
        "{index}"
    );
}

/// The specification committed in `examples/uart/spec` matches what
/// `vfl spec` writes now. To update it: `cd examples/uart && vfl spec .`.
#[test]
fn uart_example_spec_is_up_to_date() {
    let example = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/uart");
    let out = tempfile::tempdir().unwrap();
    let (ok, _, err) = vfl(&example, &["spec", ".", "-o", out.path().to_str().unwrap()]);
    assert!(ok, "{err}");
    let committed = example.join("spec");
    let mut names: Vec<_> = std::fs::read_dir(out.path())
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    names.sort();
    let mut stale = Vec::new();
    for name in &names {
        let fresh = std::fs::read_to_string(out.path().join(name)).unwrap();
        let old = std::fs::read_to_string(committed.join(name)).unwrap_or_default();
        // Git may check the files out with CRLF line ends.
        if fresh != old.replace("\r\n", "\n") {
            stale.push(name.to_string_lossy().into_owned());
        }
    }
    assert!(
        stale.is_empty(),
        "examples/uart/spec is out of date ({stale:?}): run `vfl spec .` in examples/uart"
    );
}

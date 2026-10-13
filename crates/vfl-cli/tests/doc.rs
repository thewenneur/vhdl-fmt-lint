//! End-to-end checks of `vfl doc` and `vfl schema`.

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

const BLINK: &str = "\
--! Blinks a LED.
entity blink is
  port (clk : in bit; led : out bit);
end;

architecture rtl of blink is
  type state_t is (OFF, ON_);
  signal state : state_t := OFF;
begin
  process (clk) begin
    if clk'event and clk = '1' then
      case state is
        when OFF => state <= ON_;
        when ON_ => state <= OFF;
      end case;
    end if;
  end process;
end;

entity top is
end;

architecture str of top is
begin
  u_blink : entity work.blink port map (clk => '0', led => open);
end;
";

#[test]
fn doc_writes_the_configured_format_under_the_root() {
    let dir = project(&[
        (
            "vfl.toml",
            "[doc]\nformat = \"markdown\"\nout_dir = \"site\"\ntitle = \"Blinky\"\n",
        ),
        ("src/blink.vhd", BLINK),
    ]);
    let (ok, stdout, stderr) = vfl(&dir.path().join("src"), &["doc"]);
    assert!(ok, "{stdout}{stderr}");
    assert!(stderr.contains("2 entities, 0 packages"), "{stderr}");
    let site = dir.path().join("site");
    let index = std::fs::read_to_string(site.join("index.md")).unwrap();
    assert!(index.starts_with("# Blinky\n"), "{index}");
    assert!(site.join("work.blink.symbol.svg").exists());
    assert!(site.join("work.blink.rtl.fsm-state.svg").exists());

    // Flags override the config.
    let (ok, stdout, stderr) = vfl(
        dir.path(),
        &["doc", "--format", "html", "-o", "html", "--title", "T"],
    );
    assert!(ok, "{stdout}{stderr}");
    let index = std::fs::read_to_string(dir.path().join("html/index.html")).unwrap();
    assert!(index.contains("<title>T - T</title>"), "{index}");
}

#[test]
fn schema_draws_one_diagram() {
    let dir = project(&[("vfl.toml", ""), ("blink.vhd", BLINK)]);
    let (ok, stdout, stderr) = vfl(dir.path(), &["schema", "symbol", "blink"]);
    assert!(ok, "{stderr}");
    assert!(
        stdout.starts_with("<svg") && stdout.contains(">led</text>"),
        "{stdout}"
    );

    let (ok, stdout, stderr) = vfl(
        dir.path(),
        &["schema", "fsm", "work.blink", "-f", "mermaid"],
    );
    assert!(ok, "{stderr}");
    assert_eq!(
        stdout,
        "stateDiagram-v2\n  [*] --> OFF\n  OFF --> ON_\n  ON_ --> OFF\n"
    );

    let (ok, stdout, stderr) = vfl(dir.path(), &["schema", "hierarchy", "-f", "dot"]);
    assert!(ok, "{stderr}");
    assert!(stdout.contains("n0 -> n1 [label=\"u_blink\"];"), "{stdout}");

    let (ok, _, stderr) = vfl(dir.path(), &["schema", "symbol", "nope"]);
    assert!(!ok);
    assert!(stderr.contains("no entity named `nope`"), "{stderr}");
    let (ok, _, stderr) = vfl(dir.path(), &["schema", "symbol", "blink", "-f", "dot"]);
    assert!(!ok);
    assert!(stderr.contains("SVG only"), "{stderr}");
}

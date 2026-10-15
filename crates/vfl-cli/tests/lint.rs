//! End-to-end checks of `vfl lint`, `vfl project` and `vfl rules`.

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

const PKG: &str = "package pkg is\n  constant width : natural := 8;\nend package;\n";

#[test]
fn resolves_names_across_files() {
    let dir = project(&[
        ("vfl.toml", ""),
        ("src/pkg.vhd", PKG),
        (
            "src/top.vhd",
            "use work.pkg.all;\nentity top is\n  port (d : out bit_vector(width - 1 downto 0));\nend;\n\
             architecture rtl of top is\nbegin\n  d <= (others => '0');\nend;\n",
        ),
    ]);
    let (ok, stdout, stderr) = vfl(dir.path(), &["lint", "--output-format", "concise"]);
    assert!(ok, "{stdout}{stderr}");
    assert_eq!(stdout, "");
    assert!(stderr.contains("0 errors, 0 warnings"), "{stderr}");
}

#[test]
fn reports_errors_only_for_requested_files() {
    let dir = project(&[
        ("vfl.toml", ""),
        (
            "a.vhd",
            "entity a is end;\narchitecture rtl of a is\nbegin\n  missing <= '1';\nend;\n",
        ),
        (
            "b.vhd",
            "entity b is end;\narchitecture rtl of b is\nbegin\n  nothing <= '1';\nend;\n",
        ),
    ]);
    let (ok, stdout, _) = vfl(dir.path(), &["lint", "--output-format", "concise", "b.vhd"]);
    assert!(!ok);
    assert_eq!(
        stdout,
        "b.vhd:4:3: error[sema/unresolved-name] `nothing` is not declared\n"
    );
}

#[test]
fn config_levels_suppressions_and_fixes() {
    let dir = project(&[
        (
            "vfl.toml",
            "[lint.rules]\n\"style/trailing-whitespace\" = \"warn\"\n\"naming/signal\" = { prefix = \"s_\" }\n\
             \"sema/combinational-loop\" = \"off\"\n",
        ),
        (
            "e.vhd",
            "entity e is end;  \narchitecture rtl of e is\n  signal s_ok, bad : bit;\n  \
             signal other : bit; -- vfl-ignore naming\nbegin\n  s_ok <= bad;\n  bad <= other;\n  other <= s_ok;\nend;\n",
        ),
    ]);
    let (ok, stdout, _) = vfl(dir.path(), &["lint", "--output-format", "concise"]);
    assert!(ok, "{stdout}");
    assert_eq!(
        stdout,
        "e.vhd:1:17: warning[style/trailing-whitespace] trailing whitespace\n\
         e.vhd:3:16: warning[naming/signal] signal `bad` should start with `s_`\n"
    );
    let (ok, _, _) = vfl(dir.path(), &["lint", "--deny-warnings"]);
    assert!(!ok);
    let (_, stdout, stderr) = vfl(dir.path(), &["lint", "--fix", "--output-format", "concise"]);
    assert!(stderr.contains("fixed 1 problem"), "{stderr}");
    assert!(!stdout.contains("trailing"), "{stdout}");
    assert!(
        std::fs::read_to_string(dir.path().join("e.vhd"))
            .unwrap()
            .starts_with("entity e is end;\n")
    );
}

#[test]
fn machine_readable_output() {
    let dir = project(&[
        ("vfl.toml", ""),
        (
            "a.vhd",
            "entity a is end;\narchitecture rtl of a is\nbegin\n  missing <= '1';\nend;\n",
        ),
    ]);
    let (_, stdout, _) = vfl(dir.path(), &["lint", "--output-format", "json"]);
    assert!(
        stdout.contains("\"rule\": \"sema/unresolved-name\""),
        "{stdout}"
    );
    let (_, stdout, _) = vfl(dir.path(), &["lint", "--output-format", "sarif"]);
    assert!(stdout.contains("\"version\": \"2.1.0\""), "{stdout}");
    let (_, stdout, _) = vfl(dir.path(), &["lint", "--output-format", "github"]);
    assert!(
        stdout.starts_with("::error title=sema/unresolved-name,file=a.vhd,line=4,col=3"),
        "{stdout}"
    );
}

#[test]
fn project_shows_inferred_libraries() {
    let dir = project(&[
        ("vfl.toml", ""),
        ("lib/pkg.vhd", PKG),
        (
            "top.vhd",
            "library mylib;\nuse mylib.pkg.all;\nentity top is end;\n",
        ),
    ]);
    let (ok, stdout, _) = vfl(dir.path(), &["project"]);
    assert!(ok);
    assert!(stdout.contains("lib/pkg.vhd  [mylib]"), "{stdout}");
    assert!(
        stdout.contains("mylib: top.vhd refers to pkg in this library"),
        "{stdout}"
    );
    let (ok, stdout, _) = vfl(dir.path(), &["project", "--vhdl-ls"]);
    assert!(ok);
    assert_eq!(
        stdout,
        "[libraries.defaultlib]\nfiles = [\"top.vhd\"]\n\n[libraries.mylib]\nfiles = [\"lib/pkg.vhd\"]\n"
    );
    let (ok, stdout, _) = vfl(dir.path(), &["rules"]);
    assert!(ok);
    assert!(stdout.contains("sema/unresolved-name"));
}

#[test]
fn branch_assignments_in_clocked_processes_are_an_option() {
    let source = "entity r is\n  port (clk, en, d : in bit; q : out bit);\nend;\n\
                  architecture rtl of r is\nbegin\n  process (clk)\n  begin\n    \
                  if clk'event and clk = '1' then\n      if en = '1' then\n        \
                  q <= d;\n      end if;\n    end if;\n  end process;\nend;\n";
    let rule = "\"sema/missing-branch-assignment\"";
    let on = format!("[lint.rules]\n{rule} = \"warn\"\n");
    let clocked = format!("[lint.rules]\n{rule} = {{ clocked = true }}\n");
    for (config, expected) in [(on.as_str(), ""), (clocked.as_str(), "warning")] {
        let dir = project(&[("vfl.toml", config), ("r.vhd", source)]);
        let (_, stdout, _) = vfl(dir.path(), &["lint", "--output-format", "concise"]);
        assert_eq!(
            stdout.contains(&format!("r.vhd:9:7: {expected}[sema/missing-branch-assignment] this `if` has no `else`, so `q` is not assigned")),
            !expected.is_empty(),
            "{config}{stdout}"
        );
        assert!(!stdout.contains("#clocked"), "{stdout}");
    }
}

#[test]
fn missing_branch_assignments_take_precedence_over_latches() {
    let source = "entity l is\n  port (s, d : in bit; q, r : out bit);\nend;\n\
                  architecture rtl of l is\nbegin\n  process (all)\n  begin\n    \
                  if s = '1' then\n      q <= d;\n    end if;\n  end process;\n  \
                  process (all)\n  begin\n    for i in 0 to 0 loop\n      r <= d;\n    end loop;\n    \
                  if s = '1' then\n      r <= '0';\n    end if;\n  end process;\nend;\n";
    let latch = "q` is not assigned on every path";
    for (config, branches, latches) in [
        ("", 1, 0),
        (
            "[lint.rules]\n\"sema/missing-branch-assignment\" = \"off\"\n",
            0,
            1,
        ),
    ] {
        let dir = project(&[("vfl.toml", config), ("l.vhd", source)]);
        let (_, stdout, _) = vfl(dir.path(), &["lint", "--output-format", "concise"]);
        assert_eq!(
            stdout
                .matches("warning[sema/missing-branch-assignment]")
                .count(),
            branches,
            "{config}{stdout}"
        );
        assert_eq!(stdout.matches(latch).count(), latches, "{config}{stdout}");
        assert!(!stdout.contains("`r`"), "{stdout}");
    }
}

#[test]
fn branch_assignments_are_fixed() {
    let source = "entity f is\n  port (clk, en, d : in bit);\nend;\n\
                  architecture rtl of f is\n  signal r, c : bit;\nbegin\n  \
                  process (clk)\n  begin\n    if clk'event and clk = '1' then\n      \
                  if en = '1' then\n        r <= d;\n      end if;\n    end if;\n  \
                  end process;\n  process (all)\n  begin\n    if en = '1' then\n      \
                  c <= d;\n    end if;\n  end process;\nend;\n";
    let config = "[lint.rules]\n\"sema/missing-branch-assignment\" = { clocked = true }\n";
    let dir = project(&[("vfl.toml", config), ("f.vhd", source)]);
    vfl(dir.path(), &["lint", "--fix"]);
    let fixed = std::fs::read_to_string(dir.path().join("f.vhd")).unwrap();
    assert!(
        fixed.contains("        r <= d;\n      else\n        r <= r;\n      end if;"),
        "{fixed}"
    );
    // Defaults change the design: only with `--unsafe-fixes`.
    assert!(!fixed.contains("c <= '0';"), "{fixed}");
    vfl(dir.path(), &["lint", "--fix", "--unsafe-fixes"]);
    let fixed = std::fs::read_to_string(dir.path().join("f.vhd")).unwrap();
    assert!(
        fixed.contains("  begin\n    c <= '0';\n    if en = '1' then"),
        "{fixed}"
    );
}

#[test]
fn case_without_others_is_reported_in_strict() {
    let source = "entity c is\n  port (s : in bit; q : out bit);\nend;\n\
                  architecture rtl of c is\nbegin\n  process (all)\n  begin\n    \
                  q <= '0';\n    case s is\n      when '1' => q <= '1';\n      \
                  when '0' => null;\n    end case;\n  end process;\nend;\n";
    let dir = project(&[
        ("vfl.toml", "[lint]\npreset = \"strict\"\n"),
        ("c.vhd", source),
    ]);
    let (_, stdout, _) = vfl(dir.path(), &["lint", "--output-format", "concise"]);
    assert!(
        stdout.contains("c.vhd:9:5: warning[style/case-others]"),
        "{stdout}"
    );
    vfl(dir.path(), &["lint", "--fix"]);
    let fixed = std::fs::read_to_string(dir.path().join("c.vhd")).unwrap();
    assert!(
        fixed.contains("      when '0' => null;\n      when others => null;\n    end case;"),
        "{fixed}"
    );
}

#[test]
fn incomplete_cases_are_fixed() {
    let source = "entity k is\n  port (n : in integer range 0 to 7; q : out bit);\nend;\n\
                  architecture rtl of k is\n  type state_t is (idle, run, done);\n  \
                  signal st : state_t;\nbegin\n  process (all)\n  begin\n    \
                  case st is\n      when idle => q <= '0';\n    end case;\n    \
                  case n is\n      when 0 to 2 => q <= '1';\n      when 2 => q <= '0';\n      \
                  when 5 => q <= '0';\n    end case;\n  end process;\nend;\n";
    let dir = project(&[("vfl.toml", ""), ("k.vhd", source)]);
    let (ok, stdout, _) = vfl(dir.path(), &["lint", "--output-format", "concise"]);
    assert!(!ok);
    for expected in [
        "k.vhd:10:5: error[sema/incomplete-case] the choices do not cover `run`, `done`",
        "k.vhd:13:5: error[sema/incomplete-case] the choices do not cover `3 to 4`, `6 to 7`",
        "k.vhd:15:12: error[sema/duplicate-choice] `2` is already covered by the choice at line 14",
    ] {
        assert!(stdout.contains(expected), "{expected}\n{stdout}");
    }
    // The new alternatives do nothing: only with `--unsafe-fixes`.
    vfl(dir.path(), &["lint", "--fix"]);
    let fixed = std::fs::read_to_string(dir.path().join("k.vhd")).unwrap();
    assert_eq!(fixed, source);
    vfl(dir.path(), &["lint", "--fix", "--unsafe-fixes"]);
    let fixed = std::fs::read_to_string(dir.path().join("k.vhd")).unwrap();
    assert!(
        fixed.contains(
            "      when idle => q <= '0';\n      when run => null;\n      when done => null;\n    end case;"
        ),
        "{fixed}"
    );
    assert!(
        fixed.contains(
            "      when 5 => q <= '0';\n      when 3 to 4 => null;\n      when 6 to 7 => null;\n    end case;"
        ),
        "{fixed}"
    );
}

#[test]
fn multiple_drivers_of_resolved_signals_are_an_option() {
    let source = "library ieee;\nuse ieee.std_logic_1164.all;\n\
                  entity m is\n  port (a, b : in std_logic; y : out std_logic);\nend;\n\
                  architecture rtl of m is\n  signal u : std_ulogic;\nbegin\n  \
                  y <= a;\n  y <= b;\n  u <= a;\n  u <= b;\nend;\n\
                  library ieee;\nuse ieee.std_logic_1164.all;\n\
                  entity tb is\nend;\n\
                  architecture sim of tb is\n  signal s : std_logic;\nbegin\n  \
                  s <= '0';\n  s <= '1';\nend;\n";
    let rule = "\"sema/multiple-drivers\"";
    let resolved_y = "m.vhd:10:3: warning[sema/multiple-drivers] `y` is also driven";
    let unresolved_u = "m.vhd:12:3: warning[sema/multiple-drivers] `u` is also driven";
    let testbench_s = "m.vhd:22:3: warning[sema/multiple-drivers] `s` is also driven";
    for (config, y, s) in [
        (String::new(), true, false),
        (
            format!("[lint.rules]\n{rule} = {{ resolved = \"always\" }}\n"),
            true,
            true,
        ),
        (
            format!("[lint.rules]\n{rule} = {{ resolved = \"never\" }}\n"),
            false,
            false,
        ),
    ] {
        let dir = project(&[("vfl.toml", &config), ("m.vhd", source)]);
        let (_, stdout, _) = vfl(dir.path(), &["lint", "--output-format", "concise"]);
        assert!(stdout.contains(unresolved_u), "{config}{stdout}");
        assert_eq!(stdout.contains(resolved_y), y, "{config}{stdout}");
        assert_eq!(stdout.contains(testbench_s), s, "{config}{stdout}");
    }
    let config = format!("[lint.rules]\n{rule} = {{ resolved = 1 }}\n");
    let dir = project(&[("vfl.toml", &config), ("m.vhd", source)]);
    let (_, stdout, stderr) = vfl(dir.path(), &["lint", "--output-format", "concise"]);
    assert!(
        format!("{stdout}{stderr}").contains("`resolved` of `sema/multiple-drivers` must be"),
        "{stdout}{stderr}"
    );
}

#[test]
fn combinational_loops_are_reported() {
    let source = "entity c is\n  port (a, b : in bit; y : out bit);\nend;\n\
                  architecture rtl of c is\n  signal x1, x2 : bit;\nbegin\n  \
                  x1 <= a and x2;\n  x2 <= x1 or b;\n  y <= x2;\nend;\n";
    let dir = project(&[("vfl.toml", ""), ("c.vhd", source)]);
    let (_, stdout, _) = vfl(dir.path(), &["lint", "--output-format", "concise"]);
    assert!(
        stdout.contains("c.vhd:7:3: warning[sema/combinational-loop] combinational loop with no register on the way: `x1` -> `x2` -> `x1`"),
        "{stdout}"
    );
    let dir = project(&[
        (
            "vfl.toml",
            "[lint.rules]\n\"sema/combinational-loop\" = \"off\"\n",
        ),
        ("c.vhd", source),
    ]);
    let (_, stdout, _) = vfl(dir.path(), &["lint", "--output-format", "concise"]);
    assert!(!stdout.contains("combinational-loop"), "{stdout}");
}

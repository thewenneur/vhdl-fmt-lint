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
            "[lint.rules]\n\"style/trailing-whitespace\" = \"warn\"\n\"naming/signal\" = { prefix = \"s_\" }\n",
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
        stdout.contains("top.vhd refers to pkg in this library"),
        "{stdout}"
    );
    let (ok, stdout, _) = vfl(dir.path(), &["rules"]);
    assert!(ok);
    assert!(stdout.contains("sema/unresolved-name"));
}

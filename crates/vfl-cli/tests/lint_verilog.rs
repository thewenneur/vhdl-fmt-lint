//! End-to-end checks of `vfl lint` on Verilog and SystemVerilog files.
//!
//! The files in `tests/verilog` mark the problems they must have with a
//! `// expect: rule-a, rule-b` comment on the line reported; every
//! problem reported must be marked and every mark reported.

use std::collections::BTreeSet;
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

/// (file, line, rule)
type Problem = (String, usize, String);

fn expected(dir: &Path) -> BTreeSet<Problem> {
    let mut out = BTreeSet::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if !matches!(path.extension().and_then(|e| e.to_str()), Some("v" | "sv")) {
            continue;
        }
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let text = std::fs::read_to_string(&path).unwrap();
        for (i, line) in text.lines().enumerate() {
            if let Some((_, rules)) = line.split_once("// expect: ") {
                for rule in rules.split(',') {
                    out.insert((name.clone(), i + 1, rule.trim().to_owned()));
                }
            }
        }
    }
    out
}

/// `file:line:col: level[rule] message`
fn reported(stdout: &str) -> BTreeSet<Problem> {
    stdout
        .lines()
        .map(|line| {
            let mut parts = line.splitn(4, ':');
            let file = parts.next().unwrap().to_owned();
            let line_number = parts.next().unwrap().parse().unwrap();
            let rest = parts.nth(1).unwrap();
            let rule = rest.split_once('[').unwrap().1.split_once(']').unwrap().0;
            (file, line_number, rule.to_owned())
        })
        .collect()
}

#[test]
fn reports_what_the_files_expect() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/verilog");
    let (ok, stdout, stderr) = vfl(&dir, &["lint", "--output-format", "concise"]);
    assert!(!ok, "{stderr}");
    let expected = expected(&dir);
    let reported = reported(&stdout);
    let missing: Vec<_> = expected.difference(&reported).collect();
    let unexpected: Vec<_> = reported.difference(&expected).collect();
    assert!(
        missing.is_empty() && unexpected.is_empty(),
        "missing: {missing:#?}\nunexpected: {unexpected:#?}\n{stdout}"
    );
}

#[test]
fn fixes_assignments_and_connections() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("vfl.toml"), "[lint]\npreset = \"strict\"\n").unwrap();
    std::fs::write(
        dir.path().join("fix.sv"),
        "`default_nettype none\n\
         module leaf (input wire a, output wire y);\n  assign y = a;\nendmodule\n\
         module fix (input wire clk, input wire d, output reg q, output reg r, output wire y);\n\
         \x20 always @(posedge clk) q = d;\n\
         \x20 always @* r <= d;\n\
         \x20 leaf u (d, y);\n\
         endmodule\n",
    )
    .unwrap();
    let (_, stdout, stderr) = vfl(dir.path(), &["lint", "--fix", "--unsafe-fixes"]);
    let fixed = std::fs::read_to_string(dir.path().join("fix.sv")).unwrap();
    assert!(
        fixed.contains("always @(posedge clk) q <= d;"),
        "{fixed}{stdout}{stderr}"
    );
    assert!(fixed.contains("always @* r = d;"), "{fixed}");
    assert!(fixed.contains("leaf u (.a(d), .y(y));"), "{fixed}");
}

#[test]
fn lints_only_the_requested_verilog_files() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("vfl.toml"), "").unwrap();
    let bad = "module bad (output wire y);\n  assign y = missing;\nendmodule\n";
    std::fs::write(dir.path().join("a.v"), bad).unwrap();
    std::fs::write(dir.path().join("b.v"), bad.replace("bad", "other")).unwrap();
    let (_, stdout, _) = vfl(dir.path(), &["lint", "--output-format", "concise", "a.v"]);
    assert!(stdout.contains("a.v:2:14"), "{stdout}");
    assert!(!stdout.contains("b.v"), "{stdout}");
}

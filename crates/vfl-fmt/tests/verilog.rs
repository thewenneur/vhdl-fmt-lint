//! Verilog and SystemVerilog formatting: snapshots of whole files,
//! idempotency over the parser's test files, and the rules that differ
//! from VHDL.

use std::path::Path;

use vfl_config::{Config, FormatConfig};
use vfl_fmt::{FormatError, apply_edits, format_ranges_verilog, format_verilog};
use vfl_verilog_syntax::{Options, Standard};

fn options(path: &Path) -> Options<'static> {
    let standard = path
        .extension()
        .and_then(|e| e.to_str())
        .and_then(vfl_verilog_syntax::default_standard_for_extension)
        .unwrap_or_default();
    Options {
        standard,
        ..Options::default()
    }
}

/// Formats `text` and checks that formatting the result changes nothing.
fn fmt_with(text: &str, config: &FormatConfig) -> String {
    let options = Options::default();
    let out = format_verilog(text, config, &options).unwrap_or_else(|e| panic!("{e}"));
    let again = format_verilog(&out, config, &options).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(again, out, "formatting is not idempotent");
    out
}

fn fmt(text: &str) -> String {
    fmt_with(text, &FormatConfig::default())
}

/// A format config from the body of a `[format]` table.
fn config(toml: &str) -> FormatConfig {
    Config::from_toml_str(&format!("[format]\n{toml}"))
        .unwrap()
        .format
}

#[test]
fn snapshots() {
    insta::glob!("verilog_data/*.*v", |path| {
        let text = std::fs::read_to_string(path).unwrap();
        let out = format_verilog(&text, &FormatConfig::default(), &options(path)).unwrap();
        insta::assert_snapshot!(out);
    });
}

#[test]
fn parser_test_files_are_stable() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../vfl-verilog-syntax/tests/data/ok");
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        let text = std::fs::read_to_string(&path).unwrap();
        for config in [
            FormatConfig::default(),
            config("use_tabs = true\nindent = 4\nmax_width = 40"),
            config(
                "interface_list = \"preserve\"\nassociation_list = \"expand\"\nclose_paren = \"attached\"",
            ),
            config("[format.align]\nblock_boundary = \"never\""),
        ] {
            let options = options(&path);
            let out = format_verilog(&text, &config, &options).unwrap_or_else(|e| panic!("{e}"));
            let again = format_verilog(&out, &config, &options).unwrap();
            assert_eq!(again, out, "{} is not stable", path.display());
        }
    }
}

#[test]
fn files_with_syntax_errors_are_not_formatted() {
    let result = format_verilog(
        "module m; assign a = ; endmodule",
        &FormatConfig::default(),
        &Options::default(),
    );
    assert!(matches!(result, Err(FormatError::Syntax(_))));
}

#[test]
fn spacing() {
    assert_eq!(
        fmt("module m;assign y=a?~b[3:0]:{2{c}}+f(d,-e)*x.y[i+:2];endmodule"),
        "module m;\n  assign y = a ? ~b[3:0] : {2{c}} + f(d, -e) * x.y[i+:2];\nendmodule\n"
    );
    // Expressions between brackets keep their spacing.
    assert_eq!(
        fmt("module m; logic [W-1:0] a; logic [W - 1:0] b; endmodule"),
        "module m;\n  logic [W-1:0]   a;\n  logic [W - 1:0] b;\nendmodule\n"
    );
    assert_eq!(
        fmt(
            "module m; always @(posedge clk) begin i++; x = '{default: 0}; y = int'(z); end endmodule"
        ),
        "module m;\n  always @(posedge clk) begin\n    i++;\n    x = '{default: 0};\n    y = int'(z);\n  end\nendmodule\n"
    );
}

#[test]
fn case_is_kept() {
    // Verilog is case sensitive: the case options do not apply.
    let config = config("keyword_case = \"upper\"\nidentifier_case = \"upper\"");
    assert_eq!(
        fmt_with("module Mod; wire aB; endmodule", &config),
        "module Mod;\n  wire aB;\nendmodule\n"
    );
}

#[test]
fn begin_stays_where_it_is_written() {
    assert_eq!(
        fmt("module m; always_comb if (a) begin x = 1; end else\nbegin x = 0; end endmodule"),
        "module m;\n  always_comb if (a) begin\n    x = 1;\n  end else\n  begin\n    x = 0;\n  end\nendmodule\n"
    );
    // A statement without `begin` keeps its line, or is a continuation.
    assert_eq!(
        fmt("module m; always @*\nif (a)\nx = 1;\nelse x = 0; endmodule"),
        "module m;\n  always @*\n    if (a)\n      x = 1;\n    else x = 0;\nendmodule\n"
    );
}

#[test]
fn ports_and_connections_line_up() {
    assert_eq!(
        fmt(
            "module m(input clk, rst, output logic [7:0] q, inout wire io);\nsub u(.clk(clk),\n.q(q));\nendmodule"
        ),
        "module m (\n  input              clk, rst,\n  output logic [7:0] q,\n  inout  wire        io\n);\n  sub u (\n    .clk(clk),\n    .q  (q)\n  );\nendmodule\n"
    );
}

#[test]
fn code_that_is_not_compiled_is_kept() {
    let text = "module m;\n`ifdef X\n    wire   a;\n`else\n  wire b;\n`endif\nendmodule\n";
    assert_eq!(fmt(text), text);
}

#[test]
fn directives_keep_their_indentation() {
    let text = "module m;\n  `ifndef X\n  wire b;\n  `endif\n`define Y 1\nendmodule\n";
    assert_eq!(fmt(text), text);
}

#[test]
fn escaped_identifiers_end_at_white_space() {
    assert_eq!(
        fmt("module m; wire \\a+b ; assign \\a+b = 1; endmodule"),
        "module m;\n  wire \\a+b ;\n  assign \\a+b = 1;\nendmodule\n"
    );
}

#[test]
fn regions_can_be_left_alone() {
    let text = "module m;\n  // vfl: off\nassign   a=b;\n  // vfl: on\nendmodule\n";
    assert_eq!(fmt(text), text);
}

#[test]
fn ranges() {
    let text = "module m;\nassign a=b;\nassign c=d;\n\nassign e=f;\nendmodule\n";
    let at = text.find("assign c").unwrap();
    let edits = format_ranges_verilog(
        text,
        std::slice::from_ref(&(at..at)),
        &FormatConfig::default(),
        &Options {
            standard: Standard::Sv2017,
            ..Options::default()
        },
    )
    .unwrap();
    assert_eq!(
        apply_edits(text, &edits),
        "module m;\n  assign a = b;\n  assign c = d;\n\nassign e=f;\nendmodule\n"
    );
}

#[test]
fn a_backslash_before_blanks_stays_one_line() {
    // Without its blanks, the `\` would continue the macro on the next line.
    let text = "`define A x \\  \nmodule m; endmodule\n";
    assert_eq!(fmt(text), "`define A x \\  \nmodule m;\nendmodule\n");
}

#[test]
fn comments_after_directives_stay_on_their_line() {
    let text = "`ifndef Q // guard\n`define A 1 // one\nmodule m; endmodule\n`endif // Q\n";
    assert_eq!(
        fmt(text),
        "`ifndef Q // guard\n`define A 1 // one\nmodule m;\nendmodule\n`endif // Q\n"
    );
    // A `\` right before a comment ends the macro there.
    assert_eq!(
        fmt("`define A 1 \\// c\nmodule m; endmodule\n"),
        "`define A 1 \\ // c\nmodule m;\nendmodule\n"
    );
}

#[test]
fn operators_written_apart_stay_apart() {
    assert_eq!(
        fmt("module m; assign a = - -b; assign c = ~ &d; endmodule"),
        "module m;\n  assign a = - -b;\n  assign c = ~ &d;\nendmodule\n"
    );
}

//! Verilog and SystemVerilog formatting: snapshots of whole files,
//! idempotency over the parser's test files, and the rules that differ
//! from VHDL.

use std::path::Path;

use vfl_config::{AlignConfig, AlignScope, Config, FormatConfig};
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
    fmt_with(text, &legacy())
}

/// The defaults before block alignment (0.4): most tests show their own
/// option against them. `alignment_defaults` shows the current defaults.
fn legacy() -> FormatConfig {
    FormatConfig {
        indent: 2,
        align: AlignConfig {
            scope: AlignScope::Contiguous,
            semicolon: false,
            ..AlignConfig::default()
        },
        ..FormatConfig::default()
    }
}

/// A format config from the body of a `[format]` table, on top of
/// [`legacy`] unless it picks a style.
fn config(toml: &str) -> FormatConfig {
    let mut config = Config::from_toml_str(&format!("[format]\n{toml}"))
        .unwrap()
        .format;
    if !toml.contains("style") {
        let sets = |key: &str| toml.lines().any(|l| l.trim_start().starts_with(key));
        let legacy = legacy();
        if !sets("indent") {
            config.indent = legacy.indent;
        }
        if !sets("scope") {
            config.align.scope = legacy.align.scope;
        }
        if !sets("semicolon") {
            config.align.semicolon = legacy.align.semicolon;
        }
    }
    config
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
            legacy(),
            config("use_tabs = true\nindent = 4\nmax_width = 40"),
            config(
                "interface_list = \"preserve\"\nassociation_list = \"expand\"\nclose_paren = \"attached\"",
            ),
            config("[format.align]\nblock_boundary = \"never\""),
            config("style = \"leading-comma\""),
            config("style = \"leading-comma\"\nmax_width = 40\nassociation_list = \"preserve\""),
            config("style = \"compact\""),
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
    let edits = |config: &FormatConfig| {
        let edits = format_ranges_verilog(
            text,
            std::slice::from_ref(&(at..at)),
            config,
            &Options {
                standard: Standard::Sv2017,
                ..Options::default()
            },
        )
        .unwrap();
        apply_edits(text, &edits)
    };
    assert_eq!(
        edits(&legacy()),
        "module m;\n  assign a = b;\n  assign c = d;\n\nassign e=f;\nendmodule\n"
    );
    // The `=` of the whole module line up.
    assert_eq!(
        edits(&FormatConfig::default()),
        "module m;\n    assign a = b;\n    assign c = d;\n\n    assign e = f;\nendmodule\n"
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

#[test]
fn leading_separators() {
    let text =
        "module m (input a, // first\n  output b);\n  sub u (.x(a),\n    .y(b));\nendmodule\n";
    assert_eq!(
        fmt_with(text, &config("style = \"leading-comma\"")),
        "module m (\n    input  a // first\n  , output b\n);\n    sub u (\n        .x(a)\n      , .y(b)\n    );\nendmodule\n"
    );
}

#[test]
fn leading_separators_keep_empty_arguments_together() {
    let text = "module m;\n  initial x = create(\"a_long_name_for_this_component\",, get_full_name());\nendmodule\n";
    let out = fmt_with(text, &config("style = \"leading-comma\"\nmax_width = 40"));
    assert!(out.contains(",, get_full_name()"), "{out}");
}

/// Found by the corpus (cva6): an empty argument of a broken-up call went to
/// a line of its own, and back on the line before the second time.
#[test]
fn trailing_separators_keep_empty_arguments_together() {
    let text = "module m;\n  initial x = create(\"a_long_name_for_this_component\",, get_full_name());\nendmodule\n";
    let out = fmt_with(text, &config("max_width = 40"));
    assert!(out.contains("component\",,\n"), "{out}");
}

/// Found by the `format_verilog` fuzz target: a `\` before a lone CR read as
/// CR LF once the formatter put a newline after it.
#[test]
fn a_continuation_before_a_lone_cr() {
    assert_eq!(fmt("`define W \\\r"), fmt("`define W \\\n"));
    let text = "`define F(q) \\\r\r\rmodule m; endmodule\n";
    assert_eq!(fmt(text), fmt(&text.replace('\r', "\n")));
}

/// Found by the `format_verilog` fuzz target: the line break after an empty
/// first element went to the `)`, and back the second time.
#[test]
fn an_empty_first_element_stays_after_its_paren() {
    let text = "module top;\n  counter #(,) u0 (.clk(clk), .q(q));\nendmodule\n";
    let out = fmt_with(
        text,
        &config("association_list = \"expand\"\nclose_paren = \"attached\""),
    );
    assert!(out.contains("#(\n    ,) u0 ("), "{out}");
    let out = fmt_with(text, &config("association_list = \"expand\""));
    assert!(out.contains("#(\n    ,\n  ) u0 ("), "{out}");
}

/// Found by the `format_verilog` fuzz target: range formatting cut a
/// directive continued by a `\` at the end of the file before its line
/// break.
#[test]
fn a_range_ending_in_a_continued_directive() {
    let text = "`x`define F s\\\n";
    let options = Options::default();
    let edits =
        format_ranges_verilog(text, std::slice::from_ref(&(0..0)), &legacy(), &options).unwrap();
    let ranged = apply_edits(text, &edits);
    assert_eq!(ranged, "`x\n  `define F s\\\n");
    assert_eq!(
        format_verilog(&ranged, &legacy(), &options).unwrap(),
        fmt(text)
    );
}

/// `text` after formatting the line at byte `at` alone.
fn fmt_line(text: &str, at: usize) -> String {
    let edits = format_ranges_verilog(
        text,
        std::slice::from_ref(&(at..at)),
        &legacy(),
        &Options::default(),
    )
    .unwrap_or_else(|e| panic!("{e}"));
    apply_edits(text, &edits)
}

/// Found by the `format_verilog` fuzz target: code that is not compiled lost
/// its trailing blanks, all of it when it was nothing else, and range
/// formatting then lexed it differently.
#[test]
fn code_not_compiled_keeps_its_blanks() {
    let text = "module m;\n`ifdef B `endif\nendmodule\n";
    assert_eq!(fmt(text), text);
    let text = "module m;\n`ifdef B ;\u{a0}`endif\nendmodule\n";
    assert_eq!(fmt(text), text);
    assert_eq!(fmt_line(text, 10), text);
}

/// Found by the `format_verilog` fuzz target: a directive right after an
/// `` `ifdef `` whose branch is not compiled went to a line of its own,
/// which made the line break code that is not compiled.
#[test]
fn a_directive_closing_an_empty_branch_stays_on_its_line() {
    assert_eq!(
        fmt("module m;\n`ifdef e`endif\nendmodule"),
        "module m;\n`ifdef e`endif\nendmodule\n"
    );
    assert_eq!(fmt("`ifdef S`elsif i`endif"), "`ifdef S`elsif i`endif\n");
    assert_eq!(
        fmt("module a;\n`ifdef O`else initial;\n`endif\nendmodule"),
        "module a;\n`ifdef O`else\n  initial;\n`endif\nendmodule\n"
    );
}

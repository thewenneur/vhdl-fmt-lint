//! The rules of the file as a whole: file names, `` `default_nettype ``,
//! block labels, lines, and files that do not parse.

use super::{check, check_files, lint_with, messages};

#[test]
fn file_names_match_their_unit() {
    check_files(
        &[
            ("counter.sv", "module counter;\nendmodule\n"),
            // One of the units matches.
            (
                "pair.sv",
                "module helper;\nendmodule\nmodule pair;\nendmodule\n",
            ),
            (
                "pkg.sv",
                "package other_pkg; // expect: style/file-name\nendpackage\n",
            ),
            // Nested units do not count.
            (
                "outer.sv",
                "module outer;\n    module inner;\n    endmodule\nendmodule\n",
            ),
            // Nothing declared, nothing to match.
            ("defs.svh", "localparam int N = 4;\n"),
        ],
        &["style/file-name"],
    );
    let found = lint_with(
        &[("wrong.v", "module right;\nendmodule\n")],
        &["style/file-name"],
        "",
    );
    assert_eq!(
        found[0].0.message,
        "file name does not match the unit it declares; expected `right.v`"
    );
}

#[test]
fn default_nettype_none_before_each_module() {
    check(
        "module before_directive; // expect: verilog/default-nettype
endmodule
`default_nettype none
module after_directive;
endmodule
`resetall
module after_reset; // expect: verilog/default-nettype
endmodule
`default_nettype wire
module after_wire; // expect: verilog/default-nettype
endmodule
`default_nettype none
package p;
endpackage
",
        &["verilog/default-nettype"],
    );
}

#[test]
fn always_blocks_have_labels() {
    check(
        "module m (input logic clk, input logic d, output logic q, output logic y);
    always_ff @(posedge clk) begin : regs
        q <= d;
    end
    always_ff @(posedge clk) begin // expect: style/process-label
        q <= d;
    end
    always_comb y = d; // expect: style/process-label
    always @* begin : comb
        y = d;
    end
    // Only `always` blocks.
    initial begin
        q = 0;
    end
endmodule
",
        &["style/process-label"],
    );
    assert_eq!(
        messages(
            "module m;\n    always_latch begin\n    end\nendmodule\n",
            &["style/process-label"]
        ),
        ["`always_latch` block without a label"]
    );
}

#[test]
fn line_rules_apply_to_verilog() {
    let long = format!(
        "module m;\n    // {} // expect: style/line-length\nendmodule // expect: style/trailing-whitespace \n",
        "x".repeat(70)
    );
    check(&long, &["style/line-length", "style/trailing-whitespace"]);
    check(
        "module m;\n\tlogic a; // expect: style/tabs\nendmodule\n",
        &["style/tabs"],
    );
}

#[test]
fn rules_of_meaning_wait_for_the_file_to_parse() {
    let text = "module m (input logic a, output logic y);
    assign y = missing
    always_ff @(posedge a) begin
    end
endmodule
";
    let found = super::lint(
        text,
        &[
            "sema/unresolved-name",
            "style/process-label",
            "sema/unused-port",
        ],
    );
    let rules: Vec<&str> = found.iter().map(|(f, _)| f.rule.as_str()).collect();
    // Labels are still checked: they need no meaning.
    assert_eq!(rules, ["style/process-label"]);
}

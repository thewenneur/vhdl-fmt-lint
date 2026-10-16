//! Declarations and uses.

use super::{check, check_files, fixed, lint_with, messages};

const USAGE: &[&str] = &[
    "sema/unused-signal",
    "sema/unread-signal",
    "sema/undriven-signal",
    "sema/unused-port",
    "sema/unused-generic",
];

#[test]
fn names_nothing_declares() {
    check(
        "`default_nettype none
module m (input logic a, output logic y);
    assign y = a & b; // expect: sema/unresolved-name
    assign y = $clog2(4);
    // `item` in the `with` clause of an array method.
    int q [$];
    int r [$];
    assign r = q.find with (item > 0);
endmodule
",
        &["sema/unresolved-name"],
    );
}

#[test]
fn names_from_packages_and_other_files() {
    check_files(
        &[
            (
                "pkg.sv",
                "package pkg;
    localparam int WIDTH = 8;
endpackage
",
            ),
            (
                "globals.sv",
                "localparam int DEPTH = 4;
",
            ),
            (
                "m.sv",
                "`default_nettype none
module m import pkg::*; (input logic [WIDTH-1:0] a, output logic [WIDTH-1:0] y);
    assign y = a + DEPTH + MISSING; // expect: sema/unresolved-name
endmodule
",
            ),
        ],
        &["sema/unresolved-name"],
    );
}

#[test]
fn a_package_the_project_lacks_may_declare_anything() {
    check(
        "`default_nettype none
module m import vendor_pkg::*; (input logic a, output logic y);
    assign y = a & VENDOR_CONSTANT;
endmodule
",
        &["sema/unresolved-name"],
    );
}

#[test]
fn implicit_nets() {
    check(
        "module m (input logic a, output logic y);
    sub u (.a(a), .y(wire_y)); // expect: verilog/implicit-net
    assign other = a; // expect: verilog/implicit-net
    assign y = wire_y & other;
endmodule
module sub (input logic a, output logic y);
    assign y = a;
endmodule
",
        &["verilog/implicit-net", "sema/unresolved-name"],
    );
    // With `default_nettype none` they are names nothing declares.
    check(
        "`default_nettype none
module m (input logic a);
    assign other = a; // expect: sema/unresolved-name
endmodule
",
        &["verilog/implicit-net", "sema/unresolved-name"],
    );
    assert_eq!(
        messages(
            "module m (input logic a);\n    assign other = a;\nendmodule\n",
            &["verilog/implicit-net"]
        ),
        ["`other` is not declared, so it is an implicit 1-bit wire"]
    );
}

#[test]
fn names_declared_twice() {
    check(
        "module m (a, q);
    parameter bit A = 1;
    input a;
    // A port declared again with its type.
    output q;
    reg q;
    logic x;
    logic x; // expect: sema/duplicate-declaration
    // Alternatives of a generate never coexist.
    if (A) begin : g
        sub u (.a(a));
    end else begin : g
        sub u (.a(a));
    end
    assign q = x;
endmodule
",
        &["sema/duplicate-declaration"],
    );
    assert_eq!(
        messages(
            "module m;\n    logic x;\n    logic x;\nendmodule\n",
            &["sema/duplicate-declaration"]
        ),
        ["`x` is already declared at line 2"]
    );
}

#[test]
fn signals_never_used_read_or_driven() {
    check(
        "module m #(parameter int USED = 1) (input logic a, output logic y);
    logic idle; // expect: sema/unused-signal
    logic sink; // expect: sema/unread-signal
    logic source; // expect: sema/undriven-signal
    logic both;
    logic init = 1'b0;
    logic unused_ok;
    supply1 vdd;
    assign sink = a;
    assign both = a;
    assign y = both & source & init & vdd & USED;
endmodule
",
        USAGE,
    );
}

#[test]
fn unused_ports_and_parameters() {
    let text = "module m #(parameter int SPARE = 2) (input logic a, input logic b, output logic y, output logic z);
    assign y = a;
endmodule
";
    assert_eq!(
        messages(text, USAGE),
        [
            "output port `z` is never assigned",
            "parameter `SPARE` is never used",
            "port `b` is never used"
        ]
    );
    let found = lint_with(&[("test.sv", text)], USAGE, "");
    let rules: Vec<&str> = found.iter().map(|(f, _)| f.rule.as_str()).collect();
    assert_eq!(
        rules,
        [
            "sema/undriven-signal",
            "sema/unused-generic",
            "sema/unused-port"
        ]
    );
}

#[test]
fn usage_through_connections() {
    check_files(
        &[
            (
                "sub.sv",
                "module sub (input logic a, output logic y);
    assign y = a;
endmodule
",
            ),
            (
                "m.sv",
                "module m (input logic a, output logic y, output logic z);
    logic b;
    assign b = a;
    sub u1 (.a(b), .y(y));
    // `.name` and `.*` connect signals of the same name.
    sub u2 (.a, .y(z));
endmodule
module n (input logic a, output logic y);
    sub u (.*);
endmodule
",
            ),
        ],
        USAGE,
    );
}

#[test]
fn a_wildcard_to_an_unknown_module_may_use_anything() {
    check(
        "module m (input logic a, output logic y);
    logic spare;
    unknown u (.*);
endmodule
",
        USAGE,
    );
}

#[test]
fn included_code_may_use_anything() {
    check(
        "module m (input logic a, output logic y);
    logic spare;
    `include \"body.svh\"
endmodule
",
        USAGE,
    );
}

#[test]
fn removing_an_unused_signal() {
    let text = "module m (input logic a, output logic y);
    logic idle;
    assign y = a;
endmodule
";
    assert_eq!(
        fixed(text, "sema/unused-signal"),
        text.replace("    logic idle;\n", "")
    );
    // Something else on the line stays.
    let shared = "module m (input logic a, output logic y);
    logic idle; assign y = a;
endmodule
";
    assert_eq!(
        fixed(shared, "sema/unused-signal"),
        shared.replace("logic idle; ", " ")
    );
    // A declaration of several names is not removed.
    let several = "module m (input logic a, output logic y);
    logic idle, busy;
    assign y = a & busy;
    assign busy = a;
endmodule
";
    let found = super::lint(several, &["sema/unused-signal"]);
    assert_eq!(found.len(), 1);
    assert!(found[0].1.fix.is_none());
}

#[test]
fn naming_conventions() {
    let text = "module Top #(parameter int width = 8) (input logic clk_i, input logic rst, output logic q_o);
    localparam int Depth = 4;
    logic s_ok, Bad;
    always_ff @(posedge clk_i) begin : p_regs
        q_o <= Bad;
    end
    always_ff @(posedge clk_i) begin : regs
        Bad <= rst;
    end
    assign s_ok = 1'b0;
endmodule
";
    let lint = r#"[rules]
"naming/module" = { case = "snake" }
"naming/parameter" = { case = "upper" }
"naming/constant" = { prefix = "C_" }
"naming/input-port" = { suffix = "_i" }
"naming/output-port" = { suffix = "_o" }
"naming/signal" = { prefix = "s_" }
"naming/process" = { prefix = "p_" }
"#;
    let rules = [
        "naming/module",
        "naming/parameter",
        "naming/constant",
        "naming/input-port",
        "naming/output-port",
        "naming/signal",
        "naming/process",
    ];
    let found = lint_with(&[("test.sv", text)], &rules, lint);
    let found: Vec<(usize, &str, &str)> = found
        .iter()
        .map(|(f, _)| (f.line, f.rule.as_str(), f.message.as_str()))
        .collect();
    assert_eq!(
        found,
        [
            (1, "naming/input-port", "port `rst` should end with `_i`"),
            (1, "naming/module", "module `Top` should be snake case"),
            (
                1,
                "naming/parameter",
                "parameter `width` should be upper case"
            ),
            (
                2,
                "naming/constant",
                "constant `Depth` should start with `C_`"
            ),
            (3, "naming/signal", "signal `Bad` should start with `s_`"),
            (7, "naming/process", "label `regs` should start with `p_`"),
        ]
    );
}

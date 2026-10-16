//! Instances: connections by position, ports and parameters that do not
//! exist, unconnected inputs and connection widths.

use super::{check_files, lint_with};

const LEAF: &str = "module leaf #(parameter int W = 8, parameter bit SIGNED = 0) (
    input  logic [7:0] a,
    input  logic       en,
    input  logic       opt = 1'b0,
    output logic [7:0] y
);
    assign y = en ? a : 8'd0;
endmodule
";

const RULES: &[&str] = &[
    "style/positional-association",
    "sema/unresolved-name",
    "verilog/unconnected-input",
    "sema/width-mismatch",
];

fn check(top: &str) {
    check_files(&[("leaf.sv", LEAF), ("top.sv", top)], RULES);
}

#[test]
fn connections_by_name_are_checked_against_the_module() {
    check(
        "module top (input logic [7:0] a, input logic e, output logic [7:0] y);
    leaf u_ok (.a(a), .en(e), .y(y));
    leaf u_bad (.a(a), .en(e), .y(y), .nope(e)); // expect: sema/unresolved-name
    leaf #(.W(8), .DEPTH(4)) u_param (.a(a), .en(e), .y(y)); // expect: sema/unresolved-name
endmodule
",
    );
}

#[test]
fn inputs_left_unconnected() {
    check(
        "module top (input logic [7:0] a, input logic e, output logic [7:0] y);
    leaf u_missing (.a(a), .y(y)); // expect: verilog/unconnected-input
    leaf u_empty (.a(a), .en(), .y(y)); // expect: verilog/unconnected-input
    // An input with a default, and an output, may be left out.
    leaf u_default (.a(a), .en(e));
    // `.*` connects every port of the same name.
    leaf u_star (.*);
    // No connections at all is on purpose.
    leaf u_none ();
endmodule
",
    );
}

#[test]
fn unconnected_messages() {
    let top = "module top (input logic [7:0] a, output logic [7:0] y);
    leaf u_missing (.y(y));
    leaf u_empty (.a(a), .en(), .y(y));
endmodule
";
    let found = lint_with(&[("leaf.sv", LEAF), ("top.sv", top)], RULES, "");
    let messages: Vec<&str> = found.iter().map(|(f, _)| f.message.as_str()).collect();
    assert_eq!(
        messages,
        [
            "input ports `a`, `en` of `leaf` are not connected",
            "input port `en` of `leaf` is connected to nothing"
        ]
    );
}

#[test]
fn an_empty_position_leaves_its_input_unconnected() {
    check(
        "module top (input logic [7:0] a, input logic opt, output logic [7:0] y);
    leaf u_gap (a, , opt, y); // expect: style/positional-association, verilog/unconnected-input
endmodule
",
    );
}

#[test]
fn connections_by_position() {
    check(
        "module top (input logic [7:0] a, input logic e, output logic [7:0] y, output logic g);
    leaf #(8) u_pos (a, e, 1'b0, y); // expect: style/positional-association, style/positional-association
    // Gates are positional by definition.
    and g1 (g, e, e);
    // A module the project does not have is still positional.
    unknown u_unknown (a, y); // expect: style/positional-association
endmodule
",
    );
}

#[test]
fn positional_fix_names_the_ports() {
    let top = "module top (input logic [7:0] a, input logic e, output logic [7:0] y);
    leaf u (a, e, 1'b0, y);
endmodule
";
    let found = lint_with(
        &[("leaf.sv", LEAF), ("top.sv", top)],
        &["style/positional-association"],
        "",
    );
    let [(found, finding)] = found.as_slice() else {
        panic!("{found:#?}");
    };
    assert_eq!(found.message, "ports are connected by position; name them");
    let fix = finding.fix.as_ref().unwrap();
    assert!(fix.safe);
    assert_eq!(
        crate::apply_fixes(top, [fix]).0,
        top.replace("(a, e, 1'b0, y)", "(.a(a), .en(e), .opt(1'b0), .y(y))")
    );
}

#[test]
fn positional_without_the_module_has_no_fix() {
    let top = "module top (input logic a, output logic y);
    unknown u (a, y);
endmodule
";
    let found = lint_with(&[("top.sv", top)], &["style/positional-association"], "");
    assert_eq!(found.len(), 1);
    assert!(found[0].1.fix.is_none());
}

#[test]
fn connection_widths() {
    check(
        "module top (input logic [3:0] n, input logic [7:0] a, input logic [15:0] w, input logic e, output logic [7:0] y, output logic [3:0] z);
    leaf u_narrow (.a(n), .en(e), .y(y)); // expect: sema/width-mismatch
    leaf u_wide (.a(w), .en(e), .y(y)); // expect: sema/width-mismatch
    leaf u_out (.a(a), .en(e), .y(z)); // expect: sema/width-mismatch
    // Arithmetic may be meant to grow.
    leaf u_sum (.a(n + n), .en(e), .y(y));
    // A one-bit flag extended on purpose.
    leaf u_flag (.a(e), .en(e), .y(y));
    leaf u_ok (.a(a), .en(e), .y(y));
endmodule
",
    );
}

#[test]
fn width_message_names_the_port() {
    let top = "module top (input logic [3:0] n, input logic e, output logic [7:0] y);
    leaf u (.a(n), .en(e), .y(y));
endmodule
";
    let found = lint_with(
        &[("leaf.sv", LEAF), ("top.sv", top)],
        &["sema/width-mismatch"],
        "",
    );
    assert_eq!(found.len(), 1);
    assert_eq!(
        found[0].0.message,
        "port `a` of `leaf` has 8 bits but is connected to 4"
    );
}

#[test]
fn a_module_defined_twice_is_not_looked_up() {
    let other = LEAF
        .replace("input  logic       en,\n", "")
        .replace("en ? a : 8'd0", "a");
    check_files(
        &[
            ("leaf.sv", LEAF),
            ("leaf2.sv", &other),
            (
                "top.sv",
                "module top (input logic [7:0] a, output logic [7:0] y);
    leaf u (.a(a), .y(y), .nope(a));
endmodule
",
            ),
        ],
        &["sema/unresolved-name", "verilog/unconnected-input"],
    );
}

#[test]
fn positional_fix_of_parameters() {
    let top = "module top (input logic [7:0] a, input logic e, output logic [7:0] y);
    leaf #(8, 1) u (.a(a), .en(e), .y(y));
endmodule
";
    let found = lint_with(
        &[("leaf.sv", LEAF), ("top.sv", top)],
        &["style/positional-association"],
        "",
    );
    assert_eq!(found.len(), 1);
    assert_eq!(
        found[0].0.message,
        "parameters are connected by position; name them"
    );
    let fix = found[0].1.fix.as_ref().unwrap();
    assert_eq!(
        crate::apply_fixes(top, [fix]).0,
        top.replace("#(8, 1)", "#(.W(8), .SIGNED(1))")
    );
}

#[test]
fn positional_fix_names_an_empty_position() {
    let top = "module top (input logic [7:0] a, output logic [7:0] y);
    leaf u (a, , 1'b0, y);
endmodule
";
    let found = lint_with(
        &[("leaf.sv", LEAF), ("top.sv", top)],
        &["style/positional-association"],
        "",
    );
    let fix = found[0].1.fix.as_ref().unwrap();
    assert_eq!(
        crate::apply_fixes(top, [fix]).0,
        top.replace("(a, , 1'b0, y)", "(.a(a), .en(), .opt(1'b0), .y(y))")
    );
    let first = top.replace("(a, , 1'b0, y)", "(, a[0], 1'b0, y)");
    let found = lint_with(
        &[("leaf.sv", LEAF), ("top.sv", &first)],
        &["style/positional-association"],
        "",
    );
    let fix = found[0].1.fix.as_ref().unwrap();
    assert_eq!(
        crate::apply_fixes(&first, [fix]).0,
        top.replace("(a, , 1'b0, y)", "(.a(), .en(a[0]), .opt(1'b0), .y(y))")
    );
    // A comment in the empty position is not dropped.
    let comment = top.replace("(a, , 1'b0, y)", "(a, /* none */, 1'b0, y)");
    let found = lint_with(
        &[("leaf.sv", LEAF), ("top.sv", &comment)],
        &["style/positional-association"],
        "",
    );
    assert!(found[0].1.fix.is_none());
}

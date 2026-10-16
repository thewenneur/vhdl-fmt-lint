//! `always` blocks: assignments, latches, sensitivity lists, `case`
//! defaults, resets and drivers.

use super::{check, fixed, messages};

const BLOCKING: &[&str] = &["verilog/blocking-in-clocked"];
const NONBLOCKING: &[&str] = &["verilog/nonblocking-in-combinational"];
const LATCH: &[&str] = &["sema/inferred-latch"];
const SENSITIVITY: &[&str] = &["sema/sensitivity-list", "sema/unneeded-sensitivity"];
const CASE_DEFAULT: &[&str] = &["verilog/case-default"];
const RESETS: &[&str] = &[
    "sema/missing-reset",
    "sema/mixed-reset",
    "sema/reset-polarity",
];
const DRIVERS: &[&str] = &["verilog/multiple-drivers"];

#[test]
fn blocking_assignments_in_clocked_blocks() {
    check(
        "module m (input logic clk, input logic [7:0] d, output logic [7:0] q, output logic [7:0] r);
    logic [7:0] t;
    always_ff @(posedge clk) begin
        q = d; // expect: verilog/blocking-in-clocked
        r <= d;
    end
    // A temporary nothing outside the block reads.
    always @(posedge clk) begin
        t = d + 1;
        r <= t;
    end
    // A variable of the block itself.
    always @(posedge clk) begin : named
        logic [7:0] v;
        v = d;
        q <= v;
    end
    // Blocking assignments are what combinational blocks use.
    always_comb r = d;
    // A procedure on levels and edges is not a clocked block.
    initial q = 0;
endmodule
",
        BLOCKING,
    );
}

#[test]
fn a_temporary_read_outside_its_block_is_a_signal() {
    check(
        "module m (input logic clk, input logic [7:0] d, output logic [7:0] q);
    logic [7:0] t;
    always @(posedge clk) t = d; // expect: verilog/blocking-in-clocked
    assign q = t;
endmodule
",
        BLOCKING,
    );
}

#[test]
fn blocking_fix_uses_a_nonblocking_assignment() {
    let text = "module m (input logic clk, input logic d, output logic q);
    always @(posedge clk) q = d;
endmodule
";
    assert_eq!(
        fixed(text, "verilog/blocking-in-clocked"),
        text.replace("q = d", "q <= d")
    );
    assert_eq!(
        messages(text, BLOCKING),
        ["blocking assignment to `q` in a clocked `always` block; use `<=`"]
    );
}

#[test]
fn nonblocking_assignments_in_combinational_blocks() {
    let text =
        "module m (input logic a, input logic b, output logic y, output logic z, output logic w);
    always @* y <= a; // expect: verilog/nonblocking-in-combinational
    always_comb z <= a & b; // expect: verilog/nonblocking-in-combinational
    always @(a or b) w <= a | b; // expect: verilog/nonblocking-in-combinational
    always @(posedge a) w <= b;
    always_latch if (a) y <= b;
endmodule
";
    check(text, NONBLOCKING);
    let one = "module m (input logic a, output logic y);
    always @(*) y <= a;
endmodule
";
    assert_eq!(fixed(one, NONBLOCKING[0]), one.replace("y <= a", "y = a"));
    assert_eq!(
        messages(one, NONBLOCKING),
        ["non-blocking assignment in a combinational `always` block; use `=`"]
    );
}

#[test]
fn latches_from_paths_that_do_not_assign() {
    check(
        "module m (input logic en, input logic [1:0] sel, input logic [7:0] d, output logic [7:0] a, output logic [7:0] b, output logic [7:0] c, output logic [7:0] e, output logic [7:0] f);
    always_comb begin
        if (en) a = d; // expect: sema/inferred-latch
    end
    // Assigned first, then changed on some paths.
    always_comb begin
        b = 0;
        if (en) b = d;
    end
    // Both branches.
    always_comb begin
        if (en) c = d;
        else c = ~d;
    end
    // A `case` without a `default` that does not cover every value.
    always_comb begin
        case (sel)
            2'd0: e = d; // expect: sema/inferred-latch
            2'd1: e = ~d;
        endcase
    end
    // Every value of two bits.
    always_comb begin
        case (sel)
            2'd0: f = d;
            2'd1: f = ~d;
            2'd2: f = d + 1;
            2'd3: f = d - 1;
        endcase
    end
endmodule
",
        LATCH,
    );
}

#[test]
fn latches_are_reported_once_per_variable() {
    let text = "module m (input logic a, input logic b, input logic [7:0] d, output logic [7:0] y);
    always @* begin
        if (a) y = d;
        if (b) y = ~d;
    end
endmodule
";
    assert_eq!(
        messages(text, LATCH),
        [
            "`y` is not assigned on every path through this combinational block, which infers a latch"
        ]
    );
}

#[test]
fn what_does_not_infer_a_latch() {
    check(
        "module m (input logic en, input logic [1:0] sel, input logic [7:0] d, output logic [7:0] a, output logic [7:0] b, output logic [7:0] c, output logic [7:0] e);
    parameter bit FEATURE = 1;
    logic [7:0] t;
    // `unique` and `priority` cases cover every value by definition.
    always_comb begin
        unique case (sel)
            2'd0: a = d;
            2'd1: a = ~d;
        endcase
    end
    always_comb begin
        priority case (sel)
            2'd0: b = d;
        endcase
    end
    // A full_case directive.
    always_comb begin
        case (sel) // synopsys full_case
            2'd0: c = d;
        endcase
    end
    // Elaboration keeps one branch of an `if` on a parameter.
    always_comb begin
        if (FEATURE) e = d;
    end
    // A temporary only the block reads.
    always_comb begin
        if (en) t = d;
    end
    // `always_latch` asks for a latch.
    always_latch if (en) a = d;
endmodule
",
        LATCH,
    );
}

#[test]
fn a_loop_is_taken_to_run() {
    check(
        "module m (input logic [7:0] d, output logic [7:0] y);
    always_comb begin
        for (int i = 0; i < 8; i++) y[i] = d[7 - i];
    end
endmodule
",
        LATCH,
    );
}

#[test]
fn a_case_on_an_enumeration_lists_its_members() {
    check(
        "module m (input logic [7:0] d, output logic [7:0] y, output logic [7:0] z);
    typedef enum logic [1:0] {A, B, C} abc_t;
    abc_t s;
    always_comb begin
        case (s)
            A: y = d;
            B: y = ~d;
            C: y = 0;
        endcase
    end
    always_comb begin
        case (s) // expect: verilog/case-default
            A: z = d; // expect: sema/inferred-latch
            B: z = ~d;
        endcase
    end
endmodule
",
        &["sema/inferred-latch", "verilog/case-default"],
    );
}

#[test]
fn a_macro_may_assign_anything() {
    check(
        "module m (input logic en, input logic [7:0] d, output logic [7:0] y);
    always_comb begin
        if (en) y = d;
        `ASSIGN_DEFAULTS
    end
endmodule
",
        LATCH,
    );
}

#[test]
fn sensitivity_lists_name_what_the_block_reads() {
    check(
        "module m (input logic a, input logic b, input logic c, output logic y, output logic z, output logic w);
    always @(a) y = a & b; // expect: sema/sensitivity-list
    always @(a or b or c) z = a | b; // expect: sema/unneeded-sensitivity
    always @(a, b) w = a ^ b;
    // `@*` reads what it needs.
    always @* y = c;
endmodule
",
        SENSITIVITY,
    );
}

#[test]
fn sensitivity_findings_and_fixes() {
    let missing = "module m (input logic a, input logic b, input logic c, output logic y);
    always @(a) y = a & b & c;
endmodule
";
    assert_eq!(
        messages(missing, SENSITIVITY),
        ["the block reads `b`, `c` but its sensitivity list does not name them"]
    );
    assert_eq!(
        fixed(missing, "sema/sensitivity-list"),
        missing.replace("@(a)", "@(a or b or c)")
    );
    let commas = missing.replace("@(a)", "@(a, b)");
    assert_eq!(
        fixed(&commas, "sema/sensitivity-list"),
        missing.replace("@(a)", "@(a, b, c)")
    );
    let extra = "module m (input logic a, input logic b, input logic c, output logic y);
    always @(a or b or c) y = a;
endmodule
";
    assert_eq!(
        messages(extra, &["sema/unneeded-sensitivity"]),
        [
            "`b` is in the sensitivity list but the block never reads it",
            "`c` is in the sensitivity list but the block never reads it"
        ]
    );
    let last = "module m (input logic a, input logic b, output logic y);
    always @(a or b) y = a;
endmodule
";
    assert_eq!(
        fixed(last, "sema/unneeded-sensitivity"),
        last.replace("@(a or b)", "@(a)")
    );
    let first = last.replace("y = a", "y = b");
    assert_eq!(
        fixed(&first, "sema/unneeded-sensitivity"),
        last.replace("@(a or b)", "@(b)").replace("y = a", "y = b")
    );
}

#[test]
fn the_only_signal_of_a_list_is_not_removed() {
    let text = "module m (input logic a, input logic b, output logic y);
    always @(a) y = b;
endmodule
";
    let found = super::lint(text, &["sema/unneeded-sensitivity"]);
    assert_eq!(found.len(), 1);
    assert!(found[0].1.fix.is_none());
}

#[test]
fn sensitivity_ignores_locals_and_what_the_block_writes() {
    check(
        "module m (input logic a, input logic [7:0] d, output logic [7:0] y);
    logic [7:0] mem [0:3];
    always @(a or d) begin : comb
        logic [7:0] t;
        t = d;
        y = a ? t : mem[0];
    end
endmodule
",
        SENSITIVITY,
    );
}

#[test]
fn case_statements_without_default_in_combinational_blocks() {
    check(
        "module m (input logic [1:0] sel, input logic clk, input logic [7:0] d, output logic [7:0] y, output logic [7:0] q);
    always_comb begin
        y = 0;
        case (sel) // expect: verilog/case-default
            2'd0: y = d;
        endcase
        casez (sel) // expect: verilog/case-default
            2'b1?: y = d;
        endcase
        case (sel)
            2'd0: y = d;
            default: y = 0;
        endcase
        unique0 case (sel)
            2'd1: y = d;
        endcase
    end
    // Only combinational blocks.
    always_ff @(posedge clk) begin
        case (sel)
            2'd0: q <= d;
        endcase
    end
endmodule
",
        CASE_DEFAULT,
    );
}

#[test]
fn a_case_of_sized_labels_covering_a_parameter_width() {
    check(
        "module m #(parameter int W = 2) (input logic [W-1:0] sel, output logic [7:0] y);
    always_comb begin
        case (sel)
            2'd0: y = 1;
            2'd1: y = 2;
            2'd2: y = 3;
            2'd3: y = 4;
        endcase
    end
endmodule
",
        &["verilog/case-default", "sema/inferred-latch"],
    );
}

#[test]
fn registers_left_out_of_the_reset() {
    check(
        "module m (input logic clk, input logic rst_n, input logic rst, input logic [7:0] d, output logic [7:0] a, output logic [7:0] b, output logic [7:0] c, output logic [7:0] e);
    logic [7:0] mem [0:3];
    always_ff @(posedge clk or negedge rst_n) begin
        if (!rst_n) begin
            a <= 0;
        end else begin
            a <= d;
            b <= d; // expect: sema/missing-reset
            b <= ~d;
            mem[0] <= d;
        end
    end
    // A synchronous reset.
    always_ff @(posedge clk) begin
        if (rst) c <= 0;
        else begin
            c <= d;
            e <= d; // expect: sema/missing-reset
        end
    end
endmodule
",
        RESETS,
    );
}

#[test]
fn an_enable_is_not_a_reset() {
    check(
        "module m (input logic clk, input logic en, input logic resetn, input logic [7:0] d, output logic [7:0] q, output logic [7:0] r);
    // Not named as a reset.
    always_ff @(posedge clk) begin
        if (en) q <= 0;
        else q <= d;
    end
    // Named as one, but the branch does other work than set constants.
    always_ff @(posedge clk) begin
        if (resetn) r <= d;
        else r <= 0;
    end
endmodule
",
        RESETS,
    );
}

#[test]
fn reset_polarity_against_the_edge_and_the_name() {
    check(
        "module m (input logic clk, input logic rst_n, input logic srst_n, input logic rst, input logic arst, input logic [7:0] d, output logic [7:0] a, output logic [7:0] b, output logic [7:0] c, output logic [7:0] e);
    always_ff @(posedge clk or negedge arst) begin
        if (arst) a <= 0; // expect: sema/reset-polarity
        else a <= d;
    end
    always_ff @(posedge clk or posedge rst) begin
        if (rst == 1'b0) b <= 0; // expect: sema/reset-polarity
        else b <= d;
    end
    // Named as active low, tested high.
    always_ff @(posedge clk) begin
        if (srst_n) c <= 0; // expect: sema/reset-polarity
        else c <= d;
    end
    // Right.
    always_ff @(posedge clk or negedge rst_n) begin
        if (rst_n == 0) e <= 0;
        else e <= d;
    end
endmodule
",
        RESETS,
    );
}

#[test]
fn reset_polarity_messages() {
    let text = "module m (input logic clk, input logic arst, input logic rst_ni, input logic [7:0] d, output logic [7:0] a, output logic [7:0] b);
    always_ff @(posedge clk or negedge arst) begin
        if (arst) a <= 0;
        else a <= d;
    end
    always_ff @(posedge clk) begin
        if (rst_ni) b <= 0;
        else b <= d;
    end
endmodule
";
    assert_eq!(
        messages(text, &["sema/reset-polarity"]),
        [
            "`arst` is listed with `negedge` but resets when high here",
            "`rst_ni` is named as active low but resets when high here"
        ]
    );
}

#[test]
fn a_reset_used_two_ways_in_a_unit() {
    let text = "module m (input logic clk, input logic rst, input logic [7:0] d, output logic [7:0] a, output logic [7:0] b, output logic [7:0] c);
    always_ff @(posedge clk or posedge rst) begin
        if (rst) a <= 0;
        else a <= d;
    end
    always_ff @(posedge clk) begin
        if (rst) b <= 0; // expect: sema/mixed-reset
        else b <= d;
    end
    always_ff @(posedge clk) begin
        if (!rst) c <= 0; // expect: sema/reset-polarity
        else c <= d;
    end
endmodule
";
    check(text, RESETS);
    assert_eq!(
        messages(text, RESETS),
        [
            "`rst` resets this block synchronously but the block at line 3 asynchronously",
            "`rst` resets when low here but when high at line 3"
        ]
    );
}

#[test]
fn generate_alternatives_may_reset_differently() {
    check(
        "module m #(parameter bit ASYNC = 1) (input logic clk, input logic rst, input logic [7:0] d, output logic [7:0] q);
    if (ASYNC) begin : g_async
        always_ff @(posedge clk or posedge rst) begin
            if (rst) q <= 0;
            else q <= d;
        end
    end else begin : g_sync
        always_ff @(posedge clk) begin
            if (rst) q <= 0;
            else q <= d;
        end
    end
endmodule
",
        RESETS,
    );
}

#[test]
fn a_reset_compared_with_a_parameter_has_no_known_level() {
    check(
        "module m #(parameter bit RST_LEVEL = 1) (input logic clk, input logic rst_n, input logic [7:0] d, output logic [7:0] q, output logic [7:0] r);
    always_ff @(posedge clk or negedge rst_n) begin
        if (rst_n == RST_LEVEL) q <= 0;
        else q <= d;
    end
    always_ff @(posedge clk or negedge rst_n) begin
        if (!rst_n) r <= 0;
        else r <= d;
    end
endmodule
",
        RESETS,
    );
}

#[test]
fn variables_with_more_than_one_driver() {
    check(
        "module m (input logic clk, input logic a, input logic b, output logic y, output logic [1:0] v);
    logic x, z;
    always_comb x = a;
    always_comb x = b; // expect: verilog/multiple-drivers
    assign z = a;
    always_comb z = b; // expect: verilog/multiple-drivers
    // Parts of a vector, each driven once.
    always_comb v[0] = a;
    always_comb v[1] = b;
    // One block assigning twice is one driver.
    always_comb begin
        y = a;
        if (b) y = 0;
    end
endmodule
",
        DRIVERS,
    );
}

#[test]
fn nets_and_generate_alternatives_may_have_several_drivers() {
    check(
        "module m #(parameter bit A = 1) (input logic a, input logic b, output logic q);
    wire w;
    assign w = a;
    assign w = b;
    logic y;
    if (A) begin : g_a
        always_comb y = a;
    end else begin : g_b
        always_comb y = b;
    end
    // An initial block is not a driver.
    initial q = 0;
    always_comb q = a;
endmodule
",
        DRIVERS,
    );
    let text = "module m (input logic a, input logic b);
    logic x;
    always_comb x = a;
    always_comb x = b;
endmodule
";
    assert_eq!(
        messages(text, DRIVERS),
        ["`x` is also assigned by the block at line 3; a variable must have one driver"]
    );
}

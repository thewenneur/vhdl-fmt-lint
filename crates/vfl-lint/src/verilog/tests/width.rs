//! Widths of assignments.

use super::{check, messages};

const RULES: &[&str] = &["sema/width-mismatch"];

#[test]
fn truncation_and_extension() {
    check(
        "module m (input logic [7:0] a, input logic [3:0] n, input logic [15:0] w, input logic f, output logic [7:0] y, output logic [7:0] z, output logic [7:0] v, output logic [7:0] u);
    assign y = w; // expect: sema/width-mismatch
    assign z = n; // expect: sema/width-mismatch
    assign v = a;
    // A one-bit flag extended with zeros.
    assign u = f;
    always_comb begin
        y = {n, n};
        z = {n, n, n}; // expect: sema/width-mismatch
        v = {2{n}};
        u = a[3:0]; // expect: sema/width-mismatch
    end
endmodule
",
        RULES,
    );
}

#[test]
fn arithmetic_may_grow_or_be_cut() {
    check(
        "module m (input logic [7:0] a, input logic [7:0] b, input logic [3:0] n, output logic [8:0] sum, output logic [7:0] y, output logic [7:0] z, output logic [7:0] s, output logic [15:0] e);
    assign sum = a + b;
    assign y = a * b;
    assign z = n << 2;
    assign s = $signed(n);
    assign e = -a;
    // Bitwise operators do not grow: `&` of 4 bits is 4 bits.
    assign y = n & n; // expect: sema/width-mismatch
endmodule
",
        RULES,
    );
}

#[test]
fn comparisons_and_reductions_are_one_bit() {
    check(
        "module m (input logic [7:0] a, input logic [7:0] b, output logic f, output logic [1:0] g);
    assign f = a == b;
    assign f = &a;
    assign f = a < b && b != 0;
    assign g = a > b; // a flag made wider
    assign f = a; // expect: sema/width-mismatch
endmodule
",
        RULES,
    );
}

#[test]
fn constants_are_reported_only_when_they_do_not_fit() {
    check(
        "module m (output logic [3:0] y, output logic [3:0] z, output logic [3:0] v, output logic [3:0] u, output logic [63:0] l);
    assign y = 8'd15;
    assign z = 8'd16; // expect: sema/width-mismatch
    assign v = 0;
    assign u = '1;
    assign l = 64'hffff_ffff_ffff_ffff;
endmodule
",
        RULES,
    );
}

#[test]
fn widths_of_parameters() {
    check(
        "module m #(parameter int W = 8, localparam int H = W / 2) (input logic [W-1:0] a, input logic [W:0] b, input logic [2*W-1:0] d, output logic [W-1:0] y, output logic [W-1:0] z, output logic [H-1:0] h);
    assign y = a;
    assign y = b; // expect: sema/width-mismatch
    assign z = d[W-1:0];
    assign z = {a[W-2:0], 1'b0};
    // Not linear: unknown.
    assign h = a;
endmodule
",
        RULES,
    );
}

#[test]
fn localparams_are_replaced_by_their_values() {
    check(
        "module m (input logic [7:0] a, output logic [3:0] y, output logic [7:0] z);
    localparam int N = 4;
    localparam int M = N * 2;
    logic [N-1:0] q;
    logic [M-1:0] r;
    assign y = q;
    assign z = r;
    assign y = r; // expect: sema/width-mismatch
    assign q = a[N-1:0];
endmodule
",
        RULES,
    );
}

#[test]
fn clog2_of_a_constant_and_of_a_parameter() {
    check(
        "module m #(parameter int DEPTH = 16) (input logic [3:0] a, input logic [4:0] b, output logic [$clog2(16)-1:0] y, output logic [$clog2(DEPTH)-1:0] z);
    assign y = a;
    assign y = b; // expect: sema/width-mismatch
    // The same expression, whatever DEPTH is.
    logic [$clog2(DEPTH)-1:0] p;
    assign z = p;
endmodule
",
        RULES,
    );
}

#[test]
fn types_and_part_selects() {
    check(
        "module m (input logic [31:0] a, input byte b, input int i, output logic [7:0] y, output logic [15:0] z, output logic [7:0] v);
    typedef logic [3:0] nibble_t;
    typedef struct packed { logic [3:0] hi; logic [3:0] lo; } pair_t;
    nibble_t n;
    pair_t p;
    assign y = b;
    assign z = b; // expect: sema/width-mismatch
    assign y = a[15:8];
    assign y = a[8 +: 8];
    assign y = a[15 -: 4]; // expect: sema/width-mismatch
    assign y = n; // expect: sema/width-mismatch
    assign v = p;
    // An integer loop index or counter fits.
    assign y = i;
endmodule
",
        RULES,
    );
}

#[test]
fn shift_registers_and_macros_are_not_reported() {
    check(
        "module m (input logic clk, input logic d, input logic [7:0] a, output logic [3:0] q, output logic [7:0] y);
    always_ff @(posedge clk) q <= {q, d};
    always_ff @(posedge clk) q[3:1] <= q;
    assign y = `WIDE_VALUE;
    assign y = 4'(a); // expect: sema/width-mismatch
endmodule
",
        RULES,
    );
}

#[test]
fn memories_have_the_width_of_their_elements() {
    check(
        "module m (input logic [1:0] i, output logic [7:0] y, output logic [3:0] z);
    logic [7:0] mem [0:3];
    assign y = mem[i];
    assign z = mem[i]; // expect: sema/width-mismatch
endmodule
",
        RULES,
    );
}

#[test]
fn width_messages() {
    let text = "module m #(parameter int W = 8) (input logic [W:0] a, input logic [3:0] n, output logic [W-1:0] y, output logic z);
    assign y = a;
    assign z = n;
endmodule
";
    assert_eq!(
        messages(text, RULES),
        [
            "`y` has W bits but the value has W+1",
            "`z` has 1 bit but the value has 4"
        ]
    );
}

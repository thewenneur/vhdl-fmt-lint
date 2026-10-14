`default_nettype none

module leaf #(
    parameter WIDTH = 4
) (
    input  wire [WIDTH-1:0] d,
    input  wire             en,
    output wire [WIDTH-1:0] q
);
    assign q = en ? d : '0;
endmodule

module fixed (
    input  wire [3:0] d,
    output wire [3:0] q
);
    assign q = d;
endmodule

// Its macros may declare parameters and ports.
module open_ended;
    `EXTRA_PARAMETERS
endmodule

module instances (
    input  wire [3:0] d,
    input  wire [7:0] wide,
    output wire [3:0] q0,
    output wire [3:0] q1,
    output wire [3:0] q2,
    output wire [3:0] q3,
    output wire [3:0] q4
);
    leaf u0 (d, 1'b1, q0); // expect: style/positional-association
    leaf #(.DEPTH(2)) u1 (.d(d), .en(1'b1), .q(q1)); // expect: sema/unresolved-name
    leaf u2 (.d(d), .q(q2)); // expect: verilog/unconnected-input
    leaf u3 (.d(d), .en(1'b1), .q(q3), .x()); // expect: sema/unresolved-name
    fixed u4 (.d(wide), .q(q4)); // expect: sema/width-mismatch
    open_ended #(.ANY(1)) u5 ();
endmodule

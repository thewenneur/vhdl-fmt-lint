`default_nettype none

module names #(
    parameter WIDTH = 8,
    parameter SPARE_P = 1 // expect: sema/unused-generic
) (
    input  wire             clk,
    input  wire [WIDTH-1:0] a,
    input  wire             spare, // expect: sema/unused-port
    output wire [WIDTH-1:0] b,
    output wire             c // expect: sema/undriven-signal
);
    wire [WIDTH-1:0] never; // expect: sema/unused-signal
    wire [WIDTH-1:0] unread; // expect: sema/unread-signal
    wire [WIDTH-1:0] undriven; // expect: sema/undriven-signal
    wire [WIDTH-1:0] twice;
    wire [WIDTH-1:0] twice; // expect: sema/duplicate-declaration
    // Marked as unused on purpose.
    wire unused_ok = clk;

    assign unread = a;
    assign twice = a;
    assign b = undriven ^ twice ^ missing; // expect: sema/unresolved-name
endmodule

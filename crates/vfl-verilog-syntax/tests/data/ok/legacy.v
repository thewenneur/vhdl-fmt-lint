`timescale 1ns / 1ps
`default_nettype none

// Verilog-2005: non-ANSI ports, regs, tasks, gates, generate regions.
module counter (clk, rst, en, q, carry);
  parameter WIDTH = 4;
  input clk, rst, en;
  output [WIDTH-1:0] q;
  output carry;
  reg [WIDTH-1:0] q;
  wire carry;
  integer i;
  genvar g;

  and #1 g_and (carry, en, &q);

  always @(posedge clk or posedge rst)
    if (rst)
      q <= {WIDTH{1'b0}};
    else if (en)
      q <= q + 1;

  generate
    for (g = 0; g < WIDTH; g = g + 1) begin : bits
      wire b = q[g];
    end
  endgenerate

  task show;
    input [WIDTH-1:0] value;
    begin
      $display("%m: q = %h", value);
    end
  endtask

  initial begin
    for (i = 0; i < 3; i = i + 1) #10 show(q);
    casez (q)
      4'b1???: $display("high");
      default: $display("low");
    endcase
  end

  specify
    (clk => q) = (1.0, 2.0);
  endspecify
endmodule

primitive mux2 (out, sel, a, b);
  output out;
  input sel, a, b;
  table
    0 1 ? : 1;
    0 0 ? : 0;
    1 ? 1 : 1;
    1 ? 0 : 0;
  endtable
endprimitive

module top;
  wire [3:0] q;
  wire c;
  reg clk = 0, rst = 1, en = 1;
  counter #(4) u0 (.clk(clk), .rst(rst), .en(en), .q(q), .carry(c));
  defparam u0.WIDTH = 4;
  always #5 clk = ~clk;
endmodule

`default_nettype wire

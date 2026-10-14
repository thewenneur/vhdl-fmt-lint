`define WIDTH 8
`define FF(q, d, clk) \
  always_ff @(posedge clk) q <= d;
`define ASSERT(name, prop) \
  name: assert property (prop) // a comment ending the body

`ifndef SYNTHESIS
`define SIM_ONLY
`endif

module macros (input logic clk, input logic [`WIDTH-1:0] d, output logic [`WIDTH-1:0] q);
  `FF(q, d, clk)
`ifdef SIM_ONLY
  initial $display("simulation");
`elsif FPGA
  this is not parsed (
`else
  neither is this
`endif
  `ASSERT(stable, @(posedge clk) q == $past(d))
  logic [`WIDTH:0] wide;
  always_comb begin
    `ifdef SIM_ONLY wide = {1'b0, q}; `endif
    `uvm_info("ID", $sformatf("q=%0d", q), UVM_LOW)
    wide[0] = q[0];
  end
endmodule
`undef WIDTH

package pkg;
typedef enum logic [1:0] {IDLE, RUN, DONE} state_t;
typedef struct packed { logic valid; logic [7:0] data; state_t st; } item_t;
localparam item_t RESET = '{valid: 1'b0, data: '0, st: IDLE};
function automatic logic [7:0] swap(input logic [7:0] v);
return {v[3:0], v[7:4]};
endfunction
endpackage
interface bus_if #(parameter W = 8) (input logic clk);
logic [W-1:0] data; logic valid;
modport master(output data, output valid);
modport slave(input data, input valid);
endinterface
module top import pkg::*; (bus_if.slave s, output item_t o);
assign o = '{valid: s.valid, data: s.data, st: RUN};
assert property (@(posedge s.clk) s.valid |-> ##1 !s.valid);
endmodule

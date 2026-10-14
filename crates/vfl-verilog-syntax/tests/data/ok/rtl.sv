// A small synthesizable SystemVerilog design.
module fifo
  import fifo_pkg::*;
#(
  parameter int unsigned Depth = 8,
  parameter type         T     = logic [7:0],
  localparam int         AW    = $clog2(Depth)
) (
  input  logic          clk_i,
  input  logic          rst_ni,
  input  T              data_i,
  input  logic          valid_i,
  output logic          ready_o,
  output T              data_o,
  bus_if.slave          bus,
  output logic [AW:0]   count_o
);
  typedef enum logic [1:0] {Idle, Busy = 2'd2, Done} state_e;
  typedef struct packed {
    logic       valid;
    logic [3:0] tag;
  } entry_t;

  state_e state_q, state_d;
  T mem_q [Depth];
  logic [AW-1:0] wptr_q, rptr_q;
  entry_t [1:0] entries;
  wire full = count_o == Depth;

  assign ready_o = ~full;
  assign {data_o, entries[0].tag} = {mem_q[rptr_q], 4'hF};

  always_ff @(posedge clk_i or negedge rst_ni) begin : seq
    if (!rst_ni) begin
      wptr_q  <= '0;
      state_q <= Idle;
    end else if (valid_i && ready_o) begin
      mem_q[wptr_q] <= data_i;
      wptr_q        <= wptr_q + 1'b1;
    end
  end : seq

  always_comb begin
    state_d = state_q;
    unique case (state_q)
      Idle:       state_d = Busy;
      Busy, Done: begin
        state_d = Idle;
      end
      default: ;
    endcase
    priority if (count_o inside {[0:2], Depth}) state_d = Idle;
  end

  for (genvar i = 0; i < 2; i++) begin : g_entries
    assign entries[i].valid = i[0] ? valid_i : 1'b0;
  end

  if (Depth > 4) begin : g_big
    counter #(.Width(AW + 1)) u_count (
      .clk_i,
      .rst_ni,
      .en_i    (valid_i & ready_o),
      .count_o (count_o),
      .unused_o()
    );
  end else begin : g_small
    assign count_o = '0;
  end

  function automatic logic [AW-1:0] next(input logic [AW-1:0] p, int step = 1);
    return p + step[AW-1:0];
  endfunction : next

  initial assert (Depth > 1) else $error("Depth must be > 1, got %0d", Depth);
  a_ready: assert property (@(posedge clk_i) disable iff (!rst_ni) full |-> !ready_o);
endmodule : fifo

interface bus_if #(parameter W = 8) (input logic clk);
  logic [W-1:0] data;
  logic valid, ready;
  modport master (output data, valid, input ready);
  modport slave (input data, valid, output ready);
endinterface

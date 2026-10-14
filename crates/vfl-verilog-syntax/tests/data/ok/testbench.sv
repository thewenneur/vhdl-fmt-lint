`timescale 1ns/1ps
module tb;
  import uvm_pkg::*;
  logic clk = 0;
  int unsigned errors;
  bit [7:0] queue [$];
  string names [string];
  event done;

  class packet;
    rand bit [7:0] data;
    constraint c_data { data inside {[1:10]}; }
    function new(); endfunction
  endclass

  covergroup cg @(posedge clk);
    coverpoint errors;
  endgroup

  property p_handshake;
    @(posedge clk) 1'b1 |=> 1'b1;
  endproperty

  clocking cb @(posedge clk);
    default input #1step output #1;
  endclocking

  always #5 clk = ~clk;

  initial begin
    packet p = new();
    automatic int count = 0;
    fork
      begin
        repeat (3) @(posedge clk);
        -> done;
      end
      wait (errors == 0);
    join_any
    disable fork;
    queue.push_back(8'h12);
    foreach (queue[i]) count += queue[i];
    assert (p.randomize() with { data < 5; }) else $fatal(1, "randomize failed");
    do count--; while (count > 0);
    names["a"] = $sformatf("%0d", count);
    $display("%0t: %s", $time, names["a"]);
    #1ns;
    @done;
    $finish;
  end
endmodule

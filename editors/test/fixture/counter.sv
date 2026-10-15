module counter (input logic clk, output logic [3:0] q);
always_ff @(posedge clk) begin
q<=q+1;
end
endmodule

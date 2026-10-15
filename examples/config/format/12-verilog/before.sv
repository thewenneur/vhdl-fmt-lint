module counter #(parameter int WIDTH=8, parameter int RESET_VALUE=0)(
input logic clk,
input logic rst,
input logic enable,
output logic [WIDTH-1:0] count
);
always_ff @(posedge clk) begin
if (rst) count <= RESET_VALUE;
else if (enable) count <= count + 1;
end
endmodule

module top(input logic clk, input logic rst, output logic [7:0] q);
counter #(.WIDTH(8), .RESET_VALUE(0)) u_counter (.clk(clk), .rst(rst), .enable(1'b1), .count(q));
endmodule

`default_nettype none

module resets (
    input  wire       clk,
    input  wire       rst_n,
    input  wire [7:0] d,
    output reg  [7:0] a,
    output reg  [7:0] b,
    output reg  [7:0] c
);
    always @(posedge clk or negedge rst_n) begin : both
        if (!rst_n) begin
            a <= 8'd0;
        end else begin
            a <= d;
            b <= d; // expect: sema/missing-reset
        end
    end

    always @(posedge clk) begin : sync
        if (!rst_n) c <= 8'd0; // expect: sema/mixed-reset
        else c <= d;
    end
endmodule

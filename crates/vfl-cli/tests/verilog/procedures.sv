`default_nettype none

module procedures (
    input  wire       clk,
    input  wire       rst_n,
    input  wire       en,
    input  wire [1:0] sel,
    input  wire [7:0] d,
    output reg  [7:0] q,
    output reg  [7:0] y,
    output reg  [7:0] l,
    output reg  [7:0] m,
    output reg  [7:0] k,
    output reg  [7:0] u
);
    reg [7:0] t;

    always @(posedge clk or negedge rst_n) begin : seq
        if (!rst_n) q <= 8'd0;
        else q = d; // expect: verilog/blocking-in-clocked
    end

    always @* begin : comb
        y <= d; // expect: verilog/nonblocking-in-combinational
    end

    always @* begin : latch
        if (en) l = d; // expect: sema/inferred-latch
    end

    always @(en) begin : sens // expect: sema/sensitivity-list
        if (en) m = d;
        else m = 8'd0;
    end

    // A temporary nothing outside reads may be assigned with `=`.
    always @(posedge clk or negedge rst_n) begin : temp
        if (!rst_n) begin
            t <= 8'd0;
            u <= 8'd0;
        end
        else begin
            t = d + 8'd1;
            u <= t;
        end
    end

    always @* begin : dec
        case (sel) // expect: verilog/case-default
            2'd0: k = d; // expect: sema/inferred-latch
            2'd1: k = ~d;
        endcase
    end
endmodule

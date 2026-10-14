`default_nettype none

module widths #(
    parameter N = 8
) (
    input  wire [N-1:0] a,
    input  wire [3:0]   n,
    input  wire [7:0]   s,
    output wire [N-1:0] same,
    output wire [N:0]   sum,
    output wire [3:0]   cut,
    output wire [3:0]   kept,
    output reg  [7:0]   sh
);
    assign same = a;
    assign sum = a + 1'b1;
    assign cut = s; // expect: sema/width-mismatch
    // vfl-ignore sema/width-mismatch
    assign kept = s;
    integer i;
    always @* begin : shift
        sh = s;
        for (i = 0; i < 4; i = i + 1) sh = {sh, n[i]};
    end
endmodule

module nettype ( // expect: verilog/default-nettype
    input  a,
    input  b,
    output y
);
    and g (n, a, b); // expect: verilog/implicit-net
    assign y = n;
endmodule

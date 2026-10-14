`default_nettype none

module machines (
    input  wire clk,
    input  wire rst_n,
    input  wire go,
    output wire busy
);
    typedef enum logic [1:0] {
        IDLE,
        RUN,
        DONE,
        LOST // expect: sema/fsm-unreachable-state
    } state_t;
    state_t state, next;

    always_ff @(posedge clk or negedge rst_n) begin : regs
        if (!rst_n) state <= IDLE;
        else state <= next;
    end

    always_comb begin : step
        next = state;
        case (state)
            IDLE: if (go) next = RUN;
            RUN: next = DONE;
            DONE: next = DONE; // expect: sema/fsm-stuck-state
            LOST: next = IDLE;
            default: next = IDLE;
        endcase
    end

    assign busy = state == RUN;
endmodule

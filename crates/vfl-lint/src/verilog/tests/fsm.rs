//! State machines.

use super::{check, messages};

const RULES: &[&str] = &["sema/fsm-unreachable-state", "sema/fsm-stuck-state"];

#[test]
fn an_enumeration_with_a_next_state_variable() {
    check(
        "module m (input logic clk, input logic rst_n, input logic go, output logic busy);
    typedef enum logic [1:0] {
        IDLE,
        RUN,
        DONE,
        LOST // expect: sema/fsm-unreachable-state
    } state_t;
    state_t state, next;
    always_ff @(posedge clk or negedge rst_n) begin
        if (!rst_n) state <= IDLE;
        else state <= next;
    end
    always_comb begin
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
",
        RULES,
    );
}

#[test]
fn messages_name_the_variable_and_type() {
    let text = "module m (input logic clk, input logic rst, input logic go);
    typedef enum {A, B, C} abc_t;
    abc_t s;
    always_ff @(posedge clk) begin
        if (rst) s <= A;
        else case (s)
            A: if (go) s <= B;
            B: s <= B;
            C: s <= A;
        endcase
    end
endmodule
";
    assert_eq!(
        messages(text, RULES),
        [
            "no transition of `s` reaches state `C` of `abc_t`",
            "`s` never leaves state `B` once there, except by a reset"
        ]
    );
}

#[test]
fn localparam_states() {
    check(
        "module m (input logic clk, input logic rst, input logic go);
    localparam IDLE = 2'd0;
    localparam BUSY = 2'd1;
    localparam DEAD = 2'd2; // expect: sema/fsm-unreachable-state
    logic [1:0] state;
    always @(posedge clk) begin
        if (rst) state <= IDLE;
        else case (state)
            IDLE: state <= go ? BUSY : IDLE;
            BUSY: state <= IDLE;
            DEAD: state <= IDLE;
        endcase
    end
endmodule
",
        RULES,
    );
}

#[test]
fn a_state_set_outside_the_case_may_follow_any() {
    check(
        "module m (input logic clk, input logic rst, input logic abort);
    typedef enum {A, B, C} abc_t;
    abc_t s;
    always_ff @(posedge clk) begin
        if (rst) s <= A;
        else begin
            case (s)
                A: s <= B;
                B: s <= B;
                C: s <= A;
            endcase
            if (abort) s <= C;
        end
    end
endmodule
",
        RULES,
    );
}

#[test]
fn transitions_that_are_not_known() {
    // A next state computed some other way.
    check(
        "module m (input logic clk, input logic [1:0] d);
    typedef enum logic [1:0] {A, B, C} abc_t;
    abc_t s;
    always_ff @(posedge clk) begin
        case (s)
            A: s <= abc_t'(d);
            B: s <= B;
            C: s <= A;
        endcase
    end
endmodule
",
        RULES,
    );
    // A macro in the case.
    check(
        "module m (input logic clk);
    typedef enum {A, B, C} abc_t;
    abc_t s;
    always_ff @(posedge clk) begin
        case (s)
            A: `GO(s)
            B: s <= B;
            C: s <= A;
        endcase
    end
endmodule
",
        RULES,
    );
}

#[test]
fn a_case_that_decodes_a_state_is_not_a_machine() {
    check(
        "module m (input logic clk, input logic rst, output logic [7:0] y);
    typedef enum {A, B, C} abc_t;
    abc_t s;
    always_ff @(posedge clk) begin
        if (rst) s <= A;
        else s <= s == A ? B : A;
    end
    always_comb begin
        case (s)
            A: y = 1;
            B: y = 2;
            C: y = 3;
        endcase
    end
endmodule
",
        RULES,
    );
}

#[test]
fn the_initial_value_is_a_starting_state() {
    check(
        "module m (input logic clk, input logic go);
    typedef enum {A, B, C} abc_t; // expect: sema/fsm-unreachable-state
    abc_t s = B;
    always_ff @(posedge clk) begin
        case (s)
            A: s <= B;
            B: if (go) s <= C;
            C: s <= B;
        endcase
    end
endmodule
",
        RULES,
    );
}

#[test]
fn each_rule_alone() {
    let text = "module m (input logic clk, input logic rst);
    typedef enum {A, B, C} abc_t;
    abc_t s;
    always_ff @(posedge clk) begin
        if (rst) s <= A;
        else case (s)
            A: s <= B;
            B: s <= B;
            C: s <= A;
        endcase
    end
endmodule
";
    assert_eq!(
        messages(text, &["sema/fsm-unreachable-state"]),
        ["no transition of `s` reaches state `C` of `abc_t`"]
    );
    assert_eq!(
        messages(text, &["sema/fsm-stuck-state"]),
        ["`s` never leaves state `B` once there, except by a reset"]
    );
    assert!(messages(text, &[]).is_empty());
}

library ieee;
use ieee.std_logic_1164.all;

entity blinker is
    generic (
        HALF_PERIOD : positive := 4
    );
    port (
        clk, rst : in  std_logic;
        led      : out std_logic
    );
end;

architecture rtl of blinker is
    type state_t is (OFF, ON_STATE)                  ;

    constant MAX_COUNT : natural := HALF_PERIOD - 1  ;

    -- the state and the count
    signal state       : state_t                     ;
    signal count       : natural range 0 to MAX_COUNT;

begin
    p_count : process (clk)
    begin
        if rising_edge(clk) then
            if rst = '1' or count = MAX_COUNT then
                count <= 0        ;
            else
                count <= count + 1;
            end if;
        end if;
    end process;

    -- toggles when the count wraps
    p_state : process (clk)
    begin
        if rising_edge(clk) and count = MAX_COUNT then
            if state = OFF then
                state <= ON_STATE;
            else
                state <= OFF     ;
            end if;
        end if;
    end process;
    led <= '1' when state = ON_STATE else '0';
end;

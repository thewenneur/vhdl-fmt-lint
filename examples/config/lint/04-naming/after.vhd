library ieee;
use ieee.std_logic_1164.all;

entity blinker is
    port (
        clk_i : in  std_logic;
        led_o : out std_logic
    );
end;

architecture rtl of blinker is
    type phase_t is (ON_PHASE, OFF_PHASE);
    constant C_HALF_PERIOD : natural := 25_000_000;
    signal s_ticks     : natural range 0 to C_HALF_PERIOD - 1;
    signal s_led_phase  : phase_t;
begin
    p_toggle : process (clk_i)
    begin
        if rising_edge(clk_i) then
            if s_ticks = C_HALF_PERIOD - 1 then
                s_ticks <= 0;
                if s_led_phase = ON_PHASE then
                    s_led_phase <= OFF_PHASE;
                else
                    s_led_phase <= ON_PHASE;
                end if;
            else
                s_ticks <= s_ticks + 1;
            end if;
        end if;
    end process;

    led_o <= '1' when s_led_phase = ON_PHASE else '0';
end;

library ieee;
use ieee.std_logic_1164.all;

entity board is
    port (
        clk_i  : in  std_logic;
        led0_o : out std_logic
    );
end;

architecture rtl of board is
begin
    u_blink0 : entity work.blinker
        port map (clk_i => clk_i, led_o => led0_o);
end;

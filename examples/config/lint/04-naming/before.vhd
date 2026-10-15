library ieee;
use ieee.std_logic_1164.all;

entity blinker is
    port (
        clk : in  std_logic;
        led : out std_logic
    );
end;

architecture rtl of blinker is
    type Phase is (ON_PHASE, OFF_PHASE);
    constant halfPeriod : natural := 25_000_000;
    signal ticks     : natural range 0 to halfPeriod - 1;
    signal ledPhase  : Phase;
begin
    toggle : process (clk)
    begin
        if rising_edge(clk) then
            if ticks = halfPeriod - 1 then
                ticks <= 0;
                if ledPhase = ON_PHASE then
                    ledPhase <= OFF_PHASE;
                else
                    ledPhase <= ON_PHASE;
                end if;
            else
                ticks <= ticks + 1;
            end if;
        end if;
    end process;

    led <= '1' when ledPhase = ON_PHASE else '0';
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
    blink0 : entity work.blinker
        port map (clk => clk_i, led => led0_o);
end;

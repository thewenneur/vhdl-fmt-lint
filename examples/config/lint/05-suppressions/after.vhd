-- Silence a rule in the whole file:
-- vfl-ignore-file style/process-label -- short processes, named by their comments

library ieee;
use ieee.std_logic_1164.all;

entity debouncer is
    port (
        clk     : in     std_logic;
        button  : in     std_logic;
        pressed : buffer std_logic; -- vfl-ignore style/no-buffer-ports -- the board's VHDL-93 flow
        led     : out    std_logic
    );
end;

architecture rtl of debouncer is
    signal history : std_logic_vector(3 downto 0);
    -- Silence a rule on the next line of code:
    -- vfl-ignore sema/unused-signal -- kept for the next board revision
    signal spare   : std_logic;
begin
    -- Shift the button in.
    process (clk)
    begin
        if rising_edge(clk) then
            history <= history(2 downto 0) & button;
        end if;
    end process;

    -- Change state after four equal samples.
    process (clk)
    begin
        if rising_edge(clk) then
            if history = "1111" then
                pressed <= '1';
            elsif history = "0000" then
                pressed <= '0';
            end if;
        end if;
    end process;

    led <= pressed;
end;

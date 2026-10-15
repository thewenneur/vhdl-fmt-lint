library ieee;
use ieee.std_logic_1164.all;

entity sync_ff is
    port (
        clk   : in  std_logic;
        rst   : in  std_logic;
        d     : in  std_logic;
        debug : in  std_logic;
        q     : out std_logic
    );
end;

architecture rtl of sync_ff is
    signal meta : std_logic;
begin
    process (clk)
    begin
        if rising_edge(clk) then
            meta <= d; q <= meta; -- two flip-flops in a row to bring d into this clock domain
        end if;
    end process;
end;

library ieee;
use ieee.std_logic_1164.all;

entity lut is
    port (
        clk  : in  std_logic           ;
        addr : in  natural range 0 to 7;
        data : out std_logic_vector(7 downto 0)
    );
end;

architecture rtl of lut is
    type rom_t is array (0 to 7) of std_logic_vector(7 downto 0);
    -- The table keeps its hand-made layout, one row of four per line:
    -- vfl: off
    constant ROM : rom_t := (
        x"00", x"01", x"04", x"09",
        x"10", x"19", x"24", x"31"
    );
    -- vfl: on
    signal addr_q : natural range 0 to 7;
begin
    addr_q <= addr when rising_edge(clk);
    -- The next statement is kept exactly as written:
    -- vfl: skip
    data <= ROM(addr_q)  ;   -- even the spaces before this comment
end;

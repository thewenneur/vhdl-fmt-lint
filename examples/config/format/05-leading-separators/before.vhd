library ieee;
use ieee.std_logic_1164.all;

entity top is
    generic (
        DEPTH : natural := 16; -- words
        WIDTH : natural := 8
    );
    port (
        clk  : in  std_logic;
        din  : in  std_logic_vector(WIDTH - 1 downto 0);
        dout : out std_logic_vector(WIDTH - 1 downto 0)
    );
end;

architecture rtl of top is
begin
    u_fifo : entity work.fifo generic map (DEPTH => DEPTH, WIDTH => WIDTH) port map (clk => clk, din => din, dout => dout);
end;

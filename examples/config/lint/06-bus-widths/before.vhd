library ieee;
use ieee.std_logic_1164.all;
use ieee.numeric_std.all;

entity accumulator is
    generic (
        WIDTH : natural := 8;
        DEPTH : natural := 4
    );
    port (
        clk   : in  std_logic;
        din   : in  unsigned(WIDTH - 1 downto 0);
        sum   : out unsigned(WIDTH downto 0);
        count : out unsigned(DEPTH - 1 downto 0);
        top   : out std_logic
    );
end;

architecture rtl of accumulator is
    signal acc : unsigned(WIDTH downto 0);
    signal n   : unsigned(DEPTH - 1 downto 0);
begin
    step : process (clk)
    begin
        if rising_edge(clk) then
            acc <= acc + din;
            n   <= din;
        end if;
    end process;
    sum   <= acc;
    count <= n;
    top   <= acc(WIDTH + 1);
end;

library ieee;
use ieee.std_logic_1164.all;
use ieee.numeric_std.all;

entity system is
    generic (
        DATA_W : natural := 12
    );
    port (
        clk   : in  std_logic;
        data  : in  unsigned(DATA_W - 1 downto 0);
        total : out unsigned(DATA_W - 1 downto 0);
        count : out unsigned(3 downto 0);
        full  : out std_logic
    );
end;

architecture rtl of system is
begin
    acc : entity work.accumulator
        generic map (WIDTH => DATA_W)
        port map (clk => clk, din => data, sum => total, count => count, top => full);
end;

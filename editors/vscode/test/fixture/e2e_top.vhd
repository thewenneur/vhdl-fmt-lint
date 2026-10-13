library ieee;
use ieee.std_logic_1164.all;

entity e2e_top is
  port (clk : in std_logic; q : out std_logic);
end entity;

architecture rtl of e2e_top is
  signal unused_s : std_logic;
  signal r : std_logic := '0';
begin
  process (clk) begin
    if rising_edge(clk) then
      r <= not r;
    end if;
  end process;
  q   <=    r;
end architecture;

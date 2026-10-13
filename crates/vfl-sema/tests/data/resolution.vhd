-- file: pkg.vhd
library ieee;
use ieee.std_logic_1164.all;

package pkg is
  constant WIDTH : natural := 8;
  type state_t is (idle, busy);
  function double (x : natural) return natural;
end package;

package body pkg is
  constant HIDDEN : natural := 1;
  function double (x : natural) return natural is
  begin
    return 2 * x;
  end function;
end package body;

-- file: sub.vhd
library ieee;
use ieee.std_logic_1164.all;
use work.pkg.all;

entity sub is
  generic (N : natural := WIDTH);
  port (
    clk : in  std_logic;
    d   : in  std_logic_vector(N - 1 downto 0);
    q   : out std_logic_vector(N - 1 downto 0)
  );
end entity;

architecture rtl of sub is
begin
  q <= d when rising_edge(clk);
end architecture;

-- file: top.vhd
library ieee;
use ieee.std_logic_1164.all;
use ieee.numeric_std.all;
use work.pkg.all;
use work.nothing.all;

entity top is
  port (clk, rst : in std_logic; o : out std_logic);
end entity;

architecture rtl of top is
  signal a, b : std_logic_vector(WIDTH - 1 downto 0);
  signal st   : state_t;
  signal unused_s : std_logic;
  signal never_driven : std_logic;
  signal never_read : std_logic;
  constant UNUSED_C : integer := 3;
begin
  u0 : entity work.sub port map (clk => clk, d => a, q => b);
  u1 : entity work.sub port map (clk => clk, d => a, qq => b);
  u2 : entity work.sub port map (d => a, q => open);
  u3 : entity work.sub(nope) port map (clk => clk, d => a, q => open);
  u4 : entity work.missing port map (x => y);

  comb : process (a)
    variable v : natural;
  begin
    v := double(to_integer(unsigned(a)));
    st <= busy when v > 0 else idle;
    o <= never_driven or b(0) or undeclared;
  end process;

  seq : process (clk, rst)
  begin
    if rst = '1' then
      never_read <= '0';
    elsif rising_edge(clk) then
      never_read <= a(0) and work.pkg.WIDTH > 0 and work.pkg.nope;
    end if;
  end process;
end architecture;

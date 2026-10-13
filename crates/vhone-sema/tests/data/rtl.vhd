-- file: rtl_pkg.vhd
library ieee;
use ieee.std_logic_1164.all;
package rtl_pkg is
  constant WIDTH : natural := 4 * 2;
  subtype word_t is std_logic_vector(WIDTH - 1 downto 0);
  type nibble_t is array (0 to 3) of std_logic;
end package;
-- file: child.vhd
library ieee;
use ieee.std_logic_1164.all;
entity child is
  generic (N : natural := 4);
  port (
    a : in std_logic_vector(3 downto 0);
    b : in std_logic_vector(N - 1 downto 0);
    y : out std_logic_vector(7 downto 0));
end entity;
architecture rtl of child is
begin
  y <= a & b(3 downto 0);
end architecture;
-- file: top.vhd
library ieee;
use ieee.std_logic_1164.all;
use ieee.numeric_std.all;
use work.rtl_pkg.all;
entity top is
  port (
    clk_a, clk_b, sel, en : in std_logic;
    d : in word_t;
    q : out word_t);
end entity;
architecture rtl of top is
  signal w : word_t := x"00";
  signal bad_init : std_logic_vector(3 downto 0) := "00000";
  signal n : nibble_t;
  signal u : unsigned(7 downto 0);
  signal small : std_logic_vector(3 downto 0);
  signal latch_q, full_q, case_q, cond_q : std_logic;
  signal flag_a, sync_1, sync_2, used_b : std_logic;
  signal data_a : std_logic_vector(7 downto 0);
begin
  -- Width mismatches, and widths that agree.
  w <= d;
  w <= d(3 downto 0);
  w <= "0000" & d(7 downto 4);
  w <= '0' & d(7 downto 1);
  w <= x"F" & small & '1';
  w <= 8x"3";
  w <= std_logic_vector(u + 1);
  w <= std_logic_vector(u(3 downto 0));
  w <= (others => '0');
  small <= d(WIDTH - 1 downto WIDTH - 4);
  n <= "10101";
  q <= w when sel = '1' else d(6 downto 0);
  c1 : entity work.child port map (a => d, b => small, y => w);
  c2 : entity work.child port map (small, small, small);

  -- Latches.
  latch : process (all)
  begin
    if en = '1' then
      latch_q <= d(0);
    end if;
  end process;

  no_latch : process (all)
  begin
    full_q <= '0';
    if en = '1' then
      full_q <= d(0);
    end if;
    if sel = '1' then
      cond_q <= '1';
    else
      cond_q <= '0';
    end if;
  end process;

  case_latch : process (sel, en)
  begin
    case sel is
      when '1' => case_q <= en;
      when others => null;
    end case;
  end process;

  -- Clock domains.
  reg_a : process (clk_a)
  begin
    if rising_edge(clk_a) then
      flag_a <= en;
      data_a <= d;
    end if;
  end process;

  reg_b : process (clk_b)
  begin
    if rising_edge(clk_b) then
      sync_1 <= flag_a;
      sync_2 <= sync_1;
      if data_a = x"00" then
        used_b <= sync_2;
      end if;
    end if;
  end process;
end architecture;

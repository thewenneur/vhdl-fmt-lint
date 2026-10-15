-- file: leaf.vhd
library ieee;
use ieee.std_logic_1164.all;

entity leaf is
  port (a : in std_logic; y : out std_logic);
end entity;

architecture rtl of leaf is
begin
  y <= not a;
end architecture;

-- file: drivers.vhd
library ieee;
use ieee.std_logic_1164.all;

entity drivers is
  generic (WIDE : boolean := true);
  port (
    clk, a, b, en : in std_logic;
    y             : out std_logic;
    bus_io        : inout std_logic
  );
end entity;

architecture rtl of drivers is
  type rec_t is record
    f : bit;
    g : bit;
  end record;
  signal flag   : boolean;
  signal both   : std_logic;
  signal inst   : std_logic;
  signal v      : std_logic_vector(7 downto 0);
  signal w      : bit_vector(3 downto 0);
  signal r      : rec_t;
  signal gen    : std_logic_vector(3 downto 0);
  signal choice : std_logic;
begin
  -- An unresolved signal driven by two processes.
  p1 : process (clk)
  begin
    if rising_edge(clk) then
      flag <= true;
    end if;
  end process;
  p2 : process (a)
  begin
    flag <= a = '1';
  end process;

  -- A resolved signal driven twice.
  both <= a;
  both <= b;

  -- An instance and an assignment.
  u : entity work.leaf port map (a => a, y => inst);
  inst <= b;

  -- Disjoint parts are fine; overlapping ones are not.
  v(3 downto 0) <= (others => '0');
  v(7 downto 4) <= (others => '1');
  w(1 downto 0) <= "00";
  w(1) <= '1';
  r.f <= '0';
  r.g <= '1';

  -- Generated drivers of parts with unknown indexes.
  g : for i in 0 to 3 generate
    gen(i) <= a;
  end generate;

  -- Alternatives of an `if` generate never coexist.
  g1 : if WIDE generate
    choice <= a;
  end generate;
  g2 : if not WIDE generate
    choice <= b;
  end generate;

  -- A tri-state bus.
  bus_io <= a when en = '1' else 'Z';
  bus_io <= b when en = '0' else 'Z';

  y <= inst;
end architecture;

-- file: loops.vhd
library ieee;
use ieee.std_logic_1164.all;

entity loops is
  port (clk, a, b : in std_logic; y : out std_logic);
end entity;

architecture rtl of loops is
  signal x1, x2, x3 : std_logic;
  signal s          : std_logic;
  signal l          : std_logic;
  signal c          : std_logic_vector(3 downto 0);
  signal r1, r2     : std_logic;
  signal clk_sim    : std_logic := '0';
  signal p, q       : std_logic;
begin
  -- Through three concurrent assignments.
  x1 <= a and x3;
  x2 <= not x1;
  x3 <= x2 or b;

  -- On itself.
  s <= s xor a;

  -- A latch, not a loop.
  l <= a when b = '1' else l;

  -- Different bits of a vector.
  c(0) <= a;
  c(1) <= c(0) and b;

  -- Through a register: no loop.
  r1 <= r2 and a;
  reg : process (clk)
  begin
    if rising_edge(clk) then
      r2 <= r1;
    end if;
  end process;

  -- A clock generator.
  clk_sim <= not clk_sim after 5 ns;

  -- Through a combinational process and a condition.
  comb : process (all)
  begin
    p <= '0';
    if q = '1' then
      p <= a;
    end if;
  end process;
  q <= p and b;

  y <= x3 or s or l or c(1) or r2 or clk_sim;
end architecture;

-- file: widths_pkg.vhd
library ieee;
use ieee.std_logic_1164.all;
package widths_pkg is
  constant BUS_W : natural := 16;
  type mem_t is array (0 to 3) of std_logic_vector(7 downto 0);
end package;
-- file: fifo.vhd
library ieee;
use ieee.std_logic_1164.all;
entity fifo is
  generic (
    W     : natural := 8;
    DEPTH : natural := 4;
    ADDR  : natural := 2;
    INIT  : std_logic_vector(W - 1 downto 0) := (others => '0'));
  port (
    din   : in std_logic_vector(W - 1 downto 0);
    dout  : out std_logic_vector(W - 1 downto 0);
    level : out std_logic_vector(ADDR downto 0);
    wide  : out std_logic_vector(2 * W - 1 downto 0));
end entity;
architecture rtl of fifo is
  constant LAST : natural := W - 1;
  signal reg   : std_logic_vector(LAST downto 0);
  signal big   : std_logic_vector(W downto 0);
  signal small : std_logic_vector(DEPTH - 1 downto 0);
  signal other : std_logic_vector(din'range);
  signal len   : std_logic_vector(din'length downto 0);
begin
  -- Equal whatever `W` is.
  reg <= din;
  dout <= reg;
  other <= din;
  wide <= din & reg;
  big <= '0' & din;
  len <= '1' & din;
  -- One element more or less, whatever `W` is.
  reg <= big;
  dout <= din(W - 2 downto 0);
  wide <= din & big;
  -- Different generics: 8 and 4 at their default values.
  small <= din;
  -- Out of range whatever `W` is.
  dout(0) <= din(W);
  dout <= big(W + 1 downto 1);
  dout(1) <= small(DEPTH);
  -- In range: not reported.
  dout(2) <= din(W - 1);
  level <= din(ADDR downto 0);
  -- Under an `if` generate, defaults are not used.
  g : if W = DEPTH generate
    small <= din;
  end generate;
end architecture;
-- file: top.vhd
library ieee;
use ieee.std_logic_1164.all;
use ieee.numeric_std.all;
use work.widths_pkg.all;
entity top is
  generic (DATA_W : natural := 12);
  port (
    a : in std_logic_vector(DATA_W - 1 downto 0);
    y : out std_logic_vector(DATA_W - 1 downto 0));
end entity;
architecture rtl of top is
  signal b8, c8 : std_logic_vector(7 downto 0);
  signal b16    : std_logic_vector(BUS_W - 1 downto 0);
  signal b32    : std_logic_vector(31 downto 0);
  signal lvl    : std_logic_vector(2 downto 0);
  signal lvl4   : std_logic_vector(3 downto 0);
  signal u      : unsigned(7 downto 0);
  signal u16    : unsigned(15 downto 0);
  signal mem    : mem_t;
  signal word   : std_logic_vector(7 downto 0);
  signal aw     : std_logic_vector(DATA_W downto 0);
  constant ZERO : std_logic_vector := x"000";
begin
  -- Generics from the generic map, by name and by position.
  f1 : entity work.fifo generic map (W => 16) port map (din => b16, dout => b16, level => lvl, wide => b32);
  f2 : entity work.fifo generic map (W => 16) port map (din => b8, dout => b16, level => lvl4, wide => b16);
  f3 : entity work.fifo generic map (8, 4, 3) port map (b8, c8, lvl4, b16);
  -- Defaults: `W` is 8.
  f4 : entity work.fifo port map (din => b16, dout => b8, level => lvl, wide => b16);
  -- The generics of the instantiating unit.
  f5 : entity work.fifo generic map (W => DATA_W) port map (din => a, dout => y, level => lvl, wide => open);
  f6 : entity work.fifo generic map (W => DATA_W + 1) port map (din => a, dout => aw, level => lvl, wide => open);
  -- A generic of a width that depends on another.
  f7 : entity work.fifo generic map (W => 4, INIT => x"00") port map (din => b8(3 downto 0), dout => open, level => lvl, wide => b8);
  -- An actual that is not a linear expression: not checked.
  f8 : entity work.fifo generic map (W => DATA_W / 3 + DATA_W mod 2) port map (din => b8, dout => b8, level => lvl, wide => b16);

  -- Functions of numeric_std.
  u <= resize(u16, 8);
  u <= resize(u16, 4);
  u16 <= to_unsigned(3, 16);
  u16 <= to_unsigned(3, 8);
  u16 <= u * u;
  u <= u * u;
  u16 <= resize(u, u16'length);
  u <= shift_left(u16, 2);
  b8 <= std_logic_vector(u + 1);
  b8 <= std_logic_vector(u16 + 1);

  -- Words of memories.
  word <= mem(1);
  b16 <= mem(2);
  mem(0) <= b16(7 downto 0);
  mem(3) <= b16;

  -- Constants of an unconstrained type take the width of their value.
  b8 <= ZERO;
  b16(11 downto 0) <= ZERO;

  -- Loop parameters: slices of the same width.
  gen : for i in 0 to 1 generate
    b16(i * 8 + 7 downto i * 8) <= b8;
    b16(i * 8 + 7 downto i * 8) <= b16;
  end generate;

  -- Comparisons.
  y(0) <= '1' when a = aw(DATA_W - 1 downto 0) else '0';
  y(1) <= '1' when a = aw else '0';
  y(2) <= '1' when a = x"000" else '0';
  y(3) <= '1' when b8 = b16(8 downto 0) else '0';
end architecture;

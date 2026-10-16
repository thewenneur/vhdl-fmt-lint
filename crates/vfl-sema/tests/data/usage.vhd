-- file: counter.vhd
-- Variables, generics and units used or not.
library ieee;
use ieee.std_logic_1164.all;
use ieee.numeric_std.all;

entity counter is
  generic (
    WIDTH  : positive := 8;
    UNUSED_LIMIT : natural := 0; -- reported: nothing reads it
    SPARE  : natural := 3        -- reported
  );
  port (
    clk : in  std_logic;
    q   : out unsigned(WIDTH - 1 downto 0)
  );
end entity;

architecture rtl of counter is
  signal count : unsigned(WIDTH - 1 downto 0);
begin
  q <= count;

  p_count : process (clk) is
    variable next_count : unsigned(WIDTH - 1 downto 0);
    variable idle       : boolean;              -- reported: never used
    variable a, b       : integer;              -- `b` reported, from a list
    variable written    : natural;              -- written only: used
  begin
    if rising_edge(clk) then
      next_count := count + 1;
      a          := 1;
      written    := a;
      count      <= next_count;
    end if;
  end process;
end architecture;

-- file: helpers.vhd
package helpers is
  function twice (x : integer) return integer;
end package;

package body helpers is
  function twice (x : integer) return integer is
    variable unused_tmp : integer; -- reported
    variable result     : integer;
  begin
    result := 2 * x;
    return result;
  end function;
end package body;

-- file: copy_a.vhd
-- Two files declare the same entity in `work`: both are reported.
entity twin is
end entity;

architecture a of twin is
begin
end architecture;

-- file: copy_b.vhd
entity twin is
end entity;

architecture b of twin is
begin
end architecture;

-- file: same_file.vhd
-- One file declaring a package twice.
package dup_pkg is
end package;

package dup_pkg is
end package;

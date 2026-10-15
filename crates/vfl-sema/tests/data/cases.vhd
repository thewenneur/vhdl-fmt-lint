-- file: pkg.vhd
package defs is
  subtype nibble_t is integer range 0 to 15;
end package;

-- file: cases.vhd
library ieee;
use ieee.std_logic_1164.all;
use ieee.numeric_std.all;
use work.defs.all;

entity cases is
  port (
    sel  : in std_logic_vector(1 downto 0);
    bits : in bit_vector(1 downto 0);
    s    : in std_logic;
    u    : in unsigned(2 downto 0);
    q    : out std_logic
  );
end entity;

architecture rtl of cases is
  type state_t is (idle, load, run, done);
  constant LAST : integer := 3;
  signal state : state_t;
  signal n     : integer range 0 to LAST;
  signal nib   : nibble_t;
  signal k     : natural;
  signal v     : std_logic_vector(3 downto 0);
  type ctrl_t is record
    state : state_t;
    count : nibble_t;
  end record;
  signal ctrl  : ctrl_t;
begin
  process (all)
  begin
    -- Complete: every literal once.
    case state is
      when idle | load => q <= '0';
      when run to done => q <= '1';
    end case;
    -- Missing `done`, and `load` twice.
    case state is
      when idle => q <= '0';
      when load | run => q <= '1';
      when load => q <= '1';
    end case;
    -- Fine: `when others`.
    case state is
      when idle => q <= '0';
      when others => q <= '1';
    end case;
    -- Integer range: 3 missing, 1 overlapping.
    case n is
      when 0 => q <= '0';
      when 1 to 2 => q <= '1';
      when 2 => q <= '1';
    end case;
    -- Subtype from a package: 4 to 15 missing.
    case nib is
      when 0 to 3 => q <= '0';
    end case;
    -- `natural` has no static high bound: left alone.
    case k is
      when 0 => q <= '0';
    end case;
    -- std_logic: needs `when others`.
    case s is
      when '0' => q <= '0';
      when '1' => q <= '1';
    end case;
    -- std_logic_vector: needs `when others`; "01" twice.
    case sel is
      when "00" => q <= '0';
      when "01" => q <= '1';
      when b"01" => q <= '1';
      when "1-" => q <= '1';
    end case;
    -- bit_vector: every value.
    case bits is
      when "00" | "01" => q <= '0';
      when "10" | "11" => q <= '1';
    end case;
    -- A slice; x"A" is "1010".
    case v(3 downto 0) is
      when x"A" => q <= '0';
      when "1010" => q <= '1';
      when others => q <= '1';
    end case;
    -- Record elements.
    case ctrl.state is
      when idle | load | run => q <= '0';
    end case;
    case ctrl.count is
      when 0 to 14 => q <= '0';
    end case;
    -- Matching case: left alone.
    case? sel is
      when "1-" => q <= '1';
      when others => q <= '0';
    end case?;
  end process;

  with state select q <=
    '0' when idle,
    '1' when load | run;
end architecture;

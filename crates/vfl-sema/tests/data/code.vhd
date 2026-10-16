-- file: pkg.vhd
library ieee;
use ieee.std_logic_1164.all;

package code_pkg is
  constant BYTE : natural := 8;
  constant VERBOSE : boolean := false;
  function public_used (x : integer) return integer;
  function public_unused (x : integer) return integer;
end package;

package body code_pkg is
  function public_used (x : integer) return integer is
  begin
    return x;
  end function;

  function public_unused (x : integer) return integer is
  begin
    return public_unused(x - 1);   -- a recursive call is not a use
  end function;

  -- Private to the body, and never called.
  procedure private_unused is
  begin
  end procedure;
end package body;

-- file: ranges.vhd
library ieee;
use ieee.std_logic_1164.all;
use ieee.numeric_std.all;
use work.code_pkg.all;

entity ranges is
  generic (WIDTH : natural := 8);
  port (
    clk  : in  std_logic;
    din  : in  std_logic_vector(WIDTH - 1 downto 0);
    dout : out std_logic_vector(BYTE - 1 downto 0)
  );
end entity;

architecture rtl of ranges is
  type mem_t is array (0 to 3) of std_logic_vector(7 downto 0);
  subtype word_t is std_logic_vector(15 downto 0);
  signal s     : std_logic_vector(7 downto 0);
  signal w     : word_t;
  signal mem   : mem_t;
  signal up    : std_logic_vector(0 to 7);
  signal small : integer range 0 to 7 := 8;
  signal nat   : natural;
  signal u     : unsigned(7 downto 0);
  constant OK  : natural := 3;
  constant BAD : integer range 1 to 4 := 0;
begin
  dout <= s(8 downto 1);           -- outside
  dout <= s(0 to 7);               -- wrong direction
  dout(3 downto 0) <= s(3 downto 4);  -- null slice
  dout(0) <= s(BYTE);              -- index from a package constant
  dout(1) <= din(WIDTH);           -- index from a generic's default
  dout(2) <= w(16);
  dout(3) <= up(7) and s(OK);      -- in range
  dout(4) <= mem(4)(0);
  dout(5) <= mem(3)(7);            -- in range
  nat <= -1;
  small <= 7;                      -- in range
  dout(6) <= '1' when s = w else '0';      -- 8 elements against 16
  dout(7) <= '1' when s < "0000" else '0'; -- 8 against 4
  dout(0) <= '1' when u < "0000" else '0'; -- numeric_std compares values

  wide : if WIDTH > 8 generate
    dout(0) <= din(8);             -- only elaborated when WIDTH > 8
  end generate;

  process (clk)
  begin
    if rising_edge(clk) then
      for i in 0 to 8 loop
        s(i) <= '0';               -- `i` reaches 8
      end loop;
      for i in 0 to 7 loop
        s(i) <= '1';
      end loop;
    end if;
  end process;
end architecture;

-- file: dead.vhd
library ieee;
use ieee.std_logic_1164.all;
use work.code_pkg.all;

entity dead is
  port (a : in boolean; y : out integer);
end entity;

architecture rtl of dead is
  function local_used (x : integer) return integer is
  begin
    return x + 1;
    y <= 0;                        -- unreachable, never mind the error
  end function;

  function local_unused (x : integer) return integer;
  function local_unused (x : integer) return integer is
  begin
    return x;
  end function;

  constant VERBOSE_HERE : boolean := true;
begin
  process (a)
  begin
    if false then
      y <= 1;
    elsif a then
      y <= local_used(2) + public_used(3);
    end if;
    if VERBOSE then                -- a constant: with the `constants` option
      y <= 2;
    end if;
    if not VERBOSE_HERE then
      y <= 3;
    end if;
    for i in 0 to 3 loop
      if a then
        exit;
        y <= 4;                    -- unreachable
      end if;
      exit when a;
      y <= 5;                      -- reachable: the exit has a condition
    end loop;
  end process;

  y <= 6 when TRUE else 7;
end architecture;

-- file: synth.vhd
library ieee;
use ieee.std_logic_1164.all;
use std.textio.all;

entity synth is
  port (
    clk   : in  std_logic;
    level : in  real;
    q     : out std_logic
  );
end entity;

architecture rtl of synth is
  signal r : std_logic := '0';
  signal n : integer;
begin
  q <= r after 1 ns;

  process (clk)
    variable l : line;
  begin
    if rising_edge(clk) then
      for i in 0 to n loop
        r <= not r;
      end loop;
      write(l, string'("tick"));
    end if;
  end process;

  -- synthesis translate_off
  process
  begin
    wait for 10 ns;
  end process;
  -- synthesis translate_on
end architecture;

entity synth_tb is
end entity;

architecture sim of synth_tb is
  signal clk : bit := '0';
begin
  clk <= not clk after 5 ns;     -- a testbench: not RTL
end architecture;

-- file: casing.vhd
library ieee;
use ieee.std_logic_1164.all;

entity casing is
  port (Clk : in STD_LOGIC; q : out std_logic);
end entity;

architecture rtl of casing is
  signal Data_Valid : std_logic;
begin
  q <= data_valid;
  process (clk)
  begin
    if rising_edge(CLK) then
      Data_Valid <= not Data_Valid;
    end if;
  end process;
end architecture;

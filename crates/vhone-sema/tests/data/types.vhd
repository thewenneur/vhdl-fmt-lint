-- file: types_pkg.vhd
-- Valid code: nothing here may be reported.
library ieee;
use ieee.std_logic_1164.all;
use ieee.numeric_std.all;

package types_pkg is
  type state_t is (idle, busy, error);
  type rec_t is record
    valid : std_logic;
    data  : std_logic_vector(7 downto 0);
    count : natural;
  end record;
  type rec_array_t is array (natural range <>) of rec_t;
  type mem_t is array (0 to 15) of std_logic_vector(7 downto 0);
  type matrix_t is array (0 to 3, 0 to 3) of bit;
  type node_t;
  type node_ptr is access node_t;
  type node_t is record
    value : integer;
    next_node : node_ptr;
  end record;
  type counter_t is protected
    procedure increment (by : natural := 1);
    impure function value return natural;
    impure function value (offset : integer) return integer;
  end protected;
  subtype byte_t is std_logic_vector(7 downto 0);
  type small_t is range 0 to 255;
  type ratio_t is range 0.0 to 1.0;
  constant C_ZERO : byte_t := (others => '0');
  constant C_INIT : rec_t := (valid => '0', data => x"00", count => 0);
  constant C_POS : rec_t := ('1', "10101010", 3);
  function to_byte (n : natural) return byte_t;
  function to_byte (s : string) return byte_t;
  function first (r : rec_t) return std_logic;
  procedure log (msg : string; level : natural := 0);
  procedure log (value : integer; level : natural := 0);
  procedure set_rec (r : out rec_t; v : std_logic);
  function table (n : natural := 0) return mem_t;
end package;

package body types_pkg is
  type counter_t is protected body
    variable count : natural := 0;
    procedure increment (by : natural := 1) is
    begin
      count := count + by;
    end procedure;
    impure function value return natural is
    begin
      return count;
    end function;
    impure function value (offset : integer) return integer is
    begin
      return count + offset;
    end function;
  end protected body;

  function to_byte (n : natural) return byte_t is
  begin
    return std_logic_vector(to_unsigned(n, 8));
  end function;

  function to_byte (s : string) return byte_t is
    variable result : byte_t := (others => '0');
  begin
    for i in s'range loop
      if s(i) = '1' then
        result(i - s'low) := '1';
      end if;
    end loop;
    return result;
  end function;

  function first (r : rec_t) return std_logic is
  begin
    return r.data(0) and r.valid;
  end function;

  procedure log (msg : string; level : natural := 0) is
  begin
    report msg & " at level " & integer'image(level) severity note;
  end procedure;

  procedure log (value : integer; level : natural := 0) is
  begin
    log(to_string(value), level);
  end procedure;

  procedure set_rec (r : out rec_t; v : std_logic) is
  begin
    r := (valid => v, data => (others => v), count => 0);
  end procedure;

  function table (n : natural := 0) return mem_t is
  begin
    return (others => (others => '0'));
  end function;
end package body;

-- file: valid.vhd
library ieee;
use ieee.std_logic_1164.all;
use ieee.numeric_std.all;
use ieee.math_real.all;
use std.textio.all;
use work.types_pkg.all;

entity valid is
  generic (WIDTH : positive := 8; NAME : string := "dut");
  port (
    clk, rst : in  std_logic;
    din      : in  std_logic_vector(WIDTH - 1 downto 0);
    dout     : out std_logic_vector(WIDTH - 1 downto 0);
    flag     : out boolean
  );
end entity;

architecture rtl of valid is
  signal count : unsigned(WIDTH - 1 downto 0);
  signal state : state_t := idle;
  signal mem : mem_t;
  signal rec : rec_t := C_INIT;
  signal recs : rec_array_t(0 to 3);
  signal i_sig : integer range 0 to 15 := 0;
  signal m : matrix_t := (others => (others => '0'));
  signal small : small_t := 3;
  signal ratio : ratio_t := 0.5;
  signal t : time := 10 ns;
  shared variable counter : counter_t;
begin
  dout <= std_logic_vector(count) when rst = '0' else (others => '0');
  flag <= state = busy or count > 3;

  process (clk)
    variable v : integer := 0;
    variable r : real := 0.0;
    variable l : line;
    variable ptr : node_ptr;
    variable b : boolean;
    variable rv : rec_t;
  begin
    if rising_edge(clk) then
      set_rec(r.valid => rv.valid, r.data => rv.data, r.count => rv.count, v => '1');
      rec.data <= table(3);
      assert ('A' /= 'a') and (bit_vector'(B"1100") sll 1) = B"1000";
      write(l, 2.5, digits => 3);
      write(l, t, unit => ns);
      write(l, true);
      read(l, v);
      count <= count + 1;
      count <= count + unsigned(din);
      count <= resize(count * 2, WIDTH);
      mem(to_integer(count(3 downto 0))) <= din;
      mem(i_sig) <= x"FF";
      rec.data <= mem(3);
      rec.valid <= '1' when state = idle else '0';
      recs(0).count <= recs(1).count + 1;
      state <= busy when din(0) = '1' else error;
      v := v + 1;
      v := to_integer(unsigned(din)) mod 4;
      r := real(v) * 2.5 + sqrt(r);
      v := integer(ceil(log2(real(WIDTH))));
      small <= small + 1;
      ratio <= ratio * 0.5;
      t <= t + 5 ns;
      t <= 2 * t;
      b := din = C_ZERO;
      if din(0) ?= '1' then
        b := true;
      end if;
      b := rec = C_INIT;
      if din(0) then
        null;
      end if;
      if (din(1) and din(2)) = '1' then
        v := counter.value;
        v := counter.value(3);
        counter.increment;
        counter.increment(2);
      end if;
      write(l, string'("count = "));
      write(l, to_integer(count));
      write(l, din);
      writeline(output, l);
      log("hello");
      log(v, level => 2);
      log(msg => "x" & NAME);
      ptr := new node_t'(value => 1, next_node => null);
      ptr.value := ptr.value + 1;
      ptr.all.value := 0;
      dout <= to_byte(3) when WIDTH = 8 else din;
      assert first(rec) = '1' report "bad " & to_string(count) severity warning;
      case state is
        when idle => state <= busy;
        when busy | error => state <= idle;
      end case;
      case din(1 downto 0) is
        when "00" => v := 0;
        when others => v := 1;
      end case;
      for k in mem'range loop
        mem(k) <= std_logic_vector(to_unsigned(k, 8));
      end loop;
      for k in 0 to 3 loop
        m(k, k) <= '1';
      end loop;
      wait for 0 ns;
    end if;
  end process;

  gen : for k in 0 to 3 generate
    recs(k).valid <= din(k);
  end generate;

  u_sub : entity work.valid
    generic map (WIDTH => WIDTH, NAME => "sub")
    port map (clk => clk, rst => rst, din => din, dout => open, flag => open);
end architecture;

-- file: invalid.vhd
library ieee;
use ieee.std_logic_1164.all;
use ieee.numeric_std.all;
use work.types_pkg.all;

entity invalid is
  port (
    clk  : in  std_logic;
    din  : in  std_logic_vector(7 downto 0);
    dout : out std_logic_vector(7 downto 0)
  );
end entity;

architecture rtl of invalid is
  signal count : unsigned(7 downto 0);
  signal state : state_t;
  signal rec : rec_t;
  signal n : integer;
  signal r : real;
  signal b : boolean;
  signal mem : mem_t;
begin
  dout <= count;
  n <= din;
  r <= 1;
  b <= '1';
  state <= busy + 1;
  count <= count + din;
  rec.missing <= '0';
  rec <= (valid => '0', data => x"00", cnt => 0);
  dout <= to_byte(true);
  n <= to_integer(din);
  dout <= std_logic_vector(n);
  mem(din) <= din;
  dout <= "0000";
  b <= din(0) ?= '1';

  process (clk)
  begin
    if n then
      log("x", 1, 2);
      log(nothing => 3);
      log;
    end if;
    wait for 10;
    assert n report "n is zero";
  end process;

  u_self : entity work.invalid port map (clk => clk, din => count, dout => open);
end architecture;

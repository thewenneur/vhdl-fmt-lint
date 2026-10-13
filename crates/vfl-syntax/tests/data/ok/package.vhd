package types is
  type state_t is (idle, running, 'x', \Done\);
  type small is range -8 to 7;
  type real_t is range 0.0 to 1.0e3;
  type distance is range 0 to 1e9
    units
      nm;
      um = 1000 nm;
      mm = 1000 um;
    end units distance;
  type word_array is array (natural range <>) of bit_vector(31 downto 0);
  type matrix is array (0 to 3, 0 to 3) of integer;
  type pair is record
    a, b : integer;
    flag : boolean;
  end record pair;
  type pair_ptr is access pair;
  type int_file is file of integer;
  type node;
  subtype byte is bit_vector(7 downto 0);
  subtype resolved_bit is resolved std_ulogic;
  subtype nibble_index is integer range 0 to 15;
  constant ZERO : byte := x"00";
  constant DEFERRED : integer;
  signal global : bit;
  shared variable counter : integer := 0;
  file log_file : int_file open write_mode is "log.bin";
  alias first_bit is global;
  alias "and" is std.standard."and" [bit, bit return bit];
  attribute keep : boolean;
  attribute keep of global : signal is true;
  attribute keep of all : constant is false;
  function add (a, b : integer) return integer;
  pure function "+" (l : pair; r : pair) return pair;
  impure function now_ns return natural;
  procedure log (constant msg : in string; variable count : inout natural);
  type counter_t is protected
    procedure increment (by : integer := 1);
    impure function value return integer;
  end protected counter_t;
  group pins is (signal, signal <>);
  group clocks : pins (global);
  disconnect global : bit after 0 ns;
end package types;

package body types is
  constant DEFERRED : integer := 42;

  function add (a, b : integer) return integer is
  begin
    return a + b;
  end function add;

  pure function "+" (l : pair; r : pair) return pair is
    variable result : pair;
  begin
    result := (a => l.a + r.a, b => l.b + r.b, flag => l.flag or r.flag);
    return result;
  end "+";

  impure function now_ns return natural is
  begin
    return now / 1 ns;
  end;

  procedure log (constant msg : in string; variable count : inout natural) is
  begin
    report msg;
    count := count + 1;
  end procedure;

  type counter_t is protected body
    variable count : integer := 0;
    procedure increment (by : integer := 1) is
    begin
      count := count + by;
    end procedure;
    impure function value return integer is
    begin
      return count;
    end function;
  end protected body counter_t;
end package body types;

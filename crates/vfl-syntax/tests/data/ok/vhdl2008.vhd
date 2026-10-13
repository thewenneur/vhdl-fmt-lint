context project_ctx is
  library ieee;
  use ieee.std_logic_1164.all;
end context project_ctx;

context work.project_ctx;
library osvvm;
context osvvm.OsvvmContext;

package generic_fifo is
  generic (
    type element_t;
    DEPTH : positive;
    function to_string (e : element_t) return string is <>;
    package math is new work.math_pkg generic map (<>)
  );
  /* A block
     comment. */
  type store_t is array (0 to DEPTH - 1) of element_t;
end package;

package int_fifo is new work.generic_fifo
  generic map (element_t => integer, DEPTH => 16);

entity e2008 is
  generic (type T; G : integer := 0);
  port (x : in std_ulogic_vector; y : out (resolved) std_ulogic_vector(3 downto 0));
end entity;

architecture rtl of e2008 is
  function id generic (type U) parameter (u : U) return U is
  begin
    return u;
  end function;
  function id_int is new id generic map (U => integer);
  alias probe is << signal .tb.dut.s : std_ulogic >>;
  constant up : integer := << constant ^.^.c : integer >>;
  signal v : unsigned(7 downto 0) := 8UX"F";
  default clock is rising_edge(clk);
begin
  process (all)
    variable n : integer;
  begin
    n := 1 when x = "00" else 2;
    y <= "0000" when x ?= "--" else "1111";
    case? x is
      when "1-" => null;
      when others => null;
    end case?;
    if ?? x(0) then null; end if;
    with x select n := 0 when "00", 1 when others;
    s <= force '1';
    s <= release;
    if and x then null; end if;
  end process;

  gen : if a: G = 0 generate
    signal t : bit;
  begin
  end a;
  elsif b: G = 1 generate
  else c: generate
  end generate gen;

  gen_case : case G generate
    when zero: 0 =>
      u : entity work.x;
    when others =>
  end generate gen_case;

  u_inst : entity work.y port map (a => inertial x, b => open);

  check : assert always (req -> next ack);
  cover {req; ack};
end architecture rtl;

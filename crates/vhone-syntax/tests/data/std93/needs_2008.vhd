context c is
  library ieee;
end context;

architecture a of e is
  signal v : bit_vector(7 downto 0) := 8x"FF";
begin
  process (all)
  begin
    x := 1 when a else 2;
    case? v is when others => null; end case?;
    if ?? b then null; end if;
  end process;
  g : if a generate else generate end generate;
  /* comment */
end;

package p is
  constant context : integer := 1;
  constant view : integer := 2;
end;

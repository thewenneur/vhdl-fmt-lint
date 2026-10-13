architecture rtl of escapes is
begin
  a<=b;
  -- vfl: off
  table : process begin
    x <= "00";    y <= '1';
  end process;
  -- vfl: on
  c<=d;
  -- vfl: skip
  keep   <=   this   ;
  e<=f;
end;

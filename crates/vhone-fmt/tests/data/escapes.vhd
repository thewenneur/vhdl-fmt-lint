architecture rtl of escapes is
begin
  a<=b;
  -- vhone: off
  table : process begin
    x <= "00";    y <= '1';
  end process;
  -- vhone: on
  c<=d;
  -- vhone: skip
  keep   <=   this   ;
  e<=f;
end;

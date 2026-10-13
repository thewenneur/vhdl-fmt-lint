architecture a of e is
begin
  process
  begin
    if c then
      null;
  end process;

  other : process
  begin
    wait;
  end process;
end;

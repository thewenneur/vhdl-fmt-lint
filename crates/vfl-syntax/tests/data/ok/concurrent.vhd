architecture structure of top is
  signal s, t : bit;
begin
  s <= a and b;
  t <= guarded s after 1 ns;
  with sel select
    y <= a when "00",
         b when "01" | "10",
         c when others;
  z <= a when en = '1' else
       b when en2 = '1' else
       'Z';
  postponed assert s = t report "mismatch";
  check : postponed process (s) is
  begin
    null;
  end postponed process check;
  proc_call(s, t);
  labelled_call : proc_call(s, t);
  b1 : block (en = '1') is
    generic (N : natural);
    generic map (N => 4);
    port (p : in bit);
    port map (p => s);
    signal local : bit;
  begin
    local <= guarded p;
  end block b1;
  gen_for : for i in 0 to 3 generate
    signal tmp : bit;
  begin
    tmp <= x(i);
  end generate gen_for;
  gen_if : if WIDTH > 8 generate
    u : entity work.wide;
  end generate;
  gen_cfg : component comp port map (s);
end architecture structure;

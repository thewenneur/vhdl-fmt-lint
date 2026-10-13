configuration cfg of top is
  use work.all;
  for structure
    for u1 : comp
      use entity work.impl(rtl)
        generic map (N => 8)
        port map (a => a);
    end for;
    for all : other_comp
      use configuration work.other_cfg;
    end for;
    for others : third
      use open;
    end for;
    for gen_for(0)
      for u2 : comp
      end for;
    end for;
  end for;
end configuration cfg;

architecture a of e is
  for u1 : comp use entity work.impl;
  for all : comp2 use entity work.impl2(rtl) port map (x => y);
begin
end;

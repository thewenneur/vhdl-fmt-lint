library ieee;
use ieee.std_logic_1164.all;

entity and_gate is
    port (a, b : in std_logic; y : out std_logic);
end;

architecture rtl of and_gate is
begin
    y <= a and b;
end;

library ieee;
use ieee.std_logic_1164.all;

entity top is
    port (x0, x1 : in std_logic; y : out std_logic);
end;

architecture rtl of top is
begin
    u_and : entity work.and_gate port map (a => x0, b => x1, y => y);
end;

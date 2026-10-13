-- file: lib/util.vhd
package util is
  constant ONE : integer := 1;
end package;

-- file: lib/helper.vhd
use work.util.all;
package helper is
  constant TWO : integer := ONE + 1;
end package;

-- file: app/top.vhd
library mylib;
use mylib.helper.all;
library unisim;
use unisim.vcomponents.all;

entity app is
end entity;

architecture a of app is
  constant X : integer := TWO + mylib.util.ONE;
begin
  u : BUFG port map (I => X, O => open);
  assert X = 3 report integer'image(X + whatever);
end architecture;

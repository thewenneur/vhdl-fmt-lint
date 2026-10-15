library ieee;
use ieee.std_logic_1164.all;

entity parity is
port (
data : in std_logic_vector(7 downto 0);
odd : out std_logic
);
end;

architecture rtl of parity is
begin
odd <= data(0) xor data(1) xor data(2) xor data(3)
xor data(4) xor data(5) xor data(6) xor data(7);
end;

LIBRARY IEEE;USE IEEE.STD_LOGIC_1164.ALL;
use ieee.numeric_std.all ;
ENTITY Messy IS
GENERIC(WIDTH:POSITIVE:=8;DEPTH : natural:=4);
PORT(CLK,RST:IN STD_LOGIC;
D : in std_logic_vector(WIDTH-1 downto 0);Q:OUT std_logic_vector(WIDTH-1 downto 0));
END Messy;
architecture RTL of Messy is
signal r:std_logic_vector(WIDTH-1 downto 0);signal count : integer range 0 to DEPTH:=0;
type state_t is (IDLE,RUN,DONE);
signal state:state_t;
begin
process(CLK,RST)begin
if RST='1' then r<=(others=>'0');state<=IDLE;
elsif rising_edge(CLK) then
case state is
when IDLE=>state<=RUN;
when RUN=>if count=DEPTH then state<=DONE;else count<=count+1;end if;
when others=>null;
end case;
r<=D;
end if;
end process;
Q<=r when state=DONE else (others=>'0');
end;

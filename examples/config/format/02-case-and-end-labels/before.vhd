library IEEE;
use IEEE.STD_LOGIC_1164.all;

entity Edge_Detect is
    port (
        Clk    : in  std_logic;
        Sig_In : in  std_logic;
        Rise   : out std_logic
    );
end;

architecture Rtl of Edge_Detect is
    signal Sig_D : std_logic;
begin
    P_Delay : process (Clk)
    begin
        if Rising_Edge(Clk) then
            Sig_D <= Sig_In;
        end if;
    end process;

    Rise <= Sig_In and not Sig_D;
end Rtl;

library ieee;
use ieee.std_logic_1164.all;

entity sync_ff is
    port (
        clk : in  std_logic;
        rst : in  std_logic;
        d   : in  std_logic;
        q   : out std_logic
    );
end;

architecture rtl of sync_ff is
    signal meta : std_logic;
begin
    -- Two flip-flops in a row bring d into this clock domain.
    p_sync : process (clk)
    begin
        if rising_edge(clk) then
            if rst = '1' then
                meta <= '0';
                q    <= '0';
            else
                meta <= d;
                q    <= meta;
            end if;
        end if;
    end process;
end;

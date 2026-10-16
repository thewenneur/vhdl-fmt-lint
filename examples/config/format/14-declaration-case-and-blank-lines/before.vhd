library ieee;
use ieee.std_logic_1164.all;

entity Blinker is
    generic (Half_Period : positive := 4);
    port (Clk, Rst : in std_logic; Led : out std_logic);
end;

architecture RTL of blinker is
    type State_T is (Off, On_State);
    constant Max_Count : natural := half_period - 1;
    -- the state and the count
    signal State : state_t;
    signal Count : natural range 0 to MAX_COUNT;

begin

    P_Count : process (CLK)
    begin
        if rising_edge(clk) then
            if rst = '1' or count = max_count then
                Count <= 0;
            else
                count <= COUNT + 1;
            end if;

        end if;
    end process;
    -- toggles when the count wraps
    P_State : process (clk)
    begin
        if rising_edge(clk) and Count = Max_Count then
            if State = OFF then
                state <= on_state;
            else
                state <= off;
            end if;
        end if;
    end process;
    led <= '1' when state = ON_STATE else '0';

end;

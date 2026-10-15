library ieee;
use ieee.std_logic_1164.all;
use ieee.std_logic_unsigned.all;

entity counter is
    port (
        clk   : in  std_logic;
        en    : in  std_logic;
        load  : in  std_logic;
        value : in  std_logic_vector(7 downto 0);
        count : out std_logic_vector(7 downto 0);
        zero  : out std_logic
    );
end;

architecture rtl of counter is
    signal count_reg : std_logic_vector(7 downto 0);
    signal spare     : std_logic;
begin
    p_count : process (clk, en)
    begin
        if clk'event and clk = '1' then
            if load = '1' then
                count_reg <= value;
            elsif en = '1' then
                count_reg <= count_reg + 1;
            end if;
        end if;
    end process;

    p_zero : process (count_reg, value)
    begin
        if count_reg = 0 and load = '0' then
            zero <= '1';
        else
            zero <= '0';
        end if;
    end process;

    count <= count_reg;
end;

library ieee;
use ieee.std_logic_1164.all;
use ieee.numeric_std.all;

entity packet_counter is
    port (
        clk, rst                 : in  std_logic;
        packet_valid, packet_ok  : in  std_logic;
        enable, overflow_allowed : in  std_logic;
        count                    : out unsigned(15 downto 0)
    );
end;

architecture rtl of packet_counter is
    signal count_reg : unsigned(15 downto 0);
begin
    p_count : process (clk)
    begin
        if rising_edge(clk) then
            if rst = '1'
                    or (count_reg = x"FFFF"
                        and overflow_allowed = '0'
                        and enable = '1') then
                count_reg <= (others => '0');
            elsif packet_valid = '1'
                    and packet_ok = '1'
                    and enable = '1' then
                count_reg <= count_reg
                        + to_unsigned(1, count_reg'length)
                        + unsigned'(x"0000");
            end if;
        end if;
    end process;
    count <= count_reg       when enable = '1' and rst = '0' else
             (others => '0') when rst = '1' else
             count_reg;
end;

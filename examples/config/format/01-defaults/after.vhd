library ieee;
use ieee.std_logic_1164.all;
use ieee.numeric_std.all;

entity counter is
    generic (
        WIDTH       : natural := 8;
        RESET_VALUE : natural := 0
    );
    port (
        clk    : in  std_logic;
        rst    : in  std_logic;
        enable : in  std_logic;
        count  : out unsigned(WIDTH - 1 downto 0)
    );
end entity counter;

architecture RTL of counter is
    signal count_reg : unsigned(WIDTH - 1 downto 0);
    signal at_max    : std_logic                   ;
begin
    p_count : process (clk)
    begin
        if rising_edge(clk) then
            if rst = '1' then
                count_reg <= to_unsigned(RESET_VALUE, WIDTH);
            elsif enable = '1' then
                count_reg <= count_reg + 1                  ; -- wraps around
            end if;
        end if;
    end process;
    at_max <= '1' when count_reg = (count_reg'range => '1') else '0';
    count  <= count_reg                                             ;
end architecture;

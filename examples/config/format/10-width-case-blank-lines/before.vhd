library ieee;
use ieee.std_logic_1164.all;


entity decoder is
    port (sel : in std_logic_vector(1 downto 0); enable : in std_logic; y : out std_logic_vector(3 downto 0));
end;



architecture rtl of decoder is
begin
    p_decode : process (sel, enable)
    begin
        case sel is
            when "00" => y <= "0001";
            when "01" => y <= "0010";
            when "10" => y <= "0100";
            when others => y <= "1000";
        end case;
        if (enable = '0') then
            y <= "0000";
        end if;
    end process;
end;

library ieee;
use ieee.std_logic_1164.all;

entity top is
    port (select_in : in std_logic_vector(1 downto 0); decoded_out : out std_logic_vector(3 downto 0));
end;

architecture rtl of top is
begin
    u_short : entity work.decoder port map (select_in, '0', open);
    u_long : entity work.decoder port map (sel => select_in, enable => '1', y => decoded_out);
end;

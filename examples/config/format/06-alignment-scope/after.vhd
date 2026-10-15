library ieee;
use ieee.std_logic_1164.all;
use ieee.numeric_std.all;

entity uart_tx is
    generic (
        CLKS_PER_BIT : positive := 434;
        DATA_BITS    : positive := 8
    );
    port (
        clk   : in  std_logic;
        rst   : in  std_logic;
        start : in  std_logic;
        data  : in  std_logic_vector(DATA_BITS - 1 downto 0);
        busy  : out std_logic; -- high while sending
        tx    : out std_logic
    );
end;

architecture rtl of uart_tx is
    type state_t is (IDLE, SENDING);
    signal state : state_t := IDLE;

    signal shift      : std_logic_vector(DATA_BITS + 1 downto 0);
    signal tick_count : natural range 0 to CLKS_PER_BIT - 1;
begin
    p_send : process (clk)
    begin
        if rising_edge(clk) then
            if rst = '1' then
                state <= IDLE;
                shift <= (others => '1');
            else
                case state is
                    when IDLE =>
                        if start = '1' then
                            shift      <= '1' & data & '0';
                            tick_count <= 0;
                            state      <= SENDING;
                        end if;
                    when SENDING =>
                        if tick_count = CLKS_PER_BIT - 1 then
                            tick_count <= 0;
                            shift      <= '1' & shift(shift'high downto 1);
                        else
                            tick_count <= tick_count + 1;
                        end if;
                end case;
            end if;
        end if;
    end process;

    tx   <= shift(0);
    busy <= '1' when state = SENDING else '0';
end;

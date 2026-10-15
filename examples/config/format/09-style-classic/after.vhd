LIBRARY ieee;
USE ieee.std_logic_1164.ALL;
USE ieee.numeric_std.ALL;

ENTITY uart_tx IS
    GENERIC (
        CLKS_PER_BIT : positive := 434;
        DATA_BITS    : positive := 8
    );
    PORT (
        clk   : IN  std_logic;
        rst   : IN  std_logic;
        start : IN  std_logic;
        data  : IN  std_logic_vector(DATA_BITS - 1 DOWNTO 0);
        busy  : OUT std_logic; -- high while sending
        tx    : OUT std_logic
    );
END ENTITY;

ARCHITECTURE rtl OF uart_tx IS
    TYPE state_t IS (IDLE, SENDING);
    SIGNAL state : state_t := IDLE;

    SIGNAL shift      : std_logic_vector(DATA_BITS + 1 DOWNTO 0);
    SIGNAL tick_count : natural RANGE 0 TO CLKS_PER_BIT - 1;
BEGIN
    p_send : PROCESS (clk)
    BEGIN
        IF rising_edge(clk) THEN
            IF rst = '1' THEN
                state <= IDLE;
                shift <= (OTHERS => '1');
            ELSE
                CASE state IS
                    WHEN IDLE =>
                        IF start = '1' THEN
                            shift      <= '1' & data & '0';
                            tick_count <= 0;
                            state      <= SENDING;
                        END IF;
                    WHEN SENDING =>
                        IF tick_count = CLKS_PER_BIT - 1 THEN
                            tick_count <= 0;
                            shift      <= '1' & shift(shift'high DOWNTO 1);
                        ELSE
                            tick_count <= tick_count + 1;
                        END IF;
                END CASE;
            END IF;
        END IF;
    END PROCESS;

    tx   <= shift(0);
    busy <= '1' WHEN state = SENDING ELSE '0';
END ARCHITECTURE;

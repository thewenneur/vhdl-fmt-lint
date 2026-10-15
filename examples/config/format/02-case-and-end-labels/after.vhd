LIBRARY ieee;
USE ieee.std_logic_1164.ALL;

ENTITY edge_detect IS
    PORT (
        clk    : IN  std_logic;
        sig_in : IN  std_logic;
        rise   : OUT std_logic
    );
END ENTITY edge_detect;

ARCHITECTURE rtl OF edge_detect IS
    SIGNAL sig_d : std_logic;
BEGIN
    p_delay : PROCESS (clk)
    BEGIN
        IF rising_edge(clk) THEN
            sig_d <= sig_in;
        END IF;
    END PROCESS p_delay;

    rise <= sig_in AND NOT sig_d;
END ARCHITECTURE rtl;

library ieee;
use ieee.std_logic_1164.all;

--! Deserializes frames of eight data bits and one stop bit, sampling each
--! bit in its middle.
entity uart_rx is
    generic (
        CYCLES_PER_BIT : positive := 868 --! Clock cycles per bit
    );
    port (
        clk   : in  std_logic                   ; --! Clock
        rst_n : in  std_logic                   ; --! Asynchronous reset, active low
        rxd   : in  std_logic                   ; --! Serial input
        data  : out std_logic_vector(7 downto 0); --! Received byte
        valid : out std_logic                   ; --! `data` holds a new byte, for one cycle
        error : out std_logic                     --! The stop bit was low
    );
end entity uart_rx;

architecture rtl of uart_rx is

    type state_t  is (IDLE, START, DATA_BIT, STOP)           ;

    --! The receiver's registers.
    type engine_t is record
        state   : state_t                              ;
        shift   : std_logic_vector(7 downto 0)         ;
        bit_idx : natural range 0 to 7                 ;
        timer   : natural range 0 to CYCLES_PER_BIT - 1;
    end record;

    signal engine, engine_next : engine_t                    ; --! Current and next registers
    signal rxd_sync            : std_logic_vector(1 downto 0); --! Synchronizer

begin

    --! Registers, with an asynchronous reset.
    regs : process (clk, rst_n) is
    begin
        if rst_n = '0' then
            engine.state <= IDLE             ;
            rxd_sync     <= "11"             ;
        elsif rising_edge(clk) then
            engine       <= engine_next      ;
            rxd_sync     <= rxd_sync(0) & rxd;
        end if;
    end process regs;

    --! Next-state logic.
    comb : process (all) is
    begin
        engine_next                         <= engine                                   ;
        valid                               <= '0'                                      ;
        error                               <= '0'                                      ;
        if engine.timer /= 0 then
            engine_next.timer               <= engine.timer - 1                         ;
        else
            case engine.state is
                when IDLE     =>
                    if rxd_sync(1) = '0' then
                        engine_next.timer   <= CYCLES_PER_BIT / 2                       ;
                        engine_next.state   <= START                                    ;
                    end if;
                when START    =>
                    engine_next.timer       <= CYCLES_PER_BIT - 1                       ;
                    engine_next.state       <= DATA_BIT when rxd_sync(1) = '0' else IDLE;
                when DATA_BIT =>
                    engine_next.timer       <= CYCLES_PER_BIT - 1                       ;
                    engine_next.shift       <= rxd_sync(1) & engine.shift(7 downto 1)   ;
                    if engine.bit_idx = 7 then
                        engine_next.bit_idx <= 0                                        ;
                        engine_next.state   <= STOP                                     ;
                    else
                        engine_next.bit_idx <= engine.bit_idx + 1                       ;
                    end if;
                when STOP     =>
                    valid                   <= rxd_sync(1)                              ;
                    error                   <= not rxd_sync(1)                          ;
                    engine_next.state       <= IDLE                                     ;
            end case;
        end if;
    end process comb;

    data <= engine.shift;

end architecture rtl;

library ieee;
use ieee.std_logic_1164.all;

use work.uart_pkg.all;

--! Serializes bytes: a start bit, the data bits LSB first, an optional
--! parity bit and a stop bit.
entity uart_tx is
  generic (
    CYCLES_PER_BIT : positive := 868;        --! Clock cycles per bit
    PARITY         : parity_t := PARITY_NONE --! Parity bit
  );
  port (
    clk   : in  std_logic;                    --! Clock
    rst_n : in  std_logic;                    --! Asynchronous reset, active low
    data  : in  std_logic_vector(7 downto 0); --! Byte to send
    valid : in  std_logic;                    --! `data` holds a byte to send
    ready : out std_logic;                    --! A byte is taken this cycle
    txd   : out std_logic                     --! Serial output
  );
end entity uart_tx;

architecture rtl of uart_tx is

  type state_t is (IDLE, START, DATA_BIT, PARITY_BIT, STOP);

  signal state   : state_t;                      --! Transmitter state
  signal shift   : std_logic_vector(7 downto 0); --! Bits left to send
  signal bit_idx : natural range 0 to 7;         --! Data bit being sent
  signal timer   : natural range 0 to CYCLES_PER_BIT - 1;

begin

  --! Steps through the frame, one bit every `CYCLES_PER_BIT` cycles.
  fsm : process (clk, rst_n) is
  begin
    if rst_n = '0' then
      state   <= IDLE;
      txd     <= '1';
      timer   <= 0;
      bit_idx <= 0;
    elsif rising_edge(clk) then
      if state = IDLE or timer = CYCLES_PER_BIT - 1 then
        timer <= 0;
        case state is
          when IDLE =>
            if valid = '1' then
              shift <= data;
              txd   <= '0';
              state <= START;
            end if;
          when START =>
            txd   <= shift(0);
            state <= DATA_BIT;
          when DATA_BIT =>
            if bit_idx = 7 then
              bit_idx <= 0;
              if PARITY = PARITY_NONE then
                txd   <= '1';
                state <= STOP;
              elsif PARITY = PARITY_EVEN then
                txd   <= even_parity(shift);
                state <= PARITY_BIT;
              else
                txd   <= not even_parity(shift);
                state <= PARITY_BIT;
              end if;
            else
              bit_idx <= bit_idx + 1;
              txd     <= shift(bit_idx + 1);
            end if;
          when PARITY_BIT =>
            txd   <= '1';
            state <= STOP;
          when STOP =>
            state <= IDLE;
        end case;
      else
        timer <= timer + 1;
      end if;
    end if;
  end process fsm;

  ready <= '1' when state = IDLE and valid = '1' else '0';

end architecture rtl;

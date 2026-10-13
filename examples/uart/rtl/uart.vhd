library ieee;
use ieee.std_logic_1164.all;

use work.uart_pkg.all;

--! A UART with a FIFO on each side.
--!
--! Bytes written to `tx_data` are queued and sent on `txd`; bytes received
--! on `rxd` are queued until read from `rx_data`.
entity uart is
  generic (
    CLK_HZ     : positive := 100_000_000; --! Clock frequency
    BAUD       : positive := 115_200;     --! Baud rate
    FIFO_DEPTH : positive := 16           --! Words in each FIFO
  );
  port (
    clk      : in  std_logic;                    --! Clock
    rst_n    : in  std_logic;                    --! Asynchronous reset, active low
    -- Transmit side
    tx_data  : in  std_logic_vector(7 downto 0); --! Byte to queue for sending
    tx_write : in  std_logic;                    --! Queue `tx_data`
    tx_full  : out std_logic;                    --! The transmit FIFO is full
    -- Receive side
    rx_data  : out std_logic_vector(7 downto 0); --! Oldest received byte
    rx_read  : in  std_logic;                    --! Take `rx_data`
    rx_empty : out std_logic;                    --! No received byte
    rx_error : out std_logic;                    --! A frame had a bad stop bit
    -- Serial lines
    txd      : out std_logic;                    --! Serial output
    rxd      : in  std_logic                     --! Serial input
  );
end entity uart;

architecture rtl of uart is

  constant CYCLES : positive := cycles_per_bit(CLK_HZ, BAUD); --! Clock cycles per bit

  signal rst      : std_logic;                    --! Synchronous reset for the FIFOs
  signal tx_byte  : std_logic_vector(7 downto 0); --! Next byte to send
  signal tx_empty : std_logic;
  signal tx_valid : std_logic;
  signal tx_ready : std_logic;
  signal rx_byte  : std_logic_vector(7 downto 0); --! Last received byte
  signal rx_valid : std_logic;

begin

  rst      <= not rst_n;
  tx_valid <= not tx_empty;

  --! Bytes waiting to be sent.
  tx_fifo : entity work.fifo
    generic map (WIDTH => 8, DEPTH => FIFO_DEPTH)
    port map (
      clk     => clk,
      rst     => rst,
      wr_en   => tx_write,
      wr_data => tx_data,
      rd_en   => tx_ready,
      rd_data => tx_byte,
      empty   => tx_empty,
      full    => tx_full
    );

  transmitter : entity work.uart_tx
    generic map (CYCLES_PER_BIT => CYCLES)
    port map (
      clk   => clk,
      rst_n => rst_n,
      data  => tx_byte,
      valid => tx_valid,
      ready => tx_ready,
      txd   => txd
    );

  receiver : entity work.uart_rx
    generic map (CYCLES_PER_BIT => CYCLES)
    port map (
      clk   => clk,
      rst_n => rst_n,
      rxd   => rxd,
      data  => rx_byte,
      valid => rx_valid,
      error => rx_error
    );

  --! Bytes received and not read yet.
  rx_fifo : entity work.fifo
    generic map (WIDTH => 8, DEPTH => FIFO_DEPTH)
    port map (
      clk     => clk,
      rst     => rst,
      wr_en   => rx_valid,
      wr_data => rx_byte,
      rd_en   => rx_read,
      rd_data => rx_data,
      empty   => rx_empty,
      full    => open
    );

end architecture rtl;

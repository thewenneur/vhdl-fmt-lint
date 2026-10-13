library ieee;
use ieee.std_logic_1164.all;

--! Sends a byte through the UART looped back on itself.
entity tb_uart is
end entity tb_uart;

architecture sim of tb_uart is

  signal clk      : std_logic := '0';
  signal rst_n    : std_logic := '0';
  signal tx_data  : std_logic_vector(7 downto 0);
  signal tx_write : std_logic := '0';
  signal rx_data  : std_logic_vector(7 downto 0);
  signal rx_empty : std_logic;
  signal serial   : std_logic; --! The loop-back wire

begin

  clk <= not clk after 5 ns;

  dut : entity work.uart
    generic map (CLK_HZ => 1_000_000, BAUD => 100_000)
    port map (
      clk      => clk,
      rst_n    => rst_n,
      tx_data  => tx_data,
      tx_write => tx_write,
      tx_full  => open,
      rx_data  => rx_data,
      rx_read  => '0',
      rx_empty => rx_empty,
      host_clk => clk,
      rx_error => open,
      txd      => serial,
      rxd      => serial
    );

  --! Writes one byte and waits for it to come back.
  stimulus : process is
  begin
    wait for 20 ns;
    rst_n    <= '1';
    tx_data  <= x"5A";
    tx_write <= '1';
    wait until rising_edge(clk);
    tx_write <= '0';
    wait until rx_empty = '0';
    assert rx_data = x"5A" report "wrong byte received" severity failure;
    wait;
  end process stimulus;

end architecture sim;

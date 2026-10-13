library ieee;
use ieee.std_logic_1164.all;

--! Shared definitions of the UART.
package uart_pkg is

  --! Parity bit added after the data bits.
  type parity_t is (PARITY_NONE, PARITY_EVEN, PARITY_ODD);

  constant DATA_BITS : positive := 8; --! Data bits per frame

  --! The number of clock cycles per bit for a clock and baud rate.
  function cycles_per_bit(clk_hz : positive; baud : positive) return positive;

  --! Even parity of a byte.
  function even_parity(data : std_logic_vector) return std_logic;

end package uart_pkg;

package body uart_pkg is

  function cycles_per_bit(clk_hz : positive; baud : positive) return positive is
  begin
    return clk_hz / baud;
  end function;

  function even_parity(data : std_logic_vector) return std_logic is
    variable p : std_logic := '0';
  begin
    for i in data'range loop
      p := p xor data(i);
    end loop;
    return p;
  end function;

end package body uart_pkg;

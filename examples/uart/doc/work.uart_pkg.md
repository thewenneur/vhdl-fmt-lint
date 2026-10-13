# Package `uart_pkg`

Library `work` · `rtl/uart_pkg.vhd:5`

Shared definitions of the UART.

<a id="constants"></a>

## Constants

| Name | Type | Value | Description |
|---|---|---|---|
| `DATA_BITS` | `positive` | `8` | Data bits per frame |

<a id="types"></a>

## Types

| Name | Kind | Definition | Description |
|---|---|---|---|
| `parity_t` | type | `(PARITY_NONE, PARITY_EVEN, PARITY_ODD)` | Parity bit added after the data bits. |

<a id="subprograms"></a>

## Functions and procedures

| Name | Signature | Description |
|---|---|---|
| `cycles_per_bit` | `function cycles_per_bit(clk_hz : positive; baud : positive) return positive` | The number of clock cycles per bit for a clock and baud rate. |
| `even_parity` | `function even_parity(data : std_logic_vector) return std_logic` | Even parity of a byte. |

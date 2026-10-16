# UART example specification

5 units · 8 requirements, 3 verified

<a id="units"></a>

## Units

| Unit | Kind | Requirements | Summary |
|---|---|---|---|
| [`fifo`](work.fifo.md) | entity | 0 | A synchronous first-in first-out buffer. |
| [`pulse_sync`](work.pulse_sync.md) | entity | 0 | Carries single-cycle pulses from one clock domain to another. |
| [`uart`](work.uart.md) | entity | 3 | A UART with a FIFO on each side. |
| [`uart_rx`](work.uart_rx.md) | entity | 2 | Deserializes frames of eight data bits and one stop bit, sampling each bit in its middle. |
| [`uart_tx`](work.uart_tx.md) | entity | 3 | Serializes bytes: a start bit, the data bits LSB first, an optional parity bit and a stop bit. |

<a id="requirements"></a>

## Requirements

| ID | Unit | Requirement | Verified by |
|---|---|---|---|
| `UART-1` | [`uart`](work.uart.md#requirements) | Sends the bytes written to `tx_data` on `txd` in the order they were written, and receives the bytes of `rxd` in the order they came. | `tb_uart.stimulus` `tb/tb_uart.vhd:40` |
| `UART-2` | [`uart`](work.uart.md#requirements) | Holds up to `FIFO_DEPTH` bytes each way: `tx_full` and `rx_empty` say when a side can take or give no byte. |  |
| `UART-3` | [`uart`](work.uart.md#requirements) | A frame with a bad stop bit gives one pulse of `rx_error`, one `host_clk` cycle long. |  |
| `RX-1` | [`uart_rx`](work.uart_rx.md#requirements) | Samples each bit `CYCLES_PER_BIT / 2` cycles after its start, and ignores a start bit that is high again by then. |  |
| `RX-2` | [`uart_rx`](work.uart_rx.md#requirements) | Gives each byte on `data` with `valid` high for one cycle, or `error` high for one cycle when the stop bit is low. | `tb_uart.stimulus` `tb/tb_uart.vhd:40` |
| `TX-1` | [`uart_tx`](work.uart_tx.md#requirements) | Idles high, then sends a frame per byte: a start bit (low), the eight data bits LSB first, a parity bit unless `PARITY` is `PARITY_NONE`, and a stop bit (high). | `tb_uart.stimulus` `tb/tb_uart.vhd:40` |
| `TX-2` | [`uart_tx`](work.uart_tx.md#requirements) | Each bit lasts `CYCLES_PER_BIT` clock cycles. |  |
| `TX-3` | [`uart_tx`](work.uart_tx.md#requirements) | `ready` is high in the cycle `data` is taken, only while no frame is being sent. |  |

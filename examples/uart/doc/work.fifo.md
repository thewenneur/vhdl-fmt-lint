# Entity `fifo`

Library `work` · `rtl/fifo.vhd:8`

Uses `ieee.std_logic_1164`, `ieee.numeric_std`.

A synchronous first-in first-out buffer.

Writes to a full FIFO and reads from an empty one are ignored.

![Symbol of fifo](work.fifo.symbol.svg)

<a id="generics"></a>

## Generics

| Name | Type | Default | Description |
|---|---|---|---|
| `WIDTH` | `positive` | `8` | Bits per word |
| `DEPTH` | `positive` | `16` | Number of words |

<a id="ports"></a>

## Ports

| Name | Direction | Size | Description |
|---|---|---|---|
| `clk` | in | 1 | Clock |
| `rst` | in | 1 | Synchronous reset, active high |
| `wr_en` | in | 1 | Write `wr_data` this cycle |
| `wr_data` | in | `WIDTH` (default 8) | Word to write |
| `rd_en` | in | 1 | Take the oldest word |
| `rd_data` | out | `WIDTH` (default 8) | Oldest word |
| `empty` | out | 1 | No word to read |
| `full` | out | 1 | No room to write |

<a id="users"></a>

## Instantiated by

- [`uart(rtl)`](work.uart.md#arch-rtl) as `tx_fifo`
- [`uart(rtl)`](work.uart.md#arch-rtl) as `rx_fifo`

<a id="arch-rtl"></a>

## Architecture `rtl`

### Data flow

![Data flow of fifo(rtl)](work.fifo.rtl.dataflow.svg)

### Combinational logic

![Combinational logic of fifo(rtl)](work.fifo.rtl.combinational.svg)

### Processes

| Process | Sensitivity | Clock | Drives | Description |
|---|---|---|---|---|
| `pointers` | `clk` | `clk` | `head`, `tail`, `count`, `memory` | Moves the pointers and stores written words. |

### Signals

| Name | Type | Default | Description |
|---|---|---|---|
| `memory` | `memory_t` |  |  |
| `head` | `natural range 0 to DEPTH - 1` | `0` | Next word to write |
| `tail` | `natural range 0 to DEPTH - 1` | `0` | Next word to read |
| `count` | `natural range 0 to DEPTH` | `0` | Words stored |

### Types

| Name | Kind | Definition |
|---|---|---|
| `memory_t` | type | `array (0 to DEPTH - 1) of std_logic_vector(WIDTH - 1 downto 0)` |

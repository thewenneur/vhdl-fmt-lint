#!/usr/bin/env bash
# Fills fuzz/corpus/<target>/ with seeds from the repository's VHDL and
# Verilog files: the examples and the test data. The multi-file targets
# also get the UART example as one project, and a mixed VHDL and Verilog
# project, with and without an edit script.
# Usage: fuzz/seed.sh
set -euo pipefail
cd "$(dirname "$0")"

sources() {
  find ../examples ../crates/*/tests/data -type f -size -16k \( "$@" \) | sort
}
mapfile -t vhdl < <(sources -name '*.vhd' -o -name '*.vhdl')
mapfile -t verilog < <(sources -name '*.v' -o -name '*.vh' -o -name '*.sv' -o -name '*.svh')
seed() { # target prefix files...
  local target=$1 prefix=$2
  shift 2
  mkdir -p "corpus/$target"
  for file in "$@"; do
    name=$(echo "$file" | sed 's#^\.\./##; s#/#_#g')
    { printf '%b' "$prefix"; cat "$file"; } >"corpus/$target/$name"
  done
}
for target in parse format; do
  seed "$target" '' "${vhdl[@]}"
done
seed verilog '' "${verilog[@]}"
seed format_verilog '' "${verilog[@]}"
# Files of one project start with \x01 (VHDL) or \x02 (Verilog); a \0
# starts the edit script (see src/lib.rs): 5 bytes per edit.
for target in lint reanalyze lsp; do
  seed "$target" '' "${vhdl[@]}"
  seed "$target" '\x02' "${verilog[@]}"
done

# A file including another, which includes the first.
printf '`include "inc1.vh"\nmodule top #(parameter W = `WIDTH) (input [W-1:0] a, output y);\n  assign y = ^a;\nendmodule\n\x01`ifndef INC1\n`define INC1\n`define WIDTH 8\n`include "inc0.vh"\n`endif\n' \
  >corpus/verilog/include_cycle

uart=(../examples/uart/rtl/uart_pkg.vhd ../examples/uart/rtl/uart_tx.vhd
  ../examples/uart/rtl/uart_rx.vhd ../examples/uart/rtl/uart.vhd)
project() {
  local first=1
  for file in "${uart[@]}"; do
    [ $first = 1 ] || printf '\x01'
    first=0
    cat "$file"
  done
}
mixed() {
  printf '%s\n' 'library ieee;' 'use ieee.std_logic_1164.all;' 'entity top is' \
    '  port (clk : in std_logic; q : out std_logic_vector(7 downto 0));' 'end entity;' \
    'architecture rtl of top is' 'begin' '  u_fifo : entity work.fifo' \
    '    generic map (DEPTH => 8)' '    port map (clk => clk, din => x"00", dout => q);' \
    'end architecture;'
  printf '\x02'
  printf '%s\n' 'module fifo #(parameter DEPTH = 16, parameter W = 8) (' \
    '  input logic clk,' '  input logic [W-1:0] din,' '  output logic [W-1:0] dout' \
    ');' '  always_ff @(posedge clk) dout <= din;' 'endmodule'
}
edits='\0\x00\x00\x00\x00\x00\x10\x00\x02\x00\x00\x13\x01\x00\x40\x01\x02'
for target in lint reanalyze lsp; do
  project >"corpus/$target/uart_project"
  { project; printf '%b' "$edits"; } >"corpus/$target/uart_project_edits"
  mixed >"corpus/$target/mixed_project"
  # Edits to the Verilog module (file 1): a port name, a parameter.
  { mixed; printf '\0\x00\x01\x00\x20\x04\x39\x01\x00\x30\x00\x00'; } \
    >"corpus/$target/mixed_project_edits"
done
echo "seeded ${#vhdl[@]} VHDL and ${#verilog[@]} Verilog files"

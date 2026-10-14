#!/usr/bin/env bash
# Fills fuzz/corpus/<target>/ with seeds from the repository's VHDL files:
# the examples and the test data. The multi-file targets also get the
# UART example as one project, with and without an edit script.
# Usage: fuzz/seed.sh
set -euo pipefail
cd "$(dirname "$0")"

targets=(parse format lint reanalyze lsp)
mapfile -t files < <(
  find ../examples ../crates/*/tests/data -type f \( -name '*.vhd' -o -name '*.vhdl' \) \
    -size -16k | sort
)
for target in "${targets[@]}"; do
  mkdir -p "corpus/$target"
  for file in "${files[@]}"; do
    name=$(echo "$file" | sed 's#^\.\./##; s#/#_#g')
    cp "$file" "corpus/$target/$name"
  done
done

# Files of one project are separated by \x01; a \0 starts the edit script
# (see src/lib.rs): 5 bytes per edit.
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
for target in lint reanalyze lsp; do
  project >"corpus/$target/uart_project"
  # A few edits to the first files.
  { project; printf '\0\x00\x00\x00\x00\x00\x10\x00\x02\x00\x00\x13\x01\x00\x40\x01\x02'; } \
    >"corpus/$target/uart_project_edits"
done
echo "seeded ${#files[@]} files into ${targets[*]}"

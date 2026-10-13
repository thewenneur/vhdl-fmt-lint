#!/usr/bin/env bash
# Parses pinned open-source VHDL projects and fails on any syntax error or
# round-trip mismatch. Usage: scripts/corpus.sh [CACHE_DIR]
set -euo pipefail

cache=${1:-.corpus}
mkdir -p "$cache"

# name  url  commit
repos=(
  "OSVVM   https://github.com/OSVVM/OSVVM.git          716d751566b4fb6479f69c38cc01790744c911b7"
  "UVVM    https://github.com/UVVM/UVVM.git            90d56e93c542bf0d5e2ab9f791cc7395bd1aa896"
  "vunit   https://github.com/VUnit/vunit.git          98063a438c957c5627d0003ca476bd4c4913ff64"
  "neorv32 https://github.com/stnolting/neorv32.git    b2620454ebb2407e81a6a1965ef52e2a860b0432"
  "ghdl    https://github.com/ghdl/ghdl.git            2c451de76b07b73fd338c91d68fd5cefcb152512"
)

for entry in "${repos[@]}"; do
  read -r name url commit <<<"$entry"
  dir="$cache/$name"
  if [ "$(git -C "$dir" rev-parse HEAD 2>/dev/null)" != "$commit" ]; then
    rm -rf "$dir"
    git init -q "$dir"
    git -C "$dir" fetch -q --depth 1 "$url" "$commit"
    git -C "$dir" checkout -q FETCH_HEAD
  fi
done

cargo build --release -q -p vfl-syntax --example parse_corpus
run=target/release/examples/parse_corpus
status=0

# OSVVM uses VHDL-2019 in a few packages; its *_template.vhd files hold `$` placeholders.
$run --quiet --std 2019 --exclude _template "$cache/OSVVM" || status=1
$run --quiet --std 2008 "$cache/UVVM" || status=1
$run --quiet --std 2008 "$cache/vunit" || status=1
$run --quiet --std 2008 "$cache/neorv32" || status=1
# The VESTS VHDL-93 conformance suite (compliant tests only). One file is
# an incomplete book excerpt and does not parse.
$run --quiet --std 93 --exclude ch_18_fg_18_09 \
  "$cache/ghdl/testsuite/vests/vhdl-93/ashenden/compliant" \
  "$cache/ghdl/testsuite/vests/vhdl-93/billowitch/compliant" \
  "$cache/ghdl/testsuite/vests/vhdl-93/clifton-labs/compliant" || status=1
# The whole GHDL test suite, broken files included: nothing may panic or
# fail to round-trip.
$run --quiet --allow-errors "$cache/ghdl/testsuite" || status=1

exit $status

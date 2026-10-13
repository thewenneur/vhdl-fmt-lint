#!/usr/bin/env bash
# Parses pinned open-source VHDL projects and fails on any syntax error or
# round-trip mismatch. Then formats them, with the default config and a very
# different one, and fails on any formatter error or output that changes
# when formatted again. Then lints them and fails on more semantic errors
# than the known ones. Last, checks that re-analysis after edits, reusing the
# previous analysis, matches a full analysis. Usage: scripts/corpus.sh [CACHE_DIR]
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
cargo build --release -q -p vfl-fmt --example fmt_corpus
cargo build --release -q -p vfl-cli
cargo build --release -q -p vfl-sema --example sema_corpus
target=${CARGO_TARGET_DIR:-target}
run=$target/release/examples/parse_corpus
fmt=$target/release/examples/fmt_corpus
status=0

# OSVVM uses VHDL-2019 in a few packages; its *_template.vhd files hold `$` placeholders.
$run --quiet --std 2019 --exclude _template "$cache/OSVVM" || status=1
$run --quiet --std 2008 "$cache/UVVM" || status=1
$run --quiet --std 2008 "$cache/vunit" || status=1
$run --quiet --std 2008 "$cache/neorv32" || status=1
# The VESTS VHDL-93 conformance suite (compliant tests only). One file is
# an incomplete book excerpt and does not parse.
vests=(
  "$cache/ghdl/testsuite/vests/vhdl-93/ashenden/compliant"
  "$cache/ghdl/testsuite/vests/vhdl-93/billowitch/compliant"
  "$cache/ghdl/testsuite/vests/vhdl-93/clifton-labs/compliant"
)
$run --quiet --std 93 --exclude ch_18_fg_18_09 "${vests[@]}" || status=1
# The whole GHDL test suite, broken files included: nothing may panic or
# fail to round-trip.
$run --quiet --allow-errors "$cache/ghdl/testsuite" || status=1

# Formatting. The output must parse to the same tokens and comments, and
# formatting it again must change nothing. Files with syntax errors are skipped.
alt_config=$(mktemp)
trap 'rm -f "$alt_config"' EXIT
printf '%s\n' \
  '[format]' \
  'keyword_case = "upper"' \
  'identifier_case = "lower"' \
  'use_tabs = true' \
  'indent = 4' \
  'end_label = "add"' \
  'end_keyword = "add"' \
  'max_width = 60' \
  'interface_list = "preserve"' \
  'association_list = "expand"' \
  'close_paren = "attached"' \
  'case_alternative = "break"' \
  '[format.align]' \
  'block_boundary = "never"' >"$alt_config"
for config in "" "--config $alt_config"; do
  # shellcheck disable=SC2086 # $config is meant to split.
  {
    $fmt --quiet $config --std 2019 --exclude _template "$cache/OSVVM" || status=1
    $fmt --quiet $config --std 2008 "$cache/UVVM" "$cache/vunit" "$cache/neorv32" || status=1
    $fmt --quiet $config --std 93 "${vests[@]}" || status=1
    $fmt --quiet $config "$cache/ghdl/testsuite" || status=1
  }
done

# Linting. Each project is analyzed as a whole, with zero configuration.
# The known semantic errors are real: OSVVM's demos call a removed `print`,
# UVVM's testbenches name DUTs that are not in the repository and one of its
# templates names what each VVC provides, and VUnit's OSVVM submodule is
# not checked out.
vfl=$(cd "$target/release" && pwd)/vfl
lint_errors() {
  (cd "$1" && "$vfl" lint --output-format concise 2>/dev/null || true) |
    grep -c ': error\[sema/' || true
}
for expected in "OSVVM 9" "UVVM 5" "vunit 37" "neorv32 0"; do
  read -r name known <<<"$expected"
  found=$(lint_errors "$cache/$name")
  if [ "$found" -gt "$known" ]; then
    echo "$name: $found semantic errors, expected at most $known" >&2
    (cd "$cache/$name" && "$vfl" lint --output-format concise 2>/dev/null | grep ': error\[sema/' >&2) || true
    status=1
  fi
done
# Incremental analysis: after edits to a spread of files, reusing the
# previous analysis must give exactly what a full analysis gives.
cargo build --release -q -p vfl-sema --example incremental_corpus
inc=$target/release/examples/incremental_corpus
$inc --files 4 "$cache/neorv32" "$cache/vunit" "$cache/UVVM" || status=1
$inc --files 4 --std 2019 "$cache/OSVVM" || status=1
$inc --files 8 --std 93 "${vests[@]}" || status=1

# Nothing may panic on the whole GHDL test suite.
$target/release/examples/sema_corpus "$cache/ghdl/testsuite" >/dev/null || status=1

exit $status

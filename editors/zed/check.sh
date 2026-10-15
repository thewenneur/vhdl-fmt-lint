#!/usr/bin/env bash
# Checks the Zed extension without Zed: builds it to WebAssembly, and runs
# every query in languages/ against the grammars at the revisions pinned in
# extension.toml, on the VHDL and Verilog files of the repository. Needs
# git, a C compiler, the tree-sitter CLI and the wasm32-wasip2 Rust target.
set -euo pipefail
cd "$(dirname "$0")"
repo=$(cd ../.. && pwd)

workspace_version=$(sed -n 's/^version = "\(.*\)"/\1/p' "$repo/Cargo.toml" | head -1)
for file in extension.toml Cargo.toml; do
  version=$(sed -n 's/^version = "\(.*\)"/\1/p' "$file" | head -1)
  if [[ "$version" != "$workspace_version" ]]; then
    echo "FAIL: $file has version $version, the workspace $workspace_version" >&2
    exit 1
  fi
done

for query in languages/systemverilog/*.scm; do
  if ! cmp -s "$query" "languages/verilog/$(basename "$query")"; then
    echo "FAIL: languages/verilog and languages/systemverilog differ in $(basename "$query")" >&2
    exit 1
  fi
done

cargo build --release --target wasm32-wasip2
echo "ok: extension builds"

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
mkdir "$work/grammars"
echo "{\"parser-directories\": [\"$work/grammars\"]}" > "$work/config.json"

# `name repository rev` for each [grammars.name] section.
grammars=$(awk '
  /^\[grammars\./ { name = $0; gsub(/^\[grammars\.|\]$/, "", name) }
  /^repository = / && name { repository = $3; gsub(/"/, "", repository) }
  /^rev = / && name { rev = $3; gsub(/"/, "", rev); print name, repository, rev; name = "" }
' extension.toml)

while read -r name repository rev; do
  dir="$work/grammars/tree-sitter-$name"
  git init -q "$dir"
  git -C "$dir" fetch -q --depth 1 "$repository" "$rev"
  git -C "$dir" checkout -q FETCH_HEAD
done <<< "$grammars"

check() {
  local language=$1 scope=$2
  shift 2
  for query in "languages/$language"/*.scm; do
    # A query that does not compile against the grammar is an error.
    tree-sitter query --config-path "$work/config.json" --scope "$scope" --quiet \
      "$query" "$@" > /dev/null
  done
  echo "ok: $language queries"
}

check vhdl source.vhdl "$repo"/examples/uart/rtl/*.vhd
check systemverilog source.systemverilog \
  "$repo"/crates/vfl-verilog-syntax/tests/data/ok/*.sv "$repo"/crates/vfl-verilog-syntax/tests/data/ok/*.v

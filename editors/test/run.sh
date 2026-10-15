#!/usr/bin/env bash
# Tests the editor configurations in editors/ with the editors that are
# installed: Neovim, Emacs (Eglot), Vim and Helix. Each test opens the files in
# fixture/ against `vfl lsp` and checks diagnostics and formatting (Helix:
# that it picks vfl for each language). `vfl` must be on the PATH, e.g.
#   cargo build -p vfl-cli && PATH=$PWD/target/debug:$PATH editors/test/run.sh
# With REQUIRE_ALL=1, a missing editor is a failure instead of a skip.
set -euo pipefail
cd "$(dirname "$0")"

command -v vfl > /dev/null || { echo "vfl is not on the PATH" >&2; exit 1; }

missing() {
  if [[ "${REQUIRE_ALL:-}" == 1 ]]; then
    echo "FAIL: $1 is not installed" >&2
    exit 1
  fi
  echo "skip: $1 is not installed"
}

if command -v nvim > /dev/null; then
  echo "== Neovim ($(nvim --version | head -1))"
  timeout 120 nvim --headless --clean -l neovim.lua 2>&1 | grep -E '^(ok|FAIL)'
else
  missing nvim
fi

if command -v emacs > /dev/null; then
  echo "== Emacs ($(emacs --version | head -1))"
  timeout 120 emacs --batch -Q -l emacs.el 2>&1 | grep -E '^(ok|FAIL)'
else
  missing emacs
fi

if command -v vim > /dev/null; then
  echo "== Vim ($(vim --version | head -1))"
  # Without an LSP plugin, sourcing must only define the autocommand.
  vim -es -N -u NONE -c 'source ../vim/vfl.vim' \
    -c 'if !exists("#vfl_vim_lsp#User#lsp_setup") | cquit 1 | endif' -c 'qall!'
  echo "ok: vfl.vim loads"
else
  missing vim
fi

if command -v hx > /dev/null; then
  echo "== Helix ($(hx --version))"
  config=$(mktemp -d)
  trap 'rm -rf "$config"' EXIT
  mkdir "$config/helix"
  cp ../helix/languages.toml "$config/helix/"
  for language in vhdl verilog; do
    health=$(XDG_CONFIG_HOME=$config hx --health "$language" 2>&1 | sed 's/\x1b\[[0-9;]*m//g')
    if ! grep -q "✓ vfl: " <<< "$health"; then
      echo "FAIL: Helix does not use vfl for $language:" >&2
      echo "$health" >&2
      exit 1
    fi
    echo "ok: $language uses vfl"
  done
else
  missing hx
fi

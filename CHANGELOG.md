# Changelog

## [0.1.0] - 2026-10-13

First release: a VHDL formatter, linter and language server in pure Rust, with editor
plugins for VS Code and Notepad++.

### Downloads

- `vfl-v0.1.0-<target>.tar.gz` / `.zip`: the `vfl` command-line tool for Linux (x86_64 and
  aarch64, static), macOS (Apple silicon and Intel) and Windows (x86_64).
- `vfl_npp-v0.1.0-x64.zip`: the Notepad++ plugin (64-bit). Unzip it into
  `Notepad++\plugins\`, so the DLL ends up in `plugins\vfl_npp\vfl_npp.dll`.
- `vfl-0.1.0.vsix`: the VS Code extension. Install it with
  `code --install-extension vfl-0.1.0.vsix`; it runs `vfl lsp`, so put `vfl` on the `PATH`
  or set `vfl.server.path`.
- `SHA256SUMS`: checksums of every file above.

### What is in it

- **Parser**: lossless, error-tolerant lexer and parser for VHDL-93, 2008 and 2019.
- **Formatter** (`vfl fmt`): idempotent and token-preserving, every rule configurable,
  including column alignment of ports, generics, assignments, associations, declarations
  and comments; `-- vfl: off` / `on` / `skip` regions.
- **Linter** (`vfl lint`): syntactic and cross-file semantic rules with autofixes, including
  name resolution, overload resolution and type checking, unused objects, width mismatches,
  inferred latches and clock domain crossings, naming and style rules; `recommended` and
  `strict` presets and per-line or per-file suppressions.
- **Zero-config projects**: the root, the sources and their libraries are discovered from
  the code. Library lists are imported from `vhdl_ls.toml`, VUnit `run.py`, FuseSoC
  `.core`, Vivado `.xpr` / Tcl and Quartus `.qsf` / `.qip` files when present, including
  Tcl variables in them.
- **Configuration**: layered `vfl.toml` files with `extends` and `root = true`; every key is
  in [docs/configuration.md](docs/configuration.md) and every rule in
  [docs/lints.md](docs/lints.md).
- **Language server** (`vfl lsp`): incremental re-analysis, diagnostics, formatting, quick
  fixes, hover with types, signature help, completion, go to definition, references,
  rename and symbols.
- **VS Code extension**: syntax highlighting and the language server.
- **Notepad++ plugin**: formatter, linter and the language server's editor features built
  in, no separate `vfl` install needed.
- **Performance**: 1.5 to 2.3 times faster than rust_hdl with 2.8 to 4.1 times less memory
  on neorv32, OSVVM, UVVM and VUnit ([docs/benchmarks.md](docs/benchmarks.md)).

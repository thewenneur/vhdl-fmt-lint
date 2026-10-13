# Changelog

## Unreleased

### Added

- **Semantic highlighting**: the language server colors names by what they resolve to
  (signal, port, variable, constant, generic, type, function, procedure, entity, component,
  package, library, enumeration literal, label), with modifiers for a port's or
  parameter's mode, declarations, assignment targets and names from `std` / `ieee`. Whole
  file, range and delta requests are supported; the largest UVVM file (20,000 names) takes
  7 ms. The VS Code extension declares the VHDL-specific token types with standard
  fallbacks, so any theme colors them. The Notepad++ plugin colors signals, ports,
  constants, variables, types and subprograms (**Color names by meaning**, on by default).
- **Range formatting**: format only a selection (`textDocument/rangeFormatting`,
  `vfl fmt --lines FIRST:LAST`, **Format selection** in Notepad++). The selection widens to
  the whole statements it touches and to every line aligned with them, so alignment stays
  consistent, and formatting the same lines again changes nothing.
- **Format on type**: the line of a `;` just typed, or the line above a new line, is
  formatted (`textDocument/onTypeFormatting`). Off by default in the editors
  (`vfl.format.onType` in VS Code, **Format on type** in Notepad++); `on_type = false` under
  `[format]` in `vfl.toml` turns it off for a project. Only the block around the line, between
  blank lines, is laid out, and lines are left alone while the file has syntax errors.
- Formatting requests no longer wait for the project to be analyzed again.

## [0.1.1] - 2026-10-13

A faster language server: editing, opening and hovering in large projects no longer wait for
a full re-analysis. The downloads are the same as in 0.1.0, with `0.1.1` in their names.

### Changed

- **Analysis once per burst of keystrokes**: the language server applies every edit already
  waiting before it analyzes, and skips requests the editor cancelled meanwhile. Before, it
  analyzed after every keystroke, and on large projects requests queued behind those
  analyses (hover took 860 ms after typing a comment in UVVM).
- **Only what an edit can change is refreshed**: library inference runs again only when the
  files, what they declare or reference, or a library list changed; linting gathers
  declarations only for the files it re-lints; only the diagnostics that changed are sent.
  New files and libraries are still detected.
- **Opening or closing an unchanged file does no work**: the first hover in a newly opened
  file no longer waits for a full refresh (70 to 190 ms on UVVM), and closing a file no
  longer walks the project tree. The Notepad++ plugin gets the same fix.

### Added

- `scripts/bench_vhdl_ls.py` benchmarks the language server against rust_hdl's `vhdl_ls`
  (open time, edit-to-diagnostics latency, hover, completion, definition, memory); results
  are in [docs/benchmarks.md](docs/benchmarks.md).
- The release workflow can also be started from the Actions tab (**Release > Run
  workflow** on `main`): it tags the current version and publishes it.

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

# Changelog

The extension is released with vfl and has its version. This file lists the changes to the
extension itself; the language server's, which the extension's features come from, are in
the [vfl changelog](https://github.com/thewenneur/vhdl-fmt-lint/blob/main/CHANGELOG.md).

## [Unreleased]

## [0.14.1] - 2026-11-19

### Changed

- The schema for `vfl.toml` has the new defaults of `[generate] reset_pattern` and
  `active_low_pattern`.

## [0.14.0] - 2026-11-18

### Added

- The schema for `vfl.toml` has `[generate] testbench_dut`.

## [0.13.1] - 2026-11-17

### Fixed

- minimatch 11 instead of 10 (GHSA-g3hv-75wg-m249).

## [0.13.0] - 2026-11-17

### Added

- The schema for `vfl.toml` lists the 77 new lint rules and their options (see the vfl
  changelog).

## [0.12.0] - 2026-11-16

### Added

- `lint_delay_ms` under `[lsp]` in `vfl.toml` (in the schema too): problems wait for
  typing to pause that long (see the vfl changelog).

## [0.11.1] - 2026-11-16

### Changed

- **vfl: Show Diagrams** and **Generate Documentation**: more direct state machine
  diagrams (see the vfl changelog).

## [0.11.0] - 2026-11-05

### Changed

- **vfl: Show Diagrams** and **Generate Documentation**: a combinational logic diagram per
  architecture (see the vfl changelog).

## [0.10.0] - 2026-11-05

### Changed

- **vfl: Show Diagrams**: state machines write `else`, wrap long conditions and cross
  less (see the vfl changelog).

## [0.9.0] - 2026-10-16

### Added

- **vfl: Generate Specification** writes the design specification of the workspace
  (`vfl spec`), themed like the documentation.

### Changed

- **Formatting no longer scrolls the editor**: only the changed lines are replaced.

## [0.8.0] - 2026-10-16

### Added

- **Snippets** for VHDL: `entity` (with its architecture), `architecture`, clocked
  processes with a synchronous (`process-sync`) or asynchronous (`process-async`) reset or
  none (`process-clocked`), `process-comb`, `fsm` (an architecture with a state machine),
  `fsm-process`, `testbench`, `for-generate`, `if-generate`, `package` (with its body) and
  `record`.
- **Language status**: with a VHDL or Verilog file in front, the `{}` item of the status bar
  shows the server's version and state, with **Restart**, and the `vfl.toml` in effect, with
  **Open** (or "no vfl.toml", with **Generate**).
- **`vfl.toml` completion and checking** with the
  [Even Better TOML](https://marketplace.visualstudio.com/items?itemName=tamasfe.even-better-toml)
  extension: the extension gives it a JSON schema of every key.
- **Server download**: without `vfl` on the `PATH`, in `~/.cargo/bin` or in
  `vfl.server.path`, the extension offers to download the release binary of its own version,
  checks it against the release's `SHA256SUMS` and keeps it in its global storage.
- **Multi-root workspaces**: one server per workspace folder, each with its own project.
- An icon.

### Changed

- vfl is the default formatter of Verilog and SystemVerilog files too.
- In a workspace you do not trust, the workspace's `vfl.server.path` and
  `vfl.server.extraEnv` are ignored.

## Earlier versions

See the [vfl changelog](https://github.com/thewenneur/vhdl-fmt-lint/blob/main/CHANGELOG.md).

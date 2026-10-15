# Changelog

The extension is released with vfl and has its version. This file lists the changes to the
extension itself; the language server's, which the extension's features come from, are in
the [vfl changelog](https://github.com/thewenneur/vhdl-fmt-lint/blob/main/CHANGELOG.md).

## [Unreleased]

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

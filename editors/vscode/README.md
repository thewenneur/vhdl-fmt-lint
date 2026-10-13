# vfl for VS Code

VHDL support for VS Code, powered by the [vfl](https://github.com/thewenneur/vhdl-fmt-lint)
language server:

- **Diagnostics** as you type: syntax errors, cross-file semantic checks and lint rules.
- **Quick fixes**, and **Fix All** (`vfl: Fix All Lint Findings in File`, or on save, below).
- **Formatting** with every rule from your `vfl.toml`.
- **Go to definition**, **find references**, **highlights**, **hover**, **rename**,
  **completion**, **document and workspace symbols**, across the whole project and into
  the bundled `std` and `ieee` libraries.
- **Syntax highlighting** for VHDL-93, 2008 and 2019.

The project root, sources and libraries are discovered from the code, so there is nothing to
set up. Configuration lives in `vfl.toml`, shared with the `vfl` command line; see the
[main README](https://github.com/thewenneur/vhdl-fmt-lint#configuration).

## Requirements

The extension runs `vfl lsp`, so `vfl` must be installed:

```sh
cargo install --git https://github.com/thewenneur/vhdl-fmt-lint vfl-cli
```

It is looked up in `vfl.server.path`, then on `PATH`, then in `~/.cargo/bin`.

## Settings

| Setting | Default | |
|---------|---------|---|
| `vfl.server.path` | `""` | Path to `vfl`. `~` and `${workspaceFolder}` are expanded. |
| `vfl.server.extraEnv` | `{}` | Extra environment variables for the server. |
| `vfl.trace.server` | `"off"` | Log LSP traffic (`messages` or `verbose`). |

The extension makes itself the default formatter for VHDL. To format and fix on save:

```json
"[vhdl]": {
  "editor.formatOnSave": true,
  "editor.codeActionsOnSave": { "source.fixAll.vfl": "explicit" }
}
```

## Commands

- `vfl: Restart Language Server`, after rebuilding `vfl` for instance.
- `vfl: Fix All Lint Findings in File` applies every safe fix.
- `vfl: Show Language Server Output`.

## Notes

The server analyzes the project around the first workspace folder. A file from elsewhere,
such as another folder of a multi-root workspace, is checked while it is open, but the files
around it are not loaded, so names it takes from them are not resolved.

## Development

```sh
npm ci
npm run check        # type-check
npm test             # grammar tests
npm run test:e2e     # runs VS Code against target/debug/vfl (build it first)
npm run package      # builds vfl-<version>.vsix
```

Install the package with `code --install-extension vfl-0.1.0.vsix`.

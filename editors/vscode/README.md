# vfl for VS Code

VHDL, Verilog and SystemVerilog support for VS Code, powered by the [vfl](https://github.com/thewenneur/vhdl-fmt-lint)
language server:

- **Diagnostics** as you type: syntax errors, cross-file semantic checks and lint rules.
- **Quick fixes**, and **Fix All** (`vfl: Fix All Lint Findings in File`, or on save, below).
  Besides each rule's own fix: rename a declaration to follow a naming convention
  everywhere, add the `use` clause for a name declared in a package, associate the ports
  an instantiation leaves out, or silence the rule
  on the line or in the file with a `-- vfl-ignore` comment.
- **Formatting** with every rule from your `vfl.toml`.
- **Go to definition**, **find references**, **highlights**, **hover** (with types),
  **signature help**, **rename**, **completion** (including record elements and protected
  type methods), **document and workspace symbols**, across the whole project and into
  the bundled `std` and `ieee` libraries.
- **Code generation**: insert an entity or component instantiation or a component
  declaration at the cursor, or generate a testbench (also offered as a code action on an
  entity declaration).
- **Documentation and diagrams**: generate the documentation of the project, or show the
  page of the current file's entity, with its symbol, block diagram, port maps, state
  machines and hierarchy, in a view beside the editor (the diagram button in the editor
  title bar).
- **Rename** and **find references** treat a package and its body, a subprogram and its
  body, and an entity and the components named after it (with their ports and generics) as
  one thing, work on record elements, and refuse a new name that would clash with another.
- **Syntax highlighting** for VHDL-93, 2008 and 2019, and **semantic highlighting** from
  the analysis: signals, ports (with their mode), variables, constants, generics, types,
  subprograms, entities, components, packages and enumeration literals each get their own
  token type. `signal`, `port`, `constant`, `generic`, `procedure`, `label`, `unit` and
  `attribute` fall back to standard types (`variable`, `parameter`, `function`, ...) in
  themes that do not know them; to pick your own colors, use
  `editor.semanticTokenColorCustomizations`, for example
  `"rules": { "signal": "#c586c0", "port.out": { "bold": true } }`.

**Verilog and SystemVerilog** files (`.v`, `.vh`, `.sv`, `.svh`) get syntax highlighting.
The rest is set by `mode` under `[verilog]` in `vfl.toml` (see
[configuration](https://github.com/thewenneur/vhdl-fmt-lint/blob/main/docs/configuration.md#verilog)):
by default (`mode = "names"`), VHDL that instantiates their modules resolves their ports and
parameters, with hover and go to definition; with `mode = "full"`, which is in progress and
says so once in a message, they also get syntax errors and lint findings as you type,
formatting (whole file, selection and on type), semantic highlighting, an outline of their
modules, parameters, ports, declarations and instances, and folding. To format them with
vfl, pick it with **Format Document With...**, or set it as `editor.defaultFormatter` under
`"[verilog]"` and `"[systemverilog]"`.

The project root, sources and libraries are discovered from the code, so there is nothing to
set up. Configuration lives in `vfl.toml`, shared with the `vfl` command line; see the
[main README](https://github.com/thewenneur/vhdl-fmt-lint#configuration).

## Requirements

The extension runs `vfl lsp`, so `vfl` must be installed: download it for your platform
from the [latest release](https://github.com/thewenneur/vhdl-fmt-lint/releases/latest), or
build it with Rust:

```sh
cargo install --git https://github.com/thewenneur/vhdl-fmt-lint vfl-cli
```

It is looked up in `vfl.server.path`, then on `PATH`, then in `~/.cargo/bin`.

## Settings

| Setting | Default | |
|---------|---------|---|
| `vfl.server.path` | `""` | Path to `vfl`. `~` and `${workspaceFolder}` are expanded. |
| `vfl.server.extraEnv` | `{}` | Extra environment variables for the server. |
| `vfl.format.onType` | `false` | Format the line you just finished as you type (`;` or Enter). |
| `vfl.trace.server` | `"off"` | Log LSP traffic (`messages` or `verbose`). |

The extension makes itself the default formatter for VHDL. To
format and fix on save:

```json
"[vhdl]": {
  "editor.formatOnSave": true,
  "editor.codeActionsOnSave": { "source.fixAll.vfl": "explicit" }
}
```

**Format Selection** formats the selected lines, and the whole statements and aligned lines
they touch. With `"vfl.format.onType": true`, typing `;` formats its line and Enter formats
the line above; `on_type = false` under `[format]` in `vfl.toml` turns it off for a project.

## Commands

- `vfl: Restart Language Server`, after rebuilding `vfl` for instance.
- `vfl: Fix All Lint Findings in File` applies every safe fix.
- `vfl: Show Language Server Output`.
- `vfl: Insert Entity Instantiation...`, `vfl: Insert Component Instantiation...` and
  `vfl: Insert Component Declaration...` ask for an entity of the project and insert the code
  at the cursor.
- `vfl: Generate Testbench...` writes `<entity>_tb.vhd` next to the entity and opens it.

- `vfl: Generate Documentation` (also in the explorer context menu of a folder) runs
  `vfl doc` on the project and offers to open the index: an HTML one in the browser, a
  Markdown or AsciiDoc one in the editor. Where and how it is written comes from `[doc]`
  in `vfl.toml`.
- `vfl: Show Diagrams`, the diagram button in the title bar of a VHDL editor, shows the
  documentation page of the first entity (else package) of the file, with its diagrams, in
  a view beside the editor. Diagrams zoom with the buttons over them, Ctrl or Cmd + wheel,
  or `+` `-` `0`, and pan by dragging. Links to other entities open their pages in the view. The page
  is drawn again when a VHDL file is saved, and follows the light or dark color theme. Its
  HTML is written to the extension's storage, not to the project.

The generated code follows `[format]` and `[generate]` in `vfl.toml`. Documentation is
only generated by these commands, never while you type.

## Notes

The server analyzes the project around the first workspace folder. A file from elsewhere,
such as another folder of a multi-root workspace, is checked while it is open, but the files
around it are not loaded, so names it takes from them are not resolved.

## Development

```sh
npm ci
npm run check        # type-check
npm test             # grammar and diagrams view tests
npm run test:e2e     # runs VS Code against target/debug/vfl (build it first)
npm run package      # builds vfl-<version>.vsix
```

Install the package with `code --install-extension vfl-0.6.0.vsix`.

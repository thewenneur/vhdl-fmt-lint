# vhdl-fmt-lint

A fast, fully configurable VHDL formatter, linter and language server, in pure Rust.

- **Formatter**: every rule configurable, including column alignment of ports, generics,
  assignments, associations, declarations and comments. Idempotent and token-preserving.
  Formats a whole file, a range of lines, or the line you just typed.
- **Linter**: 58 rules with autofixes: syntax, name resolution and type checking across the
  whole project, unused objects, width mismatches, inferred latches, clock domain
  crossings, design checks (missing or mixed resets, reset polarity, open inputs, `inout`
  ports used one way, unreachable or stuck state machine states), naming conventions and
  style. `recommended` and `strict` presets, every rule configurable.
- **FPGA vendor files**: Quartus settings (`.qsf`, `.qip`), SDC and Vivado XDC constraints,
  Vivado Tcl scripts, Microchip Libero and Lattice Radiant physical constraints (`.pdc`,
  `.ldc`), Lattice Diamond preferences (`.lpf`) and Gowin constraints (`.cst`) are linted too: syntax, misspelled commands, options, assignment
  and property names, missing arguments, files that do not exist, duplicate assignments,
  pins assigned twice and ports the design does not have. The editors highlight them. The
  same reader finds the libraries those files list, for the project discovery.
- **Documentation**: `vfl doc` generates HTML, Markdown or AsciiDoc documentation of the design
  from its doc comments, with entity symbols, the instantiation tree, the top level and every
  architecture as block diagrams wired by signal, port maps, data flow, clock domains and their
  crossings, reset trees, package dependencies, record and register bit fields, state machines
  and WaveDrom timing diagrams written in doc comments; `vfl schema` draws one diagram as SVG,
  Mermaid or DOT. In HTML pages every diagram zooms (buttons over it, Ctrl or Cmd + wheel,
  `+` `-` `0`) and pans by dragging.
  See [examples/uart/doc](examples/uart/doc/index.md) for an example.
- **Specification**: `vfl spec` writes a design specification per entity, from the code and
  its `@spec`, `@req` and `@verifies` comments: overview, interface with the clock each
  port is registered or sampled on, clocks and resets, behavior, state machines, registers,
  sub-blocks and requirements with the tests that verify them, and an index of every
  requirement. See the [guide](docs/spec.md) and [examples/uart/spec](examples/uart/spec/index.md).
  In VS Code, **vfl: Generate Documentation** and **vfl: Show Diagrams** (a button in the
  editor title bar, showing the current file's entity in a view beside it); in Notepad++,
  **Generate documentation** and **Show diagrams** in the plugin's menu.
- **Language server** for any LSP editor: diagnostics as you type, quick fixes, formatting,
  hover with types, signature help, completion, go to definition, implementation and type
  definition, references, rename across bodies and components, symbols, inlay hints with
  the formal names of positional maps and calls, call hierarchy and instance tree, "N
  instances" code lenses, linked editing of `end` names, and semantic highlighting of names
  by what they resolve to. Plugins for [VS Code](editors/vscode/README.md),
  [Notepad++](editors/notepadpp/README.md) and [Zed](editors/zed/README.md), and setup for
  [Neovim, Vim, Emacs, Helix, Sublime Text, Kate and JetBrains IDEs](docs/editors.md).
- **Code generation**: instantiations, component declarations and testbench skeletons from
  an entity, from the command line (`vfl new`) or the editor.
- **Zero config**: the project root, sources and libraries are discovered from the code.
  No library list to maintain.
- **VHDL-2008**, **VHDL-93** and **VHDL-2019**.
- **Verilog and SystemVerilog** (in progress): by default (`[verilog]`
  `mode = "names"`), VHDL resolves the Verilog modules it instantiates and nothing else
  is done with them; with `mode = "full"`, `vfl check` parses Verilog-1995 to 2005 and
  SystemVerilog-2005 to 2023 files with a lossless, error-tolerant parser that follows
  `` `ifdef `` branches, `vfl fmt` formats them with the same settings and alignment as
  VHDL, and `vfl lint` checks them: undeclared and implicit nets, unused and undriven
  signals, blocking assignments in clocked blocks, latches, incomplete sensitivity lists,
  missing `default`s, multiple drivers, widths, resets, state machines and instance
  connections. In the editor they get diagnostics, formatting, an outline, folding and
  semantic highlighting. In mixed projects, VHDL instantiates Verilog modules like
  entities: port maps are checked, hover and go to definition cross languages, and the
  generated documentation and diagrams include the modules.

Status: early development; see the [changelog](CHANGELOG.md) for what each release brings.

Documentation:

- [Command-line reference](docs/cli.md): every command, its options and its exit status,
  and what the language server offers.
- [Configuration reference](docs/configuration.md): every `vfl.toml` key, config lookup and
  layering, suppression comments.
- [Configuration guide by example](examples/config/README.md): one folder per topic with a
  `vfl.toml` and a file before and after `vfl fmt` or `vfl lint --fix`.
- [Design specifications](docs/spec.md): `vfl spec`, spec comments (`@spec`, `@req`,
  `@verifies`) and what the generated specification contains.
- [Lint rules](docs/lints.md): what each rule reports, its level in each preset, its fix and
  its options.
- [VS Code extension](editors/vscode/README.md) and [Notepad++ plugin](editors/notepadpp/README.md):
  features, commands and settings.
- [Editor setup](docs/editors.md): Zed, Neovim, Vim, Emacs, Helix, Sublime Text, Kate,
  JetBrains IDEs and any other LSP client.
- [Benchmarks](docs/benchmarks.md) against rust_hdl, and the [design](docs/design.md) of the
  parser, the analysis and the project discovery.

## Installation

### Command line

Download the archive for your platform from the
[latest release](https://github.com/thewenneur/vhdl-fmt-lint/releases/latest), unzip it and
put `vfl` (`vfl.exe` on Windows) somewhere on the `PATH`.

Or build it with Rust 1.88 or later ([rustup.rs](https://rustup.rs)):

```sh
cargo install --git https://github.com/thewenneur/vhdl-fmt-lint vfl-cli
vfl --version
```

This puts the `vfl` binary in `~/.cargo/bin`, which rustup adds to the `PATH`. To update, run
the same command with `--force`. From a clone, `cargo install --path crates/vfl-cli` does the
same.

### VS Code

The extension ([editors/vscode](editors/vscode/README.md)) runs `vfl lsp`. When it finds no
`vfl`, it offers to download the release binary of its own version (checked against the
release's `SHA256SUMS`), so installing `vfl` first (above) is optional. Then:

1. Download `vfl-0.8.0.vsix` from the
   [latest release](https://github.com/thewenneur/vhdl-fmt-lint/releases/latest).
2. Install it with `code --install-extension vfl-0.8.0.vsix`, or in VS Code with
   **Extensions > ... > Install from VSIX...**.
3. Open a folder with VHDL files. If `vfl` is not on the `PATH` or in `~/.cargo/bin`, set
   `vfl.server.path` to it, or accept the download.

To build the `.vsix` yourself: `cd editors/vscode && npm ci && npm run package`.

### Notepad++

The plugin ([editors/notepadpp](editors/notepadpp/README.md)) has the formatter, the linter
and the editor features of the language server (types on hover, parameter hints, completion,
quick fixes, go to definition, references, rename, symbols) built in, so it does not need
`vfl` installed. Download `vfl_npp-v0.8.0-x64.zip` (`-x86.zip` for 32-bit Notepad++,
`-arm64.zip` for ARM64) from the
[latest release](https://github.com/thewenneur/vhdl-fmt-lint/releases/latest), close Notepad++,
unzip it into the `plugins` folder of its install directory (usually
`C:\Program Files\Notepad++\plugins`, giving `plugins\vfl_npp\vfl_npp.dll`) and start it again.
The plugin's README covers building it and its commands.

### Zed, Neovim, Vim, Emacs, Helix, Sublime Text, Kate, JetBrains and others

Any editor with LSP support can use `vfl lsp` (over stdio) as the language server for VHDL,
Verilog and SystemVerilog files. [docs/editors.md](docs/editors.md) has the setup for each,
with ready-made files in [editors/](editors): a [Zed extension](editors/zed/README.md), a
Neovim config, an Emacs package for Eglot and lsp-mode, Vim plugin settings, Helix, Sublime
Text and Kate settings, and an LSP4IJ template for JetBrains IDEs. For Neovim 0.11+, for
instance:

```lua
vim.lsp.config("vfl", {
  cmd = { "vfl", "lsp" },
  filetypes = { "vhdl", "verilog", "systemverilog" },
  root_markers = { "vfl.toml", ".git" },
})
vim.lsp.enable("vfl")
```

## Usage

```sh
vfl project            # show the root, the sources and why each is in its library
vfl config show        # show the effective configuration and where it came from
vfl check [paths]      # report syntax errors
vfl fmt [--check] ...  # format in place, or list the files that would change
vfl fmt -              # format standard input to standard output
vfl fmt --lines 10:20 f.vhd  # format only those lines (and the statements around them)
vfl lint [--fix] ...   # lint, resolving names across the whole project
vfl rules              # list lint rules and their levels
vfl doc [-o dir]       # write HTML (or --format markdown|asciidoc) documentation with diagrams
vfl spec [-o dir]      # write the design specification, with its requirements
vfl schema block top   # draw one diagram: block, fsm, dataflow, clocks, resets, ...
vfl new tb counter     # write counter_tb.vhd, a testbench for entity counter
vfl new instance counter     # print an instantiation (also: component, component-instance)
vfl lsp                # language server over stdio
```

Every command and option is described in the [command-line reference](docs/cli.md).

## Configuration

Optional. Put a `vfl.toml` at the project root; nested `vfl.toml` files override only the
keys they set, `extends = "path.toml"` pulls in a shared file, and `root = true` stops the
search for parent configs. Every key is described in the
[configuration reference](docs/configuration.md), and every rule in [lints.md](docs/lints.md);
[examples/config](examples/config/README.md) shows them at work, before and after.

```toml
[format]
indent = 4
keyword_case = "upper"
end_label = "add"

[format.align]
scope = "contiguous"       # align consecutive lines only, not the whole block
semicolon = false          # leave the `;` after the last token
```

```toml
[lint]
preset = "strict"            # "none" | "recommended" (default) | "strict"

[lint.rules]
"sema/unused-port" = "off"
"style/line-length" = { max = 100 }
"naming/signal" = { prefix = "s_", case = "snake" }
```

A lint finding can be silenced with `-- vfl-ignore rule-id` at the end of its line (or on
the line above), or for a whole file with `-- vfl-ignore-file rule-id`.

Formatting can be switched off for a region with `-- vfl: off` / `-- vfl: on`, or for the next
statement with `-- vfl: skip`.

## Workspace

| Crate | Role |
|-------|------|
| `vfl-syntax`  | Lossless, error-tolerant lexer and parser |
| `vfl-config`  | Config schema and layered loading |
| `vfl-project` | Root discovery, sources, library inference |
| `vfl-sema`    | Semantic analysis |
| `vfl-fmt`     | Formatter |
| `vfl-lint`    | Linter |
| `vfl-vendor`  | Readers and checks of FPGA vendor files (`.qsf`, `.qip`, `.sdc`, `.xdc`, `.tcl`, `.pdc`, `.ldc`, `.lpf`, `.cst`) |
| `vfl-gen`     | Code generation: instantiations, components, testbenches |
| `vfl-doc`     | Documentation, specifications and diagrams (`vfl doc`, `vfl spec`, `vfl schema`) |
| `vfl-lsp`     | Language server |
| `vfl-cli`     | The `vfl` binary |
| `vfl-npp`     | [Notepad++ plugin](editors/notepadpp/README.md) (`editors/notepadpp`) |

## AI usage disclosure

Most of the code, tests and documentation in this repository are written by Claude
(Anthropic's AI model, through Claude Code), under the direction of the maintainer, who sets
the design, reviews every change and decides what is merged. Contributions are held to the
same standard whoever or whatever wrote them: CI, tests and review.

## License

Licensed under either of [Apache License 2.0](LICENSE-APACHE) or [MIT](LICENSE-MIT), at your option.

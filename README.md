# vhone

A fast, fully configurable VHDL formatter, linter and language server, in pure Rust.

> Formerly **vfl** (vhdl-fmt-lint). Existing `vfl.toml` files and `vfl-ignore` / `vfl: off`
> comments are still recognised.

- **Formatter**: every rule configurable, including column alignment of ports, generics,
  assignments, associations, declarations and comments. Idempotent and token-preserving.
- **Linter**: syntactic and cross-file semantic rules, with autofixes.
- **Documentation**: `vhone doc` generates HTML, Markdown or AsciiDoc documentation of the design
  from its doc comments, with entity symbols, the instantiation tree, the top level and every
  architecture as block diagrams wired by signal, port maps, data flow, clock domains and their
  crossings, reset trees, package dependencies, record and register bit fields, state machines
  and WaveDrom timing diagrams written in doc comments; `vhone schema` draws one diagram as SVG,
  Mermaid or DOT.
  See [examples/uart/doc](examples/uart/doc/index.md) for an example.
  In VS Code, **vhone: Generate Documentation** and **vhone: Show Diagrams** (a button in the
  editor title bar, showing the current file's entity in a view beside it); in Notepad++,
  **Generate documentation** and **Show diagrams** in the plugin's menu.
- **Language server**: diagnostics, formatting, navigation and refactoring in any LSP editor.
- **Code generation**: instantiations, component declarations and testbench skeletons from
  an entity.
- **Zero config**: the project root, sources and libraries are discovered from the code.
  No library list to maintain.
- **VHDL-2008**, **VHDL-93** and **VHDL-2019**.

Status: early development (first release: 0.1.0, see the [changelog](CHANGELOG.md)). See [docs/design.md](docs/design.md) for the design.

Documentation:

- [Configuration reference](docs/configuration.md): every `vhone.toml` key, config lookup and
  layering, suppression comments.
- [Lint rules](docs/lints.md): what each rule reports, its level in each preset, its fix and
  its options.

## Installation

### Command line

Download the archive for your platform from the
[latest release](https://github.com/thewenneur/vhone/releases/latest), unzip it and
put `vhone` (`vhone.exe` on Windows) somewhere on the `PATH`.

Or build it with Rust 1.88 or later ([rustup.rs](https://rustup.rs)):

```sh
cargo install --git https://github.com/thewenneur/vhone vhone-cli
vhone --version
```

This puts the `vhone` binary in `~/.cargo/bin`, which rustup adds to the `PATH`. To update, run
the same command with `--force`. From a clone, `cargo install --path crates/vhone-cli` does the
same.

### VS Code

The extension ([editors/vscode](editors/vscode/README.md)) runs `vhone lsp`, so install `vhone`
first (above). Then:

1. Download `vhone-0.1.1.vsix` from the
   [latest release](https://github.com/thewenneur/vhone/releases/latest).
2. Install it with `code --install-extension vhone-0.1.1.vsix`, or in VS Code with
   **Extensions > ... > Install from VSIX...**.
3. Open a folder with VHDL files. If `vhone` is not on the `PATH` or in `~/.cargo/bin`, set
   `vhone.server.path` to it.

To build the `.vsix` yourself: `cd editors/vscode && npm ci && npm run package`.

### Notepad++

The plugin ([editors/notepadpp](editors/notepadpp/README.md)) has the formatter, the linter
and the editor features of the language server (types on hover, parameter hints, completion,
quick fixes, go to definition, references, rename, symbols) built in, so it does not need
`vhone` installed. Download `vhone_npp-v0.1.1-x64.zip` from the
[latest release](https://github.com/thewenneur/vhone/releases/latest), close Notepad++,
unzip it into the `plugins` folder of its install directory (usually
`C:\Program Files\Notepad++\plugins`, giving `plugins\vhone_npp\vhone_npp.dll`) and start it again.
The plugin's README covers building it and its commands.

### Other editors

Any editor with LSP support can use `vhone lsp` (over stdio) as the VHDL language server.

Neovim 0.11+:

```lua
vim.lsp.config("vhone", {
  cmd = { "vhone", "lsp" },
  filetypes = { "vhdl" },
  root_markers = { "vhone.toml", ".git" },
})
vim.lsp.enable("vhone")
```

Helix (`languages.toml`):

```toml
[language-server.vhone]
command = "vhone"
args = ["lsp"]

[[language]]
name = "vhdl"
language-servers = ["vhone"]
```

## Usage

```sh
vhone project            # show the root, the sources and why each is in its library
vhone config show        # show the effective configuration and where it came from
vhone check [paths]      # report syntax errors
vhone fmt [--check] ...  # format in place, or list the files that would change
vhone fmt -              # format standard input to standard output
vhone fmt --lines 10:20 f.vhd  # format only those lines (and the statements around them)
vhone lint [--fix] ...   # lint, resolving names across the whole project
vhone rules              # list lint rules and their levels
vhone doc [-o dir]       # write HTML (or --format markdown|asciidoc) documentation with diagrams
vhone schema block top   # draw one diagram: block, fsm, dataflow, clocks, resets, ...
vhone new tb counter     # write counter_tb.vhd, a testbench for entity counter
vhone new instance counter     # print an instantiation (also: component, component-instance)
vhone lsp                # language server over stdio
```

## Configuration

Optional. Put a `vhone.toml` at the project root; nested `vhone.toml` files override only the
keys they set, `extends = "path.toml"` pulls in a shared file, and `root = true` stops the
search for parent configs. Every key is described in the
[configuration reference](docs/configuration.md), and every rule in [lints.md](docs/lints.md).

```toml
[format]
indent = 4
keyword_case = "upper"
end_label = "add"

[format.align]
block_boundary = "never"   # align whole port lists, across blank lines and comments
```

```toml
[lint]
preset = "strict"            # "none" | "recommended" (default) | "strict"

[lint.rules]
"sema/unused-port" = "off"
"style/line-length" = { max = 100 }
"naming/signal" = { prefix = "s_", case = "snake" }
```

A lint finding can be silenced with `-- vhone-ignore rule-id` at the end of its line (or on
the line above), or for a whole file with `-- vhone-ignore-file rule-id`.

Formatting can be switched off for a region with `-- vhone: off` / `-- vhone: on`, or for the next
statement with `-- vhone: skip`.

## Workspace

| Crate | Role |
|-------|------|
| `vhone-syntax`  | Lossless, error-tolerant lexer and parser |
| `vhone-config`  | Config schema and layered loading |
| `vhone-project` | Root discovery, sources, library inference |
| `vhone-sema`    | Semantic analysis |
| `vhone-fmt`     | Formatter |
| `vhone-lint`    | Linter |
| `vhone-gen`     | Code generation: instantiations, components, testbenches |
| `vhone-lsp`     | Language server |
| `vhone-cli`     | The `vhone` binary |
| `vhone-npp`     | [Notepad++ plugin](editors/notepadpp/README.md) (`editors/notepadpp`) |

## AI usage disclosure

Most of the code, tests and documentation in this repository are written by Claude
(Anthropic's AI model, through Claude Code), under the direction of the maintainer, who sets
the design, reviews every change and decides what is merged. Contributions are held to the
same standard whoever or whatever wrote them: CI, tests and review.

## License

Licensed under either of [Apache License 2.0](LICENSE-APACHE) or [MIT](LICENSE-MIT), at your option.

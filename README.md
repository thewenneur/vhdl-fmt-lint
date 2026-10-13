# vhdl-fmt-lint

A fast, fully configurable VHDL formatter, linter and language server, in pure Rust.

- **Formatter**: every rule configurable, including column alignment of ports, generics,
  assignments, associations, declarations and comments. Idempotent and token-preserving.
- **Linter**: syntactic and cross-file semantic rules, with autofixes.
- **Language server**: diagnostics, formatting, navigation and refactoring in any LSP editor.
- **Code generation**: instantiations, component declarations and testbench skeletons from
  an entity.
- **Zero config**: the project root, sources and libraries are discovered from the code.
  No library list to maintain.
- **VHDL-2008**, **VHDL-93** and **VHDL-2019**.

Status: early development (first release: 0.1.0, see the [changelog](CHANGELOG.md)). See [docs/design.md](docs/design.md) for the design.

Documentation:

- [Configuration reference](docs/configuration.md): every `vfl.toml` key, config lookup and
  layering, suppression comments.
- [Lint rules](docs/lints.md): what each rule reports, its level in each preset, its fix and
  its options.

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

The extension ([editors/vscode](editors/vscode/README.md)) runs `vfl lsp`, so install `vfl`
first (above). Then:

1. Download `vfl-0.1.1.vsix` from the
   [latest release](https://github.com/thewenneur/vhdl-fmt-lint/releases/latest).
2. Install it with `code --install-extension vfl-0.1.1.vsix`, or in VS Code with
   **Extensions > ... > Install from VSIX...**.
3. Open a folder with VHDL files. If `vfl` is not on the `PATH` or in `~/.cargo/bin`, set
   `vfl.server.path` to it.

To build the `.vsix` yourself: `cd editors/vscode && npm ci && npm run package`.

### Notepad++

The plugin ([editors/notepadpp](editors/notepadpp/README.md)) has the formatter, the linter
and the editor features of the language server (types on hover, parameter hints, completion,
quick fixes, go to definition, references, rename, symbols) built in, so it does not need
`vfl` installed. Download `vfl_npp-v0.1.1-x64.zip` from the
[latest release](https://github.com/thewenneur/vhdl-fmt-lint/releases/latest), close Notepad++,
unzip it into the `plugins` folder of its install directory (usually
`C:\Program Files\Notepad++\plugins`, giving `plugins\vfl_npp\vfl_npp.dll`) and start it again.
The plugin's README covers building it and its commands.

### Other editors

Any editor with LSP support can use `vfl lsp` (over stdio) as the VHDL language server.

Neovim 0.11+:

```lua
vim.lsp.config("vfl", {
  cmd = { "vfl", "lsp" },
  filetypes = { "vhdl" },
  root_markers = { "vfl.toml", ".git" },
})
vim.lsp.enable("vfl")
```

Helix (`languages.toml`):

```toml
[language-server.vfl]
command = "vfl"
args = ["lsp"]

[[language]]
name = "vhdl"
language-servers = ["vfl"]
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
vfl new tb counter     # write counter_tb.vhd, a testbench for entity counter
vfl new instance counter     # print an instantiation (also: component, component-instance)
vfl lsp                # language server over stdio
```

## Configuration

Optional. Put a `vfl.toml` at the project root; nested `vfl.toml` files override only the
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
| `vfl-gen`     | Code generation: instantiations, components, testbenches |
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

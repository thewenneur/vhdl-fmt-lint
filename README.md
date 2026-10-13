# vhdl-fmt-lint

A fast, fully configurable VHDL formatter, linter and language server, in pure Rust.

- **Formatter**: every rule configurable, including column alignment of ports, generics,
  assignments, associations, declarations and comments. Idempotent and token-preserving.
- **Linter**: syntactic and cross-file semantic rules, with autofixes.
- **Language server**: diagnostics, formatting, navigation and refactoring in any LSP editor.
- **Zero config**: the project root, sources and libraries are discovered from the code.
  No library list to maintain.
- **VHDL-2008**, **VHDL-93** and **VHDL-2019**.

Status: early development. See [docs/design.md](docs/design.md) for the design.

## Installation

```sh
cargo install --git https://github.com/thewenneur/vhdl-fmt-lint vfl-cli
```

This installs the `vfl` binary. For editors:

- **VS Code**: the extension in [editors/vscode](editors/vscode). Each CI run builds it as a
  `.vsix` (the `vfl-vscode` artifact); install it with `code --install-extension vfl-0.1.0.vsix`.
- **Any other LSP editor**: run `vfl lsp` as the language server for VHDL files.

## Usage

```sh
vfl project            # show the root, the sources and why each is in its library
vfl config show        # show the effective configuration and where it came from
vfl check [paths]      # report syntax errors
vfl fmt [--check] ...  # format in place, or list the files that would change
vfl fmt -               # format standard input to standard output
vfl lint [--fix] ...   # lint, resolving names across the whole project
vfl rules              # list lint rules and their levels
vfl lsp                # language server over stdio
```

## Editors

- **Notepad++**: a native plugin that lints as you type and formats; see
  [editors/notepadpp](editors/notepadpp/README.md).
- Any editor with LSP support: run `vfl lsp`.

## Configuration

Optional. Put a `vfl.toml` at the project root; nested `vfl.toml` files override only the
keys they set, `extends = "path.toml"` pulls in a shared file, and `root = true` stops the
search for parent configs. See the design doc for the full schema.

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

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

## Usage

```sh
vfl project            # show the discovered root and sources
vfl config show        # show the effective configuration and where it came from
vfl fmt [--check] ...  # format (not implemented yet)
vfl lint ...           # lint (not implemented yet)
vfl lsp                # language server (not implemented yet)
```

## Configuration

Optional. Put a `vfl.toml` at the project root; nested `vfl.toml` files override only the
keys they set, `extends = "path.toml"` pulls in a shared file, and `root = true` stops the
search for parent configs. See the design doc for the full schema.

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

## AI usage disclosure

Most of the code, tests and documentation in this repository are written by Claude
(Anthropic's AI model, through Claude Code), under the direction of the maintainer, who sets
the design, reviews every change and decides what is merged. Contributions are held to the
same standard whoever or whatever wrote them: CI, tests and review.

## License

Licensed under either of [Apache License 2.0](LICENSE-APACHE) or [MIT](LICENSE-MIT), at your option.

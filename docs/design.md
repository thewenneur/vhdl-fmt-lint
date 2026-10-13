# vhdl-fmt-lint: design

Status: draft, 2026-10-13. Step 1 of the project plan. Implemented so far: config schema and layering
(`vfl-config`), root discovery and source collection (`vfl-project`), the `vfl` CLI shell.

## Goals

- One pure-Rust toolchain for VHDL: a **library**, a **CLI** (`vfl`) and a **language server**.
- A **formatter** where every rule is configurable, including column alignment of
  keywords, assignments, ports, generics, declarations and trailing comments.
- A **linter** with syntactic and cross-file semantic rules, each configurable, many with autofixes.
- **Zero configuration** for the common case: open a file, the tool finds the project,
  the sources and their libraries. No hand-written library lists.
- VHDL-2008 first, then VHDL-93, then VHDL-2019, all behind one parser.
- Measurably better than rust_hdl (`vhdl_ls`) on speed, memory, correctness and features.

Non-goals for now: simulation, elaboration of generate loops with arbitrary generics,
Verilog/SystemVerilog, mixed-language projects (designed for, not built).

## Architecture

```
            ┌──────────┐   ┌──────────┐   ┌──────────┐
            │ vfl-cli  │   │ vfl-lsp  │   │ embedders│
            └────┬─────┘   └────┬─────┘   └────┬─────┘
                 └──────────────┼──────────────┘
                     ┌──────────┴──────────┐
                ┌────┴────┐           ┌────┴────┐
                │ vfl-fmt │           │vfl-lint │
                └────┬────┘           └────┬────┘
                     │                ┌────┴────┐
                     │                │vfl-sema │  name resolution, types, xrefs
                     │                └────┬────┘
                ┌────┴─────────────────────┴────┐
                │          vfl-project          │  root, files, libraries, config
                └────┬─────────────────────┬────┘
                ┌────┴─────┐          ┌────┴─────┐
                │vfl-syntax│          │vfl-config│
                └──────────┘          └──────────┘
```

| Crate        | Role |
|--------------|------|
| `vfl-syntax` | Lexer and lossless, error-tolerant parser producing a concrete syntax tree (CST). |
| `vfl-config` | Config schema, defaults, loading, layering (`extends`, nested configs). |
| `vfl-project`| Project root discovery, source collection, library inference. |
| `vfl-sema`   | Incremental semantic model: design units, scopes, name resolution, overloads, types. |
| `vfl-fmt`    | Formatter: CST to layout IR to text, with an alignment pass. |
| `vfl-lint`   | Rule framework and built-in rules (syntactic and semantic), fixes. |
| `vfl-lsp`    | Language server: diagnostics, formatting, go-to, hover, completion, rename. |
| `vfl-cli`    | The `vfl` binary: `vfl fmt`, `vfl lint`, `vfl check`, `vfl lsp`, `vfl project`. |

Each crate is usable as a library; only `vfl-cli` and `vfl-lsp` do I/O policy.

### Syntax tree

- **Lossless**: every byte of input, including whitespace and comments, is a token in the
  tree. Printing the tree reproduces the file exactly. This is what makes formatting,
  autofixes and refactorings safe.
- **Green/red tree** in the style of rust-analyzer, using the `rowan` crate: immutable,
  deduplicated green nodes; cheap parent-aware red cursors; typed AST wrappers generated
  from a grammar description.
- **Hand-written recursive descent** parser. Error recovery uses synchronization tokens
  (`;`, `end`, `begin`, `is`, declaration keywords) so one typo never hides the rest
  of the file. Parse errors are diagnostics, not failures.
- **One grammar, gated by standard.** The parser takes a `Standard` (`93`, `2008`, `2019`).
  Constructs from a newer standard still parse, but produce a "requires VHDL-2008" style
  diagnostic, which is better than a confusing syntax error.
- Case-insensitive identifiers are interned with a folded key; extended identifiers
  (`\Foo\`) keep their case.

### Semantic model

- Built on `salsa` (incremental, demand-driven queries). An edit re-parses one file and
  only recomputes what depends on its changed declarations, which keeps LSP latency low
  on large projects.
- Analysis is per design unit: primary units (entity, package, context, configuration)
  are indexed project-wide, secondary units (architecture, package body) attach to them.
- Standard libraries (`std`, `ieee`, per standard version) are bundled as VHDL sources and
  analyzed like user code. Vendor libraries (`unisim`, `altera_mf`, ...) can be pointed at
  or stubbed; missing ones degrade to "unknown" rather than cascading errors.
- Files parse and index in parallel (`rayon`).

### Formatter

1. **Token pass**: keyword/identifier casing, spacing around operators and punctuation,
   comment normalization, blank-line policy.
2. **Layout pass**: CST to a Wadler/Prettier-style document IR (groups, indents, soft
   line breaks), printed against `max_width`.
3. **Alignment pass**: after layout, consecutive lines with the same shape form an
   *alignment block* and are tabulated into columns. Targets: `:` and modes in port and
   generic lists, `:=` / `<=` / `=>`, `is` in declarations, `when` choices, trailing
   comments, and more. Each target can be turned on or off, and its block boundary is
   configurable (stop at blank lines, at comments, or never).

Guarantees, enforced in tests and at runtime:

- **Idempotent**: `fmt(fmt(x)) == fmt(x)`, checked on every corpus file and by fuzzing.
- **Semantics-preserving**: the non-trivia token stream before and after formatting
  must match (ignoring case where the config changes it). If it doesn't, the file is left
  untouched and a bug is reported, never a broken file written.
- **Range formatting** for the LSP (format selection, format on type).
- Escape hatches: `-- vfl: off` / `-- vfl: on` regions and `-- vfl: skip` for the next statement.

### Linter

- A `Rule` has an id (`naming/signal-prefix`), a category, a default severity, typed options,
  and optionally a fix (safe or unsafe).
- Syntactic rules walk the CST; semantic rules query `vfl-sema` (unused signals, missing
  sensitivity entries, unresolved names, width mismatches, ...).
- Suppression: `-- vfl-ignore rule-id` on a line, or `-- vfl-ignore-file rule-id`.
- Output formats: human, JSON, SARIF, GitHub annotations.

## Configuration

One TOML file, `vfl.toml` (or `.vfl.toml`). Everything has a default, so an empty file
or no file at all is valid. Example:

```toml
[project]
standard = "2008"            # "93" | "2008" | "2019"
exclude  = ["sim/generated/**"]

# Optional. Only needed when inference guesses wrong.
[project.libraries]
osvvm = ["third_party/osvvm/**/*.vhd"]

[format]
max_width       = 100
indent          = 2
keyword_case    = "lower"    # "lower" | "upper" | "preserve"
identifier_case = "preserve"
end_label       = "add"      # "add" | "remove" | "preserve"

[format.align]
port_colon      = true
port_mode       = true
generic_colon   = true
assignment      = true       # aligns <= and := in consecutive statements
association     = true       # aligns => in port/generic maps and aggregates
declaration     = true       # aligns names, ":" and types in declarative parts
trailing_comment = true
block_boundary  = "blank-line"   # "blank-line" | "comment" | "never"

[lint]
preset = "recommended"       # "none" | "recommended" | "strict"

[lint.rules]
"naming/signal-prefix" = { level = "warn", prefix = "s_" }
"style/no-buffer-ports" = "error"
"sema/unused-signal" = "off"
```

Lookup and layering:

- For a file, the nearest `vfl.toml` walking up from its directory applies.
- A nested config inherits from the ones above it and overrides only the keys it sets, so a
  subtree can relax rules without copying everything. `root = true` stops the upward search.
  `extends = "path.toml"` layers a shared team config underneath the file.
- CLI flags override the file (`--set format.indent=4`).
- `vfl config show <file>` prints the effective config with where each value came from.

## Project autodiscovery

The aim: open any VHDL file and get correct cross-file analysis, without a library list.

**1. Find the root.** Walk up from the file and stop at the first directory that has
`vfl.toml`, then the first that is a VCS root (`.git`, `.hg`, `.jj`). Fall back to the file's
own directory. In the LSP, workspace folders are also roots.

**2. Collect sources.** Every `*.vhd` / `*.vhdl` (configurable) under the root, honouring
`.gitignore` and `exclude` (via the `ignore` crate, the same walker as ripgrep).

**3. Import existing tool metadata when present.** If the tree already declares libraries,
use them: rust_hdl `vhdl_ls.toml`, FuseSoC `.core`, HDLmake `Manifest.py` patterns,
VUnit `run.py` (`add_library` calls, best effort), Vivado `.xpr` and Quartus `.qsf`.
This makes migration a no-op.

**4. Infer libraries from the code.** For everything else, every file is parsed (fast,
parallel) and indexed by the primary units it declares and the references it makes
(`library x; use x.pkg.all;`, `entity x.e`, `work.y`). Then:

- A unit referenced as `lib.unit` and declared in exactly one file puts that file in `lib`.
- `work.unit` references mean "same library as me", so library membership propagates along
  `work` edges until it reaches a fixed point.
- Files nothing names stay in `work`.
- A file used from two libraries can belong to both (it is analyzed once per library).
- Conflicts (one unit name declared in two files of the same library, or contradicting
  constraints) are resolved by directory proximity, and surfaced as a diagnostic that
  suggests a one-line `[project.libraries]` entry.

**5. Explain it.** `vfl project` prints the root, the files, the inferred libraries and why
each file landed where it did. The LSP exposes the same through a command.

Explicit entries in `[project.libraries]` always win over inference, so the escape hatch
is small and local rather than a full list.

## Beating rust_hdl

Tracked by a benchmark harness in CI against the latest `vhdl_ls` release, on a pinned
corpus (OSVVM, UVVM, VUnit, neorv32, the GHDL test suite, and others):

| Dimension | Target |
|-----------|--------|
| Cold full-project analysis time | faster |
| Peak memory | lower |
| LSP keystroke-to-diagnostics latency | under 50 ms on the largest corpus project |
| False errors on valid corpus code | fewer (tracked per release) |
| Setup | zero config vs. required `vhdl_ls.toml` |
| Features | formatter, autofixes, lossless refactorings, VHDL-2019 |

## Repository and CI

- Cargo workspace, edition 2024, MSRV pinned in `rust-toolchain.toml` and `Cargo.toml`.
- License: MIT OR Apache-2.0 (default choice, easy to change before the first release).
- CI (GitHub Actions): `cargo fmt --check`, `cargo clippy -D warnings`, tests on Linux,
  macOS and Windows; later corpus regression, fuzzing and benchmarks.
- Snapshot tests (`insta`) for parser trees, formatter output and diagnostics.

## Roadmap

1. Design and workspace (this document).
2. Parser: lexer, CST, VHDL-2008 grammar, then 93 and 2019 gating; corpus round-trip tests.
3. Formatter with full alignment; idempotency and token-preservation checks.
4. Project inference, semantic model, linter, CLI and LSP; benchmarks against rust_hdl.

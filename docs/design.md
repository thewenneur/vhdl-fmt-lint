# vhdl-fmt-lint: design

Status: draft, 2026-10-13. Implemented so far: config schema and layering (`vfl-config`), root
discovery and source collection (`vfl-project`), the lossless parser (`vfl-syntax`), the
formatter (`vfl-fmt`), and the `vfl` CLI with `vfl check` (syntax errors) and `vfl fmt`.

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
| `vfl-lsp`    | Language server: diagnostics, formatting, go-to, hover, signature help, completion, rename. |
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
- Reserved words depend on the standard: `context`, `force`, `release` and `parameter` are
  identifiers in VHDL-93, `view` and `private` are identifiers before VHDL-2019.
- PSL embedded in VHDL-2008 (`default clock`, `property`, `sequence`, `assert always ...`,
  `cover`, `vunit`) is kept as opaque `PSL_DIRECTIVE` nodes; it is not parsed further yet.
- Tree shape: one node per construct (`ENTITY_DECL`, `PROCESS_STMT`, `SIGNAL_ASSIGN_STMT`,
  ...). Trivia sits between nodes, never at a node's edges, so a comment before a
  declaration is a sibling of it. `prefix(...)` is always `CALL_EXPR`: telling calls,
  indexing, slices and constraints apart is left to semantic analysis.
- Testing: snapshot trees for valid and broken inputs, a check that every prefix of every
  test file (and every single-token deletion) still round-trips, and `scripts/corpus.sh`,
  which parses pinned OSVVM, UVVM, VUnit, neorv32 and the VESTS VHDL-93 suite with zero
  errors, and the whole GHDL test suite without a panic or round-trip failure. CI runs it.

### Semantic model

- Analysis is per design unit. Primary units (entity, package, context, configuration,
  package instance) are indexed per library; secondary units (architecture, package body)
  attach to them. A file in two libraries is analyzed once per library.
- Units resolve in dependency order: `std.standard` first, then levels of a topological
  sort, each level in parallel (`rayon`). A dependency cycle is broken at the unit with the
  fewest missing dependencies, so broken code still gets analyzed.
- Name resolution walks scopes from the innermost: local declarations of every enclosing
  region first, then `use`-imported names, then library names, as in the LRM. Overloaded
  names (subprograms, operators, enumeration literals) keep every visible candidate.
- Type checking runs after resolution, for every unit in parallel (it only reads the
  declarations of the units a unit depends on). Each declaration records its type
  (subtype mark, return type, parameters, type definition). Expressions are typed in two
  passes, as in the LRM: bottom-up, the set of types an expression may have; top-down,
  with the type its context needs, which picks the overload and narrows the reference to
  it. Predefined operators come from the operand types' classes (they are not declared in
  `std.standard`), and universal integer and real convert to any integer or real type.
  Errors: `sema/type-mismatch` (an expression of the wrong type) and
  `sema/no-matching-overload` (no subprogram or operator accepts the arguments, with the
  reason: arity, an unknown or repeated formal, a missing argument, or the first
  mismatching argument).
- The checker is conservative: a name from an unknown library, a generic type, an
  attribute it does not model or a literal without a typed context is "unknown", and
  unknown is compatible with everything. On the corpus (OSVVM, UVVM, VUnit, neorv32, VESTS)
  it reports no type error on valid code, except where a test suite declares several
  units of the same name in one library.
- Each resolved reference records its access (read, write, both), which feeds the
  unused / unread / undriven / sensitivity checks.
- Hardware checks (`crates/vfl-sema/src/rtl.rs`) run with type checking, per unit:
  - `sema/width-mismatch`: an assignment, initial value or port / generic association
    where a vector of scalars gets a value with a different static number of elements.
    Declarations record a width when their one-dimensional constraint has static bounds
    (literals and constants with a static integer value, through subtypes); expression
    widths come from string and bit string literals, objects, slices, `&`, logical
    operators, type conversions, and `numeric_std` `+` / `-` (the wider operand).
    Aggregates, function calls and anything generic-dependent are unknown and never
    reported.
  - `sema/inferred-latch`: a combinational process (a sensitivity list or a single
    trailing `wait on`, no clock edge, no delayed `after` assignments) that assigns a
    signal on some paths but not all. Paths follow `if` (an `else` is needed), `case`
    (every alternative), conditional assignments without a final `else`, and loops
    (assumed to run). Signals assigned in part (`s(3) <=`) are left alone.
  - `sema/clock-domain-crossing` (info): in an architecture, a signal registered by a
    process clocked on one clock and read by a process clocked on another. A plain copy
    into a register (`sync_1 <= s;`, the first stage of a synchronizer), memories (arrays
    of arrays, as in dual-clock RAMs) and processes with `wait` statements are not
    reported.
  - `sema/open-input` and `sema/inout-port`: an input port without a default associated
    with `open`; an `inout` port only read or only driven by its architectures.
  - Resets (`sema/missing-reset`, `sema/mixed-reset`, `sema/reset-polarity`): a clocked
    process's reset is the first condition of an `if` whose last branch is the clock edge
    (asynchronous), or the first condition of an `if` opening the clocked branch on a
    signal named like a reset (synchronous). Registers assigned in the clocked part but
    not in the reset part are reported (info), as are, within an architecture, reset
    signals used both synchronously and asynchronously or tested with both polarities,
    and active-low names (`rst_n`) tested active high.
  - State machines (`sema/fsm-unreachable-state`, `sema/fsm-stuck-state`): a `case` on a
    signal or variable of a project enumeration type whose alternatives assign literals
    to it or to a same-type next-state object. Any other kind of assignment to these
    objects leaves the machine alone. States no transition reaches, and reachable states
    no transition leaves, are reported.
- Unknown things degrade to "unknown" instead of cascading errors: a library that is not in
  the project (vendor primitives such as `unisim`) makes its names unchecked and is reported
  once as info, and names that may come from it are not reported.
- `std` and `ieee` are bundled as VHDL sources (`std` written from the LRM, `ieee` from the
  Apache-2.0 IEEE packages, plus the declarations of the Synopsys `std_logic_arith` family)
  and analyzed like user code, once per process.
- Incrementality is deliberately simple instead of `salsa`. Parsing is per file and cached
  (an edit re-parses one file), and library inference is a cheap pass over per-file
  summaries. `Analysis::update` then reuses the previous analysis unit by unit, in the same
  dependency order as a full run. A unit is analyzed again when its file's tree changed,
  or when a unit it depends on (by name, or by a declaration its results refer to) is
  *affected*. A re-analyzed unit is affected when its interface hash changed: its
  declarations without their positions, its region, imports and libraries. A unit that
  depends on an affected unit is affected too, whatever its own hash, since its
  dependents may read through it. Unchanged files keep their line index and unit list,
  type checking runs only for re-analyzed units, and the cross-unit checks run again in
  parallel. When files or units are added, removed or renamed, everything is analyzed
  again. The linter also reuses a file's findings while its tree, config, semantic
  diagnostics and declarations are unchanged. `scripts/corpus.sh` checks on the corpus
  that an update after edits gives exactly what a full analysis gives.
- Only an analysis built by `Analysis::update` (the language server's) keeps the hashes and
  dependencies reuse needs; a one-off `Analysis::new` (the CLI's) skips that bookkeeping.
- Edit to fresh diagnostics in the language server (`crates/vfl-lsp/examples/edit_latency.rs`):
  a median of 6 ms (neorv32) to 45 ms (UVVM), and up to a full re-analysis (about 0.8 s on
  UVVM) for a change to a package that every unit uses.

### Formatter

`vfl-fmt`, run with `vfl fmt`. Only files that parse without errors are formatted.

1. **Tokens**: the tree is flattened into tokens and comments (PSL directives stay verbatim).
   Keyword and identifier case are applied, and the optional words after `end` (`end entity
   e`) are added or dropped. Spacing comes from the pair of neighbouring tokens: spaces
   around binary operators, `:`, `=>`, `<=` and `:=`; none inside parentheses, before `,` and
   `;`, after a sign, around `.` and `'`; `f(x)` and `v(7 downto 0)` stay glued, `process
   (clk)` does not (configurable).
2. **Line breaks**: the source's line breaks are kept (runs of blank lines capped), and every
   statement, declaration, clause, `begin`, `end`, `else` and `elsif` starts a line. The
   formatter adds line breaks but never removes one, which makes idempotency structural and
   keeps diffs small. Lists are laid out one element per line, with `(` ending the opening line
   and `)` on its own line: port and generic clauses always by default, port and generic maps
   when already split or too long, and any other list of two or more elements (parameter
   lists, calls, aggregates) when its line is wider than `max_width`, outermost list first. A
   single index or argument is never split; an expression with no list to split stays long.
3. **Indentation** follows the constructs. A line continuing a statement gets
   `continuation_indent`; a line inside parentheses gets one `indent` more than the line that
   opened them; continuation lines of conditional and selected assignments start under the
   first value. A comment line is indented like the code after it, unless it was indented
   deeper than a following `end`, `else`, `when` or `)`, in which case it stays with the
   block above.
4. **Alignment**: each line can carry anchors, tokens that line up with the same token on
   neighbouring lines of the same construct. Consecutive lines with the same anchor form a
   block, and each anchor of a block is padded to the block's rightmost column, leftmost
   anchors first. Targets, each switchable: `:`, the subtype after the mode, and `:=` in port
   and generic lists; `:`, `:=` and `is` in declarations; `<=` and `:=` of consecutive
   assignments; `=>` in maps and aggregates; `=>` of one-line case alternatives and the
   assignments after them; `when` in conditional and selected assignments; trailing
   comments. Blocks end at blank lines, at comment lines too, or at neither
   (`block_boundary`).

The plan was a Wadler/Prettier document IR printed against `max_width`. Keeping the author's
line breaks and only adding breaks turned out simpler and closer to how VHDL is written by hand
(aligned blocks, hand-split conditions); long lines are handled by splitting lists. Re-wrapping
long expressions is left for later.

Guarantees, enforced in tests, in the corpus job and at runtime:

- **Idempotent**: `fmt(fmt(x)) == fmt(x)`, checked on every test file and on every corpus file
  with two very different configs.
- **Semantics-preserving**: after formatting, the output is parsed again; it must have no
  errors, the same non-trivia tokens (ignoring case where VHDL does, and the optional words
  after `end`) and the same comments. Otherwise `vfl fmt` reports a bug and leaves the file
  untouched.
- Escape hatches: `-- vfl: off` / `-- vfl: on` regions and `-- vfl: skip` for the next
  statement or declaration are kept exactly as written.
- Line endings (LF or CRLF), a UTF-8 byte-order mark and Latin-1 encoding are preserved.
- Not yet: range formatting (format selection, format on type), which comes with the LSP.

### Linter

- Rules have an id `category/name` (`sema/unused-signal`, `style/clock-edge`,
  `naming/signal`), a level in each preset, and optionally a fix marked safe or unsafe.
  `vfl rules` lists them.
- Categories: `syntax/*` (parse errors), `sema/*` (cross-file semantic checks: unresolved
  names and units, duplicates, unassociated ports, unused signals / variables / constants /
  ports / generics / use clauses, unread and undriven signals, incomplete or unneeded
  sensitivity lists, width mismatches, inferred latches, clock domain crossings),
  `style/*` (one file: buffer ports, Synopsys arithmetic packages, line
  length, trailing whitespace, tabs, process labels, positional association, `'event` clock
  edges, file names) and `naming/*` (one rule per kind of declaration, off until
  configured, with `prefix`, `suffix`, `case` and `pattern` options).
- Presets: `none` (only syntax errors), `recommended` (default: likely bugs), `strict`
  (everything but naming). Per-rule overrides set a level, options, or both; options alone
  turn a rule on at `warn`.
- Suppression: `-- vfl-ignore [rules]` at the end of a line silences that line, on a line
  of its own the next line of code; `-- vfl-ignore-file [rules]` the whole file. Without
  rules, everything; a category (`naming`) silences all its rules.
- `vfl lint` analyzes the whole project around the given paths but reports only the given
  files. `--fix` applies safe fixes (`--unsafe-fixes` also the others) and re-lints.
  Output formats: human (with source excerpts), concise, JSON, SARIF, GitHub annotations.
  The exit code is non-zero on errors, or on warnings with `--deny-warnings`.

### Language server

`vfl lsp` (stdio) analyzes the project around the first workspace folder. Open buffers
overlay the files on disk; edits are applied incrementally and analysis runs once a burst of
changes is over. It provides diagnostics with quick fixes and a `source.fixAll.vfl` action,
whole-document formatting (returned as a single minimal edit), go to definition (bundled
`std` / `ieee` sources are written to a cache directory so editors can open them),
references, document highlights (read / write), hover with the declaration's source and
its type (an object's subtype and what the type is, a subtype's base, a function's return
type, record elements reached through `a.b` or `a(i).b`; on a call, only the overload
type checking chose), signature help for subprogram calls, protected type methods and
`port map` / `generic map` (the active parameter follows commas and named formals),
document and workspace symbols, rename (including end labels and selected names) and
completion (scope, `use`d packages, selected-name members, record elements and protected
type methods after `value.`, libraries, keywords).

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
# continuation_indent = 2   # defaults to `indent`
use_tabs        = false
keyword_case    = "lower"    # "lower" | "upper" | "preserve"
identifier_case = "preserve"
end_label       = "preserve" # "add" | "remove" | "preserve": `end entity e;`
end_keyword     = "preserve" # "add" | "remove" | "preserve": `end entity;`
space_before_label_colon = true   # `lbl : process`
space_after_keyword      = true   # `process (clk)`, `port map (`
max_blank_lines  = 2
interface_list   = "expand"   # port/generic clauses: "expand" | "auto" | "preserve"
association_list = "auto"     # port/generic maps: "expand" | "auto" | "preserve"
close_paren      = "own-line" # "own-line" | "attached"
case_alternative = "preserve" # keep `when x => stmt;` on one line, or "break"
line_ending      = "auto"     # "auto" | "lf" | "crlf"

[format.align]
port_colon       = true
port_mode        = true       # the subtype after `in`/`out`/...
generic_colon    = true
interface_default = true      # `:=` in port and generic lists
assignment       = true       # aligns <= and := in consecutive statements
association      = true       # aligns => in port/generic maps and aggregates
declaration      = true       # aligns ":", ":=" and "is" in declarative parts
case_choice      = true       # => of one-line case alternatives
when_clause      = true       # `when` in conditional and selected assignments
conditional_continuation = true  # continuation lines start under the first value
trailing_comment = true
block_boundary   = "blank-line"   # "blank-line" | "comment" | "never"

[lint]
preset = "recommended"       # "none" | "recommended" | "strict"

[lint.rules]
"naming/signal" = { prefix = "s_", case = "snake" }
"style/no-buffer-ports" = "error"
"sema/unused-signal" = "off"
```

Lookup and layering:

- For a file, the nearest `vfl.toml` walking up from its directory applies.
- A nested config inherits from the ones above it and overrides only the keys it sets, so a
  subtree can relax rules without copying everything. `root = true` stops the upward search.
  `extends = "path.toml"` layers a shared team config underneath the file.
- `vfl config show <file>` prints the effective config and the files it came from.
- Every key is described in [configuration.md](configuration.md), every rule in
  [lints.md](lints.md).

## Project autodiscovery

The aim: open any VHDL file and get correct cross-file analysis, without a library list.

**1. Find the root.** Walk up from the file: the outermost directory with a `vfl.toml`
wins, so nested override configs do not split the project, stopping at a config with
`root = true` and never going above the nearest VCS root (`.git`, `.hg`, `.jj`). Without a
config, the VCS root; failing that, the file's own directory. In the LSP, workspace folders
are also roots.

**2. Collect sources.** Every `*.vhd` / `*.vhdl` (configurable) under the root, honouring
`.gitignore` and `exclude` (via the `ignore` crate, the same walker as ripgrep).

**3. Import existing tool metadata when present.** If the tree already declares libraries,
use them. Implemented, all read without running the tool:

- rust_hdl `vhdl_ls.toml` in the root (with `$VAR` expansion).
- VUnit `run.py`, best effort: `add_library`/`library` and `add_source_files` calls,
  with string variables, `Path(__file__).parent` joins and f-strings. With VUnit's
  builtins added, files added to `osvvm` are ignored, so that a few local files do not
  pass for the whole library.
- FuseSoC `.core`: `filesets` entries of a `vhdlSource*` type, in `logical_name` or `work`.
- Vivado `.xpr` (`Library` attribute, default `xil_defaultlib`) and Tcl `read_vhdl
  -library` scripts.
- Quartus `.qsf`/`.qip`: `set_global_assignment -name VHDL_FILE ... -library`.

  In both Tcl formats, `set` variables are followed (`set path [file dirname [info
  script]]`, then `$path/rtl/x.vhd` or `${path}`), along with `[file join|dirname|
  normalize|tail]`, `[info script]`, `[pwd]`, `[glob [-directory d] pattern]` and
  Quartus's `$::quartus(qip_path)`. A word using anything else is skipped.

These files are found by the same walk as sources (so `.gitignore` applies). Their globs
use the tools' semantics: `*` stays within a directory. `vfl.toml` wins over
`vhdl_ls.toml`, which wins over tool files; a file a tool lists in no library is still
inferred. HDLmake `Manifest.py` is not read yet. This makes migration a no-op.

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

Measured by `scripts/bench_rust_hdl.py` against the latest `vhdl_ls` release on a corpus
(OSVVM, UVVM, VUnit, neorv32, and the GHDL test suite for robustness). Current numbers are
in [benchmarks.md](benchmarks.md): 1.5 to 2.3 times faster, 2.8 to 4.1 times less memory,
and far fewer false errors, with zero configuration.

| Dimension | Target | Status |
|-----------|--------|--------|
| Cold full-project analysis time | faster | 1.5-2.3x faster |
| Peak memory | lower | 2.8-4.1x lower |
| LSP keystroke-to-diagnostics latency | under 50 ms on the largest corpus project | 6-45 ms median per edit; a change to a package everything uses re-analyzes its dependents |
| False errors on valid corpus code | fewer (tracked per release) | fewer on every project |
| Setup | zero config vs. required `vhdl_ls.toml` | done; `vfl project --vhdl-ls` exports one |
| Features | formatter, autofixes, lossless refactorings, VHDL-2019, type checking | done |

## Repository and CI

- Cargo workspace, edition 2024, MSRV pinned in `rust-toolchain.toml` and `Cargo.toml`.
- License: MIT OR Apache-2.0 (default choice, easy to change before the first release).
- CI (GitHub Actions): `cargo fmt --check`, `cargo clippy -D warnings`, tests on Linux,
  macOS and Windows; later corpus regression, fuzzing and benchmarks.
- Snapshot tests (`insta`) for parser trees, formatter output and diagnostics.

## Roadmap

1. Design and workspace (this document).
2. Parser: lexer, CST, VHDL-2008 grammar, then 93 and 2019 gating; corpus round-trip tests.
3. Formatter with full alignment; idempotency and token-preservation checks (done).
4. Project inference, semantic model, linter, CLI and LSP; benchmarks against rust_hdl
   (done).
5. Overload resolution and type checking (done).
6. Incremental analysis for the LSP: an edit re-analyzes only the units it affects,
   reusing the results of the others (done).
7. Type-aware LSP features: hover showing an object's type and the overload a call
   resolves to, signature help for subprogram calls and instantiations, completion of
   record elements and protected type methods (done).
8. More rules: width mismatches in assignments and associations, inferred latches
   (combinational processes that do not assign a signal on every path), clock domain
   hints (a signal sampled by two clocks without synchronization) (done).
9. More import formats for library lists: VUnit `run.py`, FuseSoC `.core`, Vivado and
   Quartus project files, alongside today's `vhdl_ls.toml` (done).

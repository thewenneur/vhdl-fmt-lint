# vhdl-fmt-lint: design

Status: implemented. Every step of the [roadmap](#roadmap) is done; the sections below
describe how the parser, the project discovery, the analysis, the formatter, the linter and
the language server work.

## Goals

- One pure-Rust toolchain for VHDL: a **library**, a **CLI** (`vfl`) and a **language server**.
- A **formatter** where every rule is configurable, including column alignment of
  keywords, assignments, ports, generics, declarations and trailing comments.
- A **linter** with syntactic and cross-file semantic rules, each configurable, many with autofixes.
- **Zero configuration** for the common case: open a file, the tool finds the project,
  the sources and their libraries. No hand-written library lists.
- VHDL-2008 first, then VHDL-93, then VHDL-2019, all behind one parser.
- Measurably better than rust_hdl (`vhdl_ls`) on speed, memory, correctness and features.

Non-goals for now: simulation, elaboration of generate loops with arbitrary generics.
Verilog and SystemVerilog support is being added in phases (see below).

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
| `vfl-gen`    | Code generated from an entity: instantiations, component declarations, testbench skeletons. |
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
  - `sema/missing-branch-assignment`: the same paths, reported per branch: a branch of
    an `if` or `case` that leaves out a signal another branch assigns and nothing before
    the statement assigns, or an `if` without `else` that assigns one. Clocked processes
    (their clocked branch, resets left out) are checked with the `clocked` option. Unlike
    the other checks it runs on demand (`Analysis::missing_branch_assignments`), only for
    files the rule is on for, so it costs nothing when off.
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
  - Case choices (`sema/incomplete-case`, `sema/duplicate-choice`): the choices of a
    `case` or selected assignment on an object of an enumeration, an integer subtype
    with static bounds, or a character array of static width, as positions, integer
    spans or array values. Missing spans and overlaps are found by sorting them; a
    choice that is not static leaves the statement alone. The fix of a missing choice
    is built with the finding, which only exists for the few statements reported.
  - `sema/multiple-drivers` and `sema/combinational-loop` run on demand
    (`Analysis::driver_checks`), only for files one of them is on for. They walk the
    statements of each architecture (and its blocks and `for` generates) and read what
    each writes and reads from the references already resolved, with the static part
    of each name (`v(3)`, `v(7 downto 4)`, `r.f`). Drivers are grouped by signal; a
    signal with several drivers whose parts overlap is reported. For loops, each
    assignment of a combinational statement is a node, with an edge to the
    assignments that read what it writes (values, indexes, and the conditions they
    are under); the strongly connected components (Tarjan's algorithm) are the loops.
    Both are linear in the references of the file.
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
   other lines. By default (`scope = "block"`) every anchor of a kind lines up across the
   whole enclosing block (process, architecture, generate body, subprogram, package, entity,
   record, interface list), even with other lines, blank lines or comments in between; with
   `scope = "contiguous"` only consecutive lines of the same construct do. Each anchor of a
   run is padded to the run's rightmost column, leftmost anchors first. Targets, each switchable: `:`, the subtype after the mode, and `:=` in port
   and generic lists; `:`, `:=` and `is` in declarations; `<=` and `:=` of consecutive
   assignments; `=>` in maps and aggregates; `=>` of one-line case alternatives and the
   assignments after them; `when` in conditional and selected assignments; the `;` ending
   those lines (interface elements, declarations and statements each among themselves);
   trailing comments. With `scope = "contiguous"`, runs end at blank lines, at comment lines
   too, or at neither (`block_boundary`).

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
- Range formatting (`vfl fmt --lines`, LSP range and on-type formatting): the whole file is
  formatted and only the output lines of the selected source lines are copied back, widened
  to the statements or declarations they touch (not past the construct around them), to
  every line of an alignment run they are part of, and to lines that start and end on the
  same tokens in the source and the output. Protected regions are never touched.
  Only the part around the selection is laid out: whole lines between blank lines, cut
  between the items of a construct, and not after a comment line, whose indentation follows
  the code below it. With `scope = "block"`, when a line to change holds an aligned token,
  the part grows to the whole block of that token (the process, the interface list, ...)
  and is laid out again (format on type: only blocks up to 16 KB, else the line aligns with
  the lines near it); the lines of an edit that stay as they are are left out of it.
  Files with `-- vfl:` directives, `max_blank_lines = 0`, or `block_boundary = "never"` with
  `scope = "contiguous"`, are laid out whole. The corpus job checks that both give the same
  edits.

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
whole-document formatting (returned as a single minimal edit), range formatting, format on
type (`;` formats its line, a new line formats the line above it), go to definition (bundled
`std` / `ieee` sources are written to a cache directory so editors can open them),
references, document highlights (read / write), hover with the declaration's source and
its type (an object's subtype and what the type is, a subtype's base, a function's return
type, record elements reached through `a.b` or `a(i).b`; on a call, only the overload
type checking chose), signature help for subprogram calls, protected type methods and
`port map` / `generic map` (the active parameter follows commas and named formals),
semantic tokens (whole file, range and delta: names typed by what they resolve to, with
modifiers for port and parameter modes, declarations, writes and `std` / `ieee` names),
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
style           = "default"  # "default" | "leading-comma" | "compact" | "classic"
max_width       = 100
indent          = 4
# continuation_indent = 4   # defaults to `indent`
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
interface_separator   = "trailing" # `;` of port/generic clauses: "trailing" | "leading"
association_separator = "trailing" # `,` of maps, calls, aggregates: "trailing" | "leading"
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
semicolon        = true       # the `;` ending aligned lines
scope            = "block"    # "block": the whole process, architecture, ...; "contiguous"
block_boundary   = "blank-line"   # with "contiguous": "blank-line" | "comment" | "never"

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
`root = true` and never going above the nearest VCS root (`.git`, `.hg`, `.jj`; a git
submodule, whose `.git` file points into `.git/modules/`, belongs to its superproject). Without a
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
use the tools' semantics: `*` stays within a directory. `vfl.toml` wins over tool files;
a file a tool lists in no library is still inferred. `vhdl_ls.toml` ranks below inference
by default: it places only the files inference leaves in no library, and its libraries then
propagate along `work` references. If that leaves library conflicts (references to project
units missing from the library meant, units declared twice in a library), the ranking with
`vhdl_ls.toml` above inference is tried too and kept when it leaves fewer (`[project]` `vhdl-ls = "prefer"` ranks it just below
`vfl.toml` instead, `"off"` ignores it). HDLmake `Manifest.py` is not read yet. This makes migration a no-op.

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
10. Documentation and diagrams: `vfl doc` writes HTML, Markdown or AsciiDoc pages from the
    project and its doc comments, with entity symbols, the instantiation tree, block diagrams
    of the top level and of each architecture, port maps and state diagrams as SVG;
    `vfl schema` draws one diagram as SVG, Mermaid or DOT. It reads the syntax trees only
    and is not part of the language server (done).
11. Code generation: entity and component instantiations, component declarations and
    testbench skeletons from an entity, in `vfl new` and as editor commands and code
    actions (done).
12. Semantic highlighting: names colored by what the analysis resolved them to, in the
    language server and the Notepad++ plugin (done).
13. Design checks: missing, mixed and wrongly polarized resets, open inputs, `inout`
    ports used one way, unreachable and stuck state machine states (done).
14. Refactoring: rename across a package and its body, a subprogram and its body, an
    entity and its components, and record elements, with clash checks; quick fixes for
    naming conventions, missing `use` clauses and suppressions (done).
15. Range formatting and format on type (done).
16. Notepad++ parity: the plugin analyzes on a background thread and offers every
    feature of the language server (done).
17. Verilog and synthesizable SystemVerilog, in four phases:
    1. `vfl-verilog-syntax`: a lossless lexer and parser in the same style as
       `vfl-syntax`, with its own rowan language and kinds, so the VHDL crates do not
       change. Directives are trivia; `` `ifdef `` branches not compiled are one
       `INACTIVE` trivia token; macros are not expanded but parsed where they stand
       (an expression, an item or a statement). Classes, covergroups, properties,
       sequences and constraints are opaque. `vfl check` reads `.v`/`.sv` files
       (done).
    2. Mixed-language projects: each module's summary (name, parameters, ports,
       instances, comments) is read off its tree and given to the VHDL analysis as a
       unit of a foreign file, whose VHDL tree is empty. The module is an entity, its
       parameters generics and its ports ports, with no VHDL type, so the type checker
       leaves their associations alone; literal vector widths are still checked.
       Library inference treats modules as entities and their instances as default
       bindings. A Verilog edit re-analyzes the module, and the VHDL units that use it
       only when its interface changed. Hover, go to definition and `vfl doc` (module
       pages, hierarchy, block diagrams) cross languages (done).
    3. Formatter and editor features. The formatter's engine (elements, layout,
       alignment, checks) is generic over a `Lang` trait that each language implements
       with its tables: which nodes are items and containers, which keywords start or
       end lines, how tokens are spaced, what lines up. It is monomorphized, so VHDL
       formatting runs the same code as before. The language server answers syntax
       errors, symbols, folding and semantic tokens of Verilog files from their trees,
       without the VHDL analysis (done).
    4. Lint rules and semantic checks for the synthesizable subset.

    Files are dispatched by extension when the project is collected; a project without
    Verilog files never runs Verilog code.

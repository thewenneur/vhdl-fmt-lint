# Changelog

## [Unreleased]

### Added

- **Waivers**: `[[lint.waiver]]` entries in `vfl.toml` silence findings by file glob, rule
  or category, line or line range, and message pattern, without touching the sources, on
  the command line and in editors.
- **Baselines**: `vfl lint --write-baseline FILE` records the current findings;
  `--baseline FILE` (or `lint.baseline` in `vfl.toml`) then reports only new ones, and
  the summary counts the baselined ones. Findings are matched by file, rule, message and
  line text, not line number, so edits elsewhere in a file keep them baselined.
- **`vfl fmt --diff`** prints a unified diff of what formatting would change, and fails
  like `--check` if there is any.
- **JUnit XML and GitLab Code Quality output**: `vfl lint --output-format junit` and
  `--output-format gitlab`.
- **`vfl lint --changed-since REF`** reports only the files changed since a git revision
  (the whole project is still analyzed), and with `--changed-lines` only the findings on
  changed lines.
- **`vfl lint --fix-only RULES`** applies the fixes of some rules or categories only.
- **`vfl rules --explain RULE`** prints a rule's levels and full description.
- **32-bit and ARM64 Notepad++**: releases have `vfl_npp-vX.Y.Z-x86.zip` and `-arm64.zip`
  next to the 64-bit plugin, and a `vfl` binary for Windows on ARM.

### Changed

- **Zed downloads the server of its own version**, not the latest release, and checks the
  archive against the release's `SHA256SUMS` before keeping the binary; Windows on ARM
  gets a binary too. The example tasks run from the worktree root (`$ZED_WORKTREE_ROOT`)
  instead of the current file's directory, and the README says they need `vfl` on the
  `PATH`.
- **Notepad++ format on save skips files larger than 512 KB**, with a note in the status
  bar, since it formats before the file is written and holds up the save;
  `format_on_save_max_kb` in `vfl_npp.ini` sets the limit.

## [0.7.0] - 2026-10-15

Library inference ranks above `vhdl_ls.toml`, every editor action can take a keyboard
shortcut, and projects without a `vfl.toml` pick their lint preset and format style in the
editor.

### Added

- **Keyboard shortcuts for every editor action**: every VS Code command (in **Preferences:
  Open Keyboard Shortcuts**, search `vfl`) and every Notepad++ menu command, settings
  included (**Settings > Shortcut Mapper > Plugin commands**). New Notepad++ commands are
  only added at the end of the menu, so shortcuts survive upgrades. Zed gets example
  `tasks.json` and `keymap.json` for the actions it has no command for.

- **Lint preset and format style from the editor** for projects without a `vfl.toml`:
  `vfl.lint.preset` and `vfl.format.style` in VS Code, **Lint preset without vfl.toml...**
  and **Format style without vfl.toml...** in Notepad++, `lsp.vfl.initialization_options`
  in Zed, and any `vfl.toml` key under `config` in the initialization options of other
  LSP clients. A `vfl.toml` replaces them.
- **Generate project config (vfl.toml)** in VS Code and Notepad++, and as a code action in
  other editors, writes the settings in effect to the project's `vfl.toml`;
  keys at their default value are written as comments.
- **Configuration guide by example** in `examples/config`: one folder per formatter or lint
  topic, each with a `vfl.toml`, a file before and after `vfl fmt` or `vfl lint --fix`, and
  a README. A test keeps the `after` files in sync with vfl.
- **`sema/missing-branch-assignment`**: every signal a process assigns in a branch of an
  `if` or `case` must be assigned in all its branches (and the `if` must have an `else`),
  unless it has a default before the statement. Reported at the branch that leaves it out.
  Combinational processes by default; `{ clocked = true }` checks clocked processes too,
  leaving the reset branch out. Warn in `recommended`, error in `strict`; a latch it explains is reported by it only, not also as `sema/inferred-latch`. Fixes: `q <= q;` in
  clocked processes (safe), a default before the statement in combinational ones (unsafe).
- **`style/case-others`** (strict): a `case` statement without `when others`; the fix adds
  `when others => null;`.

### Changed

- **Library inference ranks above `vhdl_ls.toml`**: a rust_hdl `vhdl_ls.toml` now only
  places the files inference leaves in no library, so a stale one no longer overrides what
  the code says. When inference leaves library conflicts (references to units missing from
  the library meant, units declared twice in a library) and ranking `vhdl_ls.toml` first
  leaves fewer, vfl keeps that instead and `vfl project` says so. The new `[project]`
  `vhdl-ls` key sets this: `"fallback"` (default), `"prefer"` (always the old ranking) or
  `"off"`.

### Fixed

- **`-- vfl: skip` keeps the comment at the end of the skipped statement** as written; the
  formatter used to change the spaces before it.

## [0.6.0] - 2026-10-15

State machine diagrams follow their branches instead of a circle and stay readable at 50+
states; every diagram of the HTML documentation zooms and pans.

### Added

- **Zoom and pan on every diagram** of the HTML documentation and of the VS Code diagrams
  view: buttons over the diagram (−, Fit, +, 1:1), Ctrl or Cmd + wheel at the pointer, `+`
  `-` `0` while hovering, and drag to pan.
- **The Zed extension is a release download**: `vfl-zed-vX.Y.Z.zip`, its sources, to install
  with **zed: install dev extension** without cloning the repository.

### Changed

- **State machine diagrams are laid out in layers** from the initial state down: a state with
  several successors opens branches that run side by side, loops go back up and merge into
  one line, edges route between states, and labels never overlap. A 58-state machine draws
  at 1152×757 instead of 3195×3207.
- **Notepad++ colors input and output ports differently**: inputs blue, outputs red,
  inout, buffer and linkage stay pink.
- The Zed setup tells users to remove the VHDL and Verilog extensions, which would otherwise
  start a second language server.

## [0.5.0] - 2026-10-15

The formatter aligns every token across its whole block by default, the `;` ending the lines
too, with 4-column indentation; editors beyond VS Code and Notepad++ get a Zed extension and
ready-made setups.

### Added

- **Zed extension** (`editors/zed`): VHDL, Verilog and SystemVerilog highlighting, outline,
  brackets and indentation from Tree-sitter grammars, and the language server, from the
  `PATH` or downloaded from the latest release. Installed as a dev extension for now.
- **Setup for more editors** ([docs/editors.md](docs/editors.md)), with ready-made files in
  `editors/`: Neovim, Vim (vim-lsp, yegappan/lsp, coc.nvim), Emacs (Eglot and lsp-mode),
  Helix, Sublime Text, Kate and JetBrains IDEs (LSP4IJ). The Neovim, Emacs, Vim and Helix
  setups are tested against those editors in CI.

### Changed

- **The formatter aligns every token across its whole block by default**: every `<=` of a
  process at any `if` depth, every `=>` of the port maps of an architecture and of the
  alternatives of a `case`, every `:` of a declarative part, line up even with other lines,
  blank lines or comments in between. A block is a process, an architecture, a block or
  generate body, a subprogram, an entity, a package, a component, a record or protected
  type, or a generic, port or parameter list (in Verilog: a module, an `always` or
  `initial` procedure, a function or task, a generate block, a struct, a port or parameter
  list). The new `[format.align]` key `scope` sets this: `"block"` (the default) or
  `"contiguous"`, the earlier behavior, where only consecutive lines align, up to
  `block_boundary`.
- **The `;` ending aligned lines is aligned too by default**, with the new
  `[format.align]` key `semicolon`. Interface elements, declarations and statements each
  line up their `;` among themselves.
- **`indent` is 4 by default** (was 2). The `compact` style keeps 2, and the `compact` and
  `classic` styles keep aligning consecutive lines only, without the `;`.
- Range and on-type formatting follow the wider alignment: when the edited line has an
  aligned token, its whole block is laid out, and only the lines that change are in the
  edits. To stay fast while typing, format on type does this only for blocks up to 16 KB;
  in a larger one the line aligns with the lines near it, and formatting the file or a
  selection aligns the rest.

To format as 0.4 did:

```toml
[format]
indent = 2

[format.align]
scope = "contiguous"
semicolon = false
```

### Fixed

- A broken-up Verilog call with an empty argument (`f(a,, b)`) no longer puts the empty
  argument on a line of its own, which the next formatting undid.

## [0.4.0] - 2026-10-15

Full Verilog and SystemVerilog support becomes opt-in (`[verilog] mode`), the formatter gains
leading separators and named styles, and editors can fill in the ports an instantiation
leaves out.

### Added

- **Quick fix for `sema/unassociated-port`**: editors offer to associate the ports or
  generics an instantiation leaves out, adding a port map or generic map when there is
  none. A port gets the signal or constant of the same name when one is declared
  (`b => b`), else `open` (or, with the rule's option `undeclared = "name"`, its own
  name for a required port, to be filled in); a second fix associates every port left
  out, not only the required ones. The new associations follow the
  map's layout: on its line, or one per line with the commas before or after and the
  arrows aligned, and a formatted instantiation stays formatted.
- **Leading separators**: `interface_separator = "leading"` and
  `association_separator = "leading"` in `[format]` put the `;` of port and generic
  clauses and the `,` of port maps, generic maps, calls and aggregates at the start of the
  next line (`, data_in => cnt`), hanging left of the elements so that they and their
  alignment still line up. Verilog port lists and connections follow the same keys.
- **Format styles**: `style = "leading-comma"`, `"compact"` or `"classic"` in `[format]`
  picks a set of defaults; keys set explicitly still win.

### Changed

- **Verilog and SystemVerilog files are only read for VHDL name resolution by default**,
  set by the new `[verilog]` `mode`: `"names"` (the default) reads them only so that VHDL
  resolves the modules it instantiates, with nothing reported, formatted or colored in
  the Verilog files; `"off"` ignores them; `"full"` turns on everything 0.3 did, and says once that this
  support is in progress (on standard error, and in a message in VS Code and Notepad++).
  To keep 0.3's behavior, add `mode = "full"` under `[verilog]` in `vfl.toml`.
- The VS Code extension no longer makes itself the default formatter of Verilog and
  SystemVerilog.

### Fixed

- A list written with leading commas no longer formats to a lone `,` on its own line:
  with the default trailing separators, the commas move back to the ends of the lines.
- The formatter no longer glues together tokens that were written apart and would read
  as one token once joined (`s foo` in a malformed external path, `- -a`).
- A file with lone CR line ends no longer fails to format when a Verilog `` `define `` is
  continued by a `\` onto an empty line: lone CRs are read as line ends throughout.

## [0.3.1] - 2026-10-15

Faster and leaner analysis, with no change in what it reports.

### Changed

- **Faster re-analysis after an edit**: 25 to 30% fewer instructions per edit (UVVM and
  neorv32, measured with Callgrind), and a faster project open. Syntax tree walks no longer
  allocate a node for every step, and internal maps hash with FxHash instead of SipHash.
  Lint results are byte-identical to 0.3.0.
- **Less memory in the language server**: 10 to 12% less resident memory once a project
  is open, and a long session no longer grows past what it needs: on UVVM, 40 edits of a
  package most of the project uses now peak at 168 MiB instead of 235 MiB. A
  re-analysis frees the previous results of the units it analyzes again before building
  their new ones, a name's targets are stored inline when there is one, and the tables
  kept per unit no longer keep their growth slack. Edits, hover and completion are as
  fast or faster (see `docs/benchmarks.md`, and `scripts/bench_memory.py` to measure).

## [0.3.0] - 2026-10-14

### Added

- **Verilog and SystemVerilog parsing** (`vfl-verilog-syntax`): a lossless, error-tolerant
  parser for Verilog (IEEE 1364-1995/2001/2005) and synthesizable SystemVerilog (IEEE
  1800-2005 to 2023), with `` `begin_keywords ``. Directives stay in the tree; the
  `` `ifdef `` branch compiled is chosen from the configured `defines` and the macros
  defined in the file and its includes. Classes, covergroups, properties and constraints
  are kept opaque so testbenches still parse. `vfl check` reads `.v`, `.vh`, `.sv` and
  `.svh` files, configured by the new `[verilog]` section. VHDL-only projects run no
  Verilog code.
- **Mixed VHDL and Verilog projects**: the project's Verilog and SystemVerilog modules are
  entities for the VHDL analysis. An instance like `entity work.fifo` has its generic and
  port maps checked against the module's parameters and ports (unknown formals, inputs
  left unassociated, literal vector widths), and a module goes in the
  libraries of the units that instantiate it, like an entity. In the editor, hover shows
  a module's or a port's Verilog declaration, go to definition opens the Verilog file,
  and an edit to a Verilog file updates the VHDL diagnostics; VHDL units are analyzed
  again only when what they see of a module (its ports and parameters) changed. `vfl
  doc` documents each module (parameters, ports, comments, instances) and draws it in
  the hierarchy, block and symbol diagrams. `vfl project` lists the Verilog files with
  their libraries.
- **Verilog and SystemVerilog formatting**: `vfl fmt` formats `.v` and `.sv` files with the
  `[format]` settings that apply to them (indentation, list layouts, line width) and the
  same alignment as VHDL: port and parameter types, names and defaults, declarations,
  assignment operators, named connections and case items. `begin` stays where it is
  written, directives keep their indentation, code in `` `ifdef `` branches that are not
  compiled is kept as written, and `// vfl: off` regions are left alone. Like VHDL, the
  output must parse to the same tokens and comments, and formatting it again changes
  nothing. `vfl fmt - --stdin-filename top.sv` formats standard input as the named file.
- **Verilog in the editor**: the language server reports the syntax errors of Verilog and
  SystemVerilog files, formats them (document, range, on type), lists their modules,
  parameters, ports, declarations, functions, tasks and instances as document symbols,
  folds their blocks, lists and comments, and colors their names by declaration. The VS
  Code extension registers the `verilog` and `systemverilog` languages with a grammar;
  the Notepad++ plugin formats, checks, outlines and colors Verilog buffers.
- **Verilog and SystemVerilog linting**: `vfl lint`, the language server and the
  Notepad++ plugin lint Verilog files, with names resolved across the project's modules,
  packages and included files. New `verilog/*` rules: blocking assignments in clocked
  blocks and non-blocking ones in combinational blocks (with fixes), multiple drivers,
  implicit nets, unconnected inputs, missing `` `default_nettype none `` and `case`
  without `default`. The shared rules apply too: undeclared and duplicate names, unknown
  ports and parameters of instances, unused, unread and undriven signals, ports and
  parameters, sensitivity lists (with fixes), inferred latches, width mismatches in
  assignments and port connections, resets, unreachable and stuck states, positional
  connections (with a fix), process labels, file names, line style and naming
  conventions (new `naming/module`, `naming/parameter` and `naming/task`). Suppression
  comments are written `// vfl-ignore rule`. Tuned on picorv32, verilog-ethernet, ibex,
  cva6, common_cells and sv-tests.
- **Fuzz testing** (`fuzz/`): cargo-fuzz targets for the VHDL and Verilog parsers
  (lossless round trip; Verilog with macros and include cycles), the formatter
  (idempotence, range formatting agreeing with whole-file formatting), the linter and its
  fixes (Verilog's included), incremental re-analysis of mixed VHDL and Verilog projects (equal to a full
  analysis) and the language server under `didChange` edits, and the Verilog formatter.
  CI runs each target briefly.

### Fixed

- **Completion after a non-ASCII character**: the language server panicked when the word
  before the cursor followed a multi-byte character.
- **Range formatting and `-- vfl: on`**: formatting a range indented the `-- vfl: on` line,
  which `vfl fmt` keeps as written, so the two disagreed.
- **Editor requests in a Verilog file**: hover, completion and the other requests made in
  an open Verilog file panicked the language server, which analyzes Verilog files only
  for their modules. They no longer reach the VHDL handlers.
- **Unterminated string at the end of a file**: the type checker panicked when the string
  ended in a multi-byte character.
- **Unterminated Verilog include name**: `` `include "name `` with no closing quote,
  ending in a multi-byte character, panicked the parser.
- **Verilog include cycles**: a file including itself more than once on each level of a
  cycle took exponential time to parse (50 s for 175 bytes). At most 1024 included files
  are now read per file.
- **Range formatting and lone carriage returns**: in a file that breaks lines with a lone
  `\r` (old Mac line endings), formatting a range dropped the blank lines before it. Such
  a file is now formatted whole.
- **Verilog lexer**: a block comment left open in a `` `define `` body and protected text
  with no end are now syntax errors, as an open block comment elsewhere is.

## [0.2.1] - 2026-10-14

### Fixed

- **Git submodules belong to their superproject**: a submodule's `.git` file was taken for a
  repository root, so running `vfl` inside a submodule, or opening it as an editor
  workspace, lost the superproject's other sources and its `vfl.toml`. A submodule is now
  part of the project around it; `root = true` in a `vfl.toml` still makes it a project of
  its own, and linked worktrees stay roots.

## [0.2.0] - 2026-10-13

Documentation and diagrams of the design, code generation, design-level checks, semantic
highlighting, project-wide refactoring, range formatting, and a Notepad++ plugin that never
freezes and matches the language server feature for feature.

### Added

- **Documentation and diagrams** (`vfl doc`, `vfl schema`): HTML, Markdown or AsciiDoc pages
  from the design and its doc comments, with entity symbols, the instantiation tree, the top
  level and every architecture as block diagrams wired by signal, port maps, data flow,
  clock domains and their crossings, reset trees, package dependencies, record and register
  bit fields, state machines and WaveDrom timing diagrams. `vfl schema` draws one diagram as
  SVG, Mermaid or DOT. Settings under `[doc]`; an example in `examples/uart/doc`. In VS Code,
  **vfl: Generate Documentation** and **vfl: Show Diagrams** (a view beside the editor); in
  Notepad++, **Generate documentation** and **Show diagrams**.
- **Code generation** (`vfl new`): entity and component instantiations, component
  declarations and testbench skeletons (clock, reset and stimulus process) from an entity,
  also as editor commands and as a code action on an entity. Settings under `[generate]`.
- **Design checks**: `sema/missing-reset`, `sema/mixed-reset`, `sema/reset-polarity`,
  `sema/open-input`, `sema/inout-port`, `sema/fsm-unreachable-state` and
  `sema/fsm-stuck-state`.
- **Semantic highlighting**: signals, ports (with their mode), variables, constants,
  generics, types, subprograms, entities, components, packages and enumeration literals are
  colored by what the analysis resolved them to, in VS Code and Notepad++.
- **Range formatting and format on type**: `vfl fmt --lines`, Format Selection, and
  formatting of the line just typed (`;` and Enter; `format.on_type`, opt-in in the
  editors).
- **More quick fixes**: removing unused signals, variables and constants and unneeded
  sensitivity entries, indenting with spaces, `numeric_std` for the Synopsys packages, `out`
  for `buffer` ports (VHDL-2008), naming the formals of positional maps, renaming to follow
  a naming convention, adding the `use` clause for an undeclared name, and silencing any
  rule with a `vfl-ignore` comment.
- **Notepad++**: **List problems in project**, quick fixes for a selection, and a test that
  fails when the language server gains a feature the plugin does not have.
- **Documentation**: a [command-line reference](docs/cli.md), and the README, editor READMEs
  and design document brought up to date.

### Changed

- **Rename and find references** treat a package and its body, a subprogram and its body,
  and an entity and the components named after it (with their ports and generics) as one
  thing, work on record elements, and refuse a new name that would clash with another.
- **Notepad++ never freezes**: loading a project, linting, hover, completion, navigation,
  fixes, rename, code generation and documentation run on a background thread, and name
  colors are painted only for the lines on screen.
- Editor requests (highlights, hover, definition, completion, code actions) no longer scan
  the whole project, and formatting no longer waits for an analysis.

## Unreleased

### Added

- **Semantic highlighting**: the language server colors names by what they resolve to
  (signal, port, variable, constant, generic, type, function, procedure, entity, component,
  package, library, enumeration literal, label), with modifiers for a port's or
  parameter's mode, declarations, assignment targets and names from `std` / `ieee`. Whole
  file, range and delta requests are supported; the largest UVVM file (20,000 names) takes
  7 ms. The VS Code extension declares the VHDL-specific token types with standard
  fallbacks, so any theme colors them. The Notepad++ plugin colors signals, ports,
  constants, variables, types and subprograms (**Color names by meaning**, on by default).
- **Range formatting**: format only a selection (`textDocument/rangeFormatting`,
  `vfl fmt --lines FIRST:LAST`, **Format selection** in Notepad++). The selection widens to
  the whole statements it touches and to every line aligned with them, so alignment stays
  consistent, and formatting the same lines again changes nothing.
- **Format on type**: the line of a `;` just typed, or the line above a new line, is
  formatted (`textDocument/onTypeFormatting`). Off by default in the editors
  (`vfl.format.onType` in VS Code, **Format on type** in Notepad++); `on_type = false` under
  `[format]` in `vfl.toml` turns it off for a project. Only the block around the line, between
  blank lines, is laid out, and lines are left alone while the file has syntax errors.
- Formatting requests no longer wait for the project to be analyzed again.

## [0.1.1] - 2026-10-13

A faster language server: editing, opening and hovering in large projects no longer wait for
a full re-analysis. The downloads are the same as in 0.1.0, with `0.1.1` in their names.

### Changed

- **Analysis once per burst of keystrokes**: the language server applies every edit already
  waiting before it analyzes, and skips requests the editor cancelled meanwhile. Before, it
  analyzed after every keystroke, and on large projects requests queued behind those
  analyses (hover took 860 ms after typing a comment in UVVM).
- **Only what an edit can change is refreshed**: library inference runs again only when the
  files, what they declare or reference, or a library list changed; linting gathers
  declarations only for the files it re-lints; only the diagnostics that changed are sent.
  New files and libraries are still detected.
- **Opening or closing an unchanged file does no work**: the first hover in a newly opened
  file no longer waits for a full refresh (70 to 190 ms on UVVM), and closing a file no
  longer walks the project tree. The Notepad++ plugin gets the same fix.

### Added

- `scripts/bench_vhdl_ls.py` benchmarks the language server against rust_hdl's `vhdl_ls`
  (open time, edit-to-diagnostics latency, hover, completion, definition, memory); results
  are in [docs/benchmarks.md](docs/benchmarks.md).
- The release workflow can also be started from the Actions tab (**Release > Run
  workflow** on `main`): it tags the current version and publishes it.

## [0.1.0] - 2026-10-13

First release: a VHDL formatter, linter and language server in pure Rust, with editor
plugins for VS Code and Notepad++.

### Downloads

- `vfl-v0.1.0-<target>.tar.gz` / `.zip`: the `vfl` command-line tool for Linux (x86_64 and
  aarch64, static), macOS (Apple silicon and Intel) and Windows (x86_64).
- `vfl_npp-v0.1.0-x64.zip`: the Notepad++ plugin (64-bit). Unzip it into
  `Notepad++\plugins\`, so the DLL ends up in `plugins\vfl_npp\vfl_npp.dll`.
- `vfl-0.1.0.vsix`: the VS Code extension. Install it with
  `code --install-extension vfl-0.1.0.vsix`; it runs `vfl lsp`, so put `vfl` on the `PATH`
  or set `vfl.server.path`.
- `SHA256SUMS`: checksums of every file above.

### What is in it

- **Parser**: lossless, error-tolerant lexer and parser for VHDL-93, 2008 and 2019.
- **Formatter** (`vfl fmt`): idempotent and token-preserving, every rule configurable,
  including column alignment of ports, generics, assignments, associations, declarations
  and comments; `-- vfl: off` / `on` / `skip` regions.
- **Linter** (`vfl lint`): syntactic and cross-file semantic rules with autofixes, including
  name resolution, overload resolution and type checking, unused objects, width mismatches,
  inferred latches and clock domain crossings, naming and style rules; `recommended` and
  `strict` presets and per-line or per-file suppressions.
- **Zero-config projects**: the root, the sources and their libraries are discovered from
  the code. Library lists are imported from `vhdl_ls.toml`, VUnit `run.py`, FuseSoC
  `.core`, Vivado `.xpr` / Tcl and Quartus `.qsf` / `.qip` files when present, including
  Tcl variables in them.
- **Configuration**: layered `vfl.toml` files with `extends` and `root = true`; every key is
  in [docs/configuration.md](docs/configuration.md) and every rule in
  [docs/lints.md](docs/lints.md).
- **Language server** (`vfl lsp`): incremental re-analysis, diagnostics, formatting, quick
  fixes, hover with types, signature help, completion, go to definition, references,
  rename and symbols.
- **VS Code extension**: syntax highlighting and the language server.
- **Notepad++ plugin**: formatter, linter and the language server's editor features built
  in, no separate `vfl` install needed.
- **Performance**: 1.5 to 2.3 times faster than rust_hdl with 2.8 to 4.1 times less memory
  on neorv32, OSVVM, UVVM and VUnit ([docs/benchmarks.md](docs/benchmarks.md)).

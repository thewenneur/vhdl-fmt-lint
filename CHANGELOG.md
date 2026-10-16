# Changelog

## [Unreleased]

### Added

- **FPGA vendor files are linted and highlighted: Quartus `.qsf` and `.qip`, SDC, Vivado
  `.xdc` and Tcl scripts, Microchip Libero and Lattice Radiant `.pdc` / `.ldc`, Lattice
  Diamond `.lpf` and Gowin `.cst`.** Nine `vendor/*` rules report unbalanced braces, quotes and
  brackets, misspelled commands, options, assignment and property names (with what was
  meant), missing arguments (`create_clock` without `-period`, `set_false_path` without
  `-from`/`-to`), source, IP and script files that do not exist, duplicate assignments,
  one pin assigned to two ports, and ports the top-level entity does not have. `vfl lint`
  checks the vendor files under its paths, and the language server and the Notepad++
  plugin those open in the editor, without analyzing the project again. VS Code gets
  Quartus Settings, SDC / XDC / PDC Constraints, Lattice Preferences and Gowin Constraints
  languages with their grammars, Notepad++ highlights them as Tcl (`.lpf` and `.cst` as C),
  and Zed the Tcl ones with the Tcl grammar. See [docs/lints.md](docs/lints.md#vendor).

### Changed

- **The libraries listed in `.qsf`, `.qip` and Vivado `.tcl` files are read with a real Tcl
  reader**, the one the vendor rules use: commands separated by `;`, comments after `;#`,
  commands and file lists over several lines, and braced words taken literally are now
  understood, and `if` bodies are no longer read as if they always ran.

- **`vfl spec` writes a design specification from the code and spec comments.** Each
  entity gets a page in numbered sections: overview, interface (with the timing of each
  port: clock, reset, registered or sampled on which clock, or combinational, followed
  through the instances), clocks and resets with what uses them, behavior, state machines,
  registers with their clock and reset, sub-blocks and requirements. `-- @spec text`,
  `-- @req ID: text` and `-- @verifies ID, ID` comments add to the place they apply to,
  and the index lists every requirement with the tests that verify it. Duplicate IDs and
  unknown verified requirements are reported (`--check` fails on them). Pages are HTML,
  Markdown or AsciiDoc; tags, sections and output are set under `[doc.spec]`. Spec
  comments are left out of `vfl doc`. The editors have the action too: **vfl: Generate
  Specification** in VS Code, **Generate specification** at the end of the Notepad++
  menu, and a `vfl: generate specification` task (`ctrl-alt-v s`) for Zed.
  [docs/spec.md](docs/spec.md) is the guide, [examples/uart/spec](examples/uart/spec/index.md) the example.

- **State machine diagrams are drawn with orthogonal lines, and the transitions into the
  same state merge into one arrow.** All the returns to `IDLE` join one trunk with a single
  arrowhead, as do all the errors into `FAULT`; trunks that many states join run down the
  side of the diagram instead of weaving between the branches. The transitions leaving a
  state share one stem that branches out, each condition sits on its own branch, before
  it merges with others, and lines are lined up so that they run straight. The DOT export
  merges them too (`concentrate`).

### Fixed

- Bugs found by a 12-hour fuzzing campaign (`.github/workflows/fuzz-campaign.yml`):
  `style/process-all` no longer panics on a sensitivity list cut short by a syntax error
  (`process(==x)`); the Verilog formatter keeps `1'1b` as written instead of turning
  `'1` into a literal; a `-- vfl: off` just before an `end` kept as written no longer
  moves at each format; and a Verilog parameter used in a VHDL expression
  (`2 ** WIDTH - 1` in an architecture of a Verilog module) no longer crashes the linter
  and the language server.

## [0.8.0] - 2026-10-16

New design checks (multiple drivers, combinational loops, incomplete and duplicate
case choices, range and width checks, dead code), waivers and baselines, `vfl order` for
compile scripts, more project imports and bundled vendor primitives, inlay hints, call
hierarchy and a design hierarchy view in the editor, VSG-style format defaults, and a
language server that keeps typing responsive on large projects.

### Added

- **Language server: invalid `vfl.toml` diagnostics.** A syntax error, unknown key or
  invalid value in a config file is shown on the file itself, with its range, besides the
  message. The message comes again if the file is fixed and then broken again. An invalid
  nested config no longer silently gives the files under it the default settings: it is
  reported, and they get the config of the valid files above it (or the editor's
  settings).
- **Language server: loading progress.** Editors that support `$/progress` show how many
  files are parsed while the project loads.
- **Language server: file watching without the editor's watcher.** In editors that cannot
  watch files for the server (no dynamic registration of
  `workspace/didChangeWatchedFiles`), the server now looks for new, changed and deleted
  sources and config files itself, every few seconds (more seldom in large projects).
- **`[project]` `max-file-size`** (2 MB by default): the language server analyzes only the
  entities and packages of larger VHDL files, generated netlists say, and does not lint
  them, with an information diagnostic saying so. A 3 MB netlist (16,000 instances) made
  opening its project take 6.4 s and each edit of it about 7 s; now 0.34 s and under
  0.3 s.

- **`vfl order`** prints the files of the project in compile order with their libraries:
  as a list, as JSON, or as a script for GHDL (`ghdl -a --work=lib`), Questa/ModelSim
  (`vlib`, `vmap`, `vcom`, `vlog`) or Vivado (`read_vhdl -library`). `--top unit` keeps only
  what an entity or configuration needs. Files that use each other's units are reported
  and make the command fail.
- **More project imports**: library lists are also read from `.f` file lists (`-makelib`
  and `-work` libraries, nested `-f`/`-F` lists, environment variables; `+incdir+`, `-v`,
  `-y` and other options are skipped), Edalize EDAM files (`*.eda.yml`, `edam.json`),
  hdlmake `Manifest.py` (`library` and `files`, read without running it) and
  Questa/ModelSim `.mpf` projects (`compile_to`).
- **Vendor primitive libraries**: declarations of the commonly used Xilinx `unisim` and
  `xpm`, Intel `altera_mf` and Lattice (`ecp5u`, `machxo2`, `sb_ice40_components_syn`)
  primitives are bundled and analyzed when the design uses one of these libraries and the
  project has no copy of it. Their components and entities resolve (hover, go to
  definition, type checks of the actuals) instead of the library being left unchecked;
  primitives, ports and generics they do not declare are not reported. `[project]`
  `vendor-stubs = false` turns them off.
- **Range checks**: `sema/index-range` (warn in `recommended`, error in `strict`) reports an
  index or slice outside an array's static range, a slice that goes `to` on a `downto`
  vector (or the reverse), a null slice, and a loop parameter that runs past the array;
  bounds may come from constants and, with the `generics` option (on by default), from
  generics' default values. `sema/value-range` (same levels) reports a static integer
  value outside the range of the subtype it is given to (`natural := -1`).
- **Dead code**: `sema/unreachable-code` (warn) reports statements after `return`, an
  unconditional `exit` or `next`, or `wait;`. `sema/constant-condition` (info) reports
  `if`, `elsif` and `when` conditions that are the literal `true` or `false` (boolean
  constants too with the `constants` option; `if` generates are left alone).
  `sema/unused-subprogram` (warn) reports functions and procedures nothing calls; those of
  packages only with the `public` option, against the whole project.
- **`sema/comparison-width`** (warn in `recommended`, error in `strict`): a predefined `=`,
  `/=`, `<`, `<=`, `>` or `>=` between arrays of different static widths, which is always
  false for `=` and compares from the left for the others. `numeric_std` comparisons are
  not reported.
- **Synthesis checks** for the architectures of entities with ports (off in `recommended`):
  `sema/synthesis-delay` (`after`, `wait for`), `sema/synthesis-file-io` (files and
  `std.textio` in processes), `sema/synthesis-loop` (loops bounded by a signal or
  variable), `sema/synthesis-initial-value` (signal initial values, `info` in `strict`)
  and `sema/synthesis-real-port` (ports of type `real`). Code between `synthesis
  translate_off` and `translate_on` comments is skipped.
- **Style rules** (off in `recommended`): `style/identifier-case` (a name written with
  another case than its declaration, with a safe fix), `style/boolean-comparison`
  (`x = true`, with a safe fix), `style/magic-number` (integer literals in statements,
  with an `allow` list), `style/file-header` (the leading comments must match `pattern`),
  `style/one-unit-per-file`, `style/generic-default`, `style/port-order` (clock and reset
  ports first, by name), `style/process-all` (VHDL-2008 combinational processes, with an
  unsafe fix) and `style/shared-variable` (of protected types too with `protected`).
- **`sema/incomplete-case`** (error): a `case` statement or selected assignment whose
  choices leave out values of the selector's subtype (enumerations, integer subtypes with
  static bounds, `std_logic_vector` and other character arrays) and has no `when others`.
  The fix (unsafe, also an editor quick fix) adds the missing `when` alternatives with a
  `null` body, or `when others => null;` when more than 8 are missing.
- **`sema/duplicate-choice`** (error): a choice that names a value an earlier choice
  covers, such as a literal listed twice or overlapping integer ranges.
- **`sema/multiple-drivers`** (warn in `recommended`, error in `strict`): a signal, or an
  overlapping static part of it, driven by more than one process, concurrent assignment
  or instance output. The `resolved` option (`"rtl"` by default, `"always"`, `"never"`)
  says when signals of resolved types such as `std_logic` are reported; tri-state drivers
  (`'Z'`) never are.
- **`sema/combinational-loop`** (warn in `recommended`, error in `strict`): a signal that
  depends on itself through concurrent assignments and combinational processes without a
  register, reported once per loop with its path (`a -> b -> a`).
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
- **Inlay hints** in the language server: the port, generic or parameter name before each
  positional actual of a port map, generic map or subprogram call, and optionally the
  mode and type of each port map actual. Only the range on screen is worked out, from the
  existing analysis. Each kind can be turned on or off under `[lsp.inlay_hints]` in
  `vfl.toml`, and per user from the editor (`inlayHints` in the initialization options;
  `vfl.inlayHints.*` in VS Code).
- **Go to implementation** (entity to architectures, component to entity, package and
  subprogram to body), **go to type definition** and **go to declaration** (body to what it
  completes) in the language server; **Go to implementation** and **Go to type
  definition** in Notepad++.
- **Linked editing** of a name and its `end` name, and **selection ranges** (VS Code's
  expand selection; **Expand selection** in Notepad++).
- **Call hierarchy** for subprograms (callers and callees) and for the instance tree
  (which architectures instantiate an entity, what an architecture instantiates), and an
  **"N instances" code lens** on entities and components, counted only when shown and
  listing the instances when clicked in VS Code; **Show call hierarchy** in Notepad++.
- **32-bit and ARM64 Notepad++**: releases have `vfl_npp-vX.Y.Z-x86.zip` and `-arm64.zip`
  next to the 64-bit plugin, and a `vfl` binary for Windows on ARM.
- **VHDL snippets** in VS Code (and a copy for Zed's snippets directory): entity and
  architecture, clocked processes with a synchronous, asynchronous or no reset, a
  combinational process, a state machine, a testbench, generate statements, a package with
  its body and a record type.
- **VS Code language status**: the server's version and state with a restart action, and
  the `vfl.toml` in effect for the file in front with an action to open it (or to generate
  one).
- **A JSON schema of `vfl.toml`** ([editors/vscode/schemas/vfl.schema.json](editors/vscode/schemas/vfl.schema.json)),
  which VS Code hands to Even Better TOML for completion, hover and checking. Tests keep it
  in sync with the config keys, their accepted values and defaults, and the lint rules.
- **VS Code downloads the server** of its own version when no `vfl` is found, after asking,
  and checks it against the release's `SHA256SUMS`. `vfl.server.path`, the `PATH` and
  `~/.cargo/bin` still come first.
- **VS Code multi-root workspaces**: one server per workspace folder.
- **Long expressions are broken**: a line still longer than `max_width` once its lists are
  broken up is broken at the operators of its expressions that bind loosest (`else` of
  conditional assignments, then `and`/`or`, relational, shift, `+`/`-`/`&`, `*`/`/`), in
  conditions, assignments and port map actuals. `operator_position` (`"trailing"`,
  `"leading"`, `"preserve"`) places the operator; `break_expressions = false` turns it off.
  Lines that fit are not changed.
- **`[format.declaration_case]`**: the case of identifiers by the kind of their declaration
  (constants and generics in upper case, ports in lower case...), applied to every use of
  the name in the file.
- **`[format.blank_lines]`**: blank lines before and after `begin`, before `end`, before
  processes and between declaration groups, as an exact number or a `{ min, max }` range.

- **Design hierarchy view**: the language server answers a `vfl/hierarchy` request with
  the instantiation tree of the project, from a chosen top or from every entity no other
  entity instantiates (testbenches optionally left out): each instance with its label,
  entity (or component, configuration or Verilog module), library and architecture, where
  the instance and the entity are, generate statements and blocks as groups, and the
  instances that bind to nothing flagged. It is built from the analysis only when asked,
  never while typing, and bounded against recursion. VS Code shows it as a **VHDL
  Hierarchy** view in the Explorer (top picked with a quick pick and kept in
  `vfl.hierarchy.top`; open the entity, set as top or generate a testbench from an
  instance), refreshed after saves and analyses; Notepad++ lists it with **Show design
  hierarchy**. Zed has no tree view for extensions.

### Changed

- **Language server: analysis on its own thread.** The server reads edits, formats
  documents and takes cancellations while it analyzes: formatting another file during the
  re-analysis that follows an edit of UVVM's `types_pkg` takes 14 ms instead of 330 ms, and
  a cancelled request waiting for that analysis is answered at once instead of after it
  (6 ms instead of 330 ms). Requests that need the analysis still wait for the analysis of
  the latest edits.
- The language server converts and sends only the diagnostics of files whose findings
  changed (3 times faster: 0.08 ms instead of 0.25 ms after an edit in UVVM), applies the
  changes of an edit without copying the document twice or indexing its lines once per
  change, and finds a file by its path in a map rather than by a search of the project.
- The language server reads files created or deleted outside the editor together: a
  burst of watched-file events (a `git checkout`, a generator) costs one walk of the
  project at most, and none when the new files are in directories the project already
  has files in (300 new files: 13 ms instead of 300 walks, 370 ms on a 300-file project),
  then one analysis.
- Reading which units a file declares and uses no longer takes time quadratic in the
  number of statements of an architecture: 28 ms instead of 3.7 s for a 3 MB netlist,
  on every load and every edit of it.
- **Format defaults follow VHDL Style Guide (VSG)**, which TerosHDL also uses: `max_width`
  is 120 (was 100, now the same as the `style/line-length` rule), `max_blank_lines` is 1
  (was 2) and `association_list` is `"expand"` (was `"auto"`), so port and generic maps get
  one association per line like port and generic clauses. Code formatted with the old
  defaults only loses its extra blank lines and gets its one-line maps expanded: no line
  is joined. `max_width = 100`, `max_blank_lines = 2` and `association_list = "auto"` bring
  the old layout back.
- **Zed downloads the server of its own version**, not the latest release, and checks the
  archive against the release's `SHA256SUMS` before keeping the binary; Windows on ARM
  gets a binary too. The example tasks run from the worktree root (`$ZED_WORKTREE_ROOT`)
  instead of the current file's directory, and the README says they need `vfl` on the
  `PATH`.
- **Notepad++ format on save skips files larger than 512 KB**, with a note in the status
  bar, since it formats before the file is written and holds up the save;
  `format_on_save_max_kb` in `vfl_npp.ini` sets the limit.
- VS Code: vfl is the default formatter of Verilog and SystemVerilog files too, a workspace
  you do not trust cannot set `vfl.server.path` or `vfl.server.extraEnv`, and the extension
  has an icon and a changelog.
- The formatter's `max_width` and the `style/line-length` rule count display columns:
  Chinese, Japanese and Korean characters take two.
- A line continuing an element of a list (a port map actual broken over lines) is indented
  `continuation_indent` further than the elements.
- LSP position conversion finds the line of an offset without scanning the line's text
  twice.
- The fuzzing dictionary and seeds include non-ASCII text (no-break and ideographic spaces,
  accents, CJK, an emoji outside the BMP) and CRLF line breaks.

### Fixed

- **Language server crash on hover** over a declaration continued on lines indented with
  no-break spaces (or other multi-byte spaces): the re-indentation sliced a line by a
  character count. Adding port associations next to such indentation, or after an edit
  that changes a multi-byte character, no longer panics either.
- **The language server survives a bug in a handler**: a request that panics fails with
  `InternalError` (and the panic is logged to stderr) instead of killing the server; a
  notification or an analysis that panics is skipped the same way.
- **Edits past the end of a CRLF line** are inserted before the `\r\n`, not between `\r`
  and `\n`.
- Name resolution no longer panics on an unbalanced scope (a bug); it degrades instead.
- Verilog: an empty position in an instance's connections (`sub u (a, , c)`) leaves its
  input unconnected, which `verilog/unconnected-input` now reports, and the fix of
  `style/positional-association` names it (`.b()`) instead of leaving an empty position
  among named connections, which did not parse.
- `style/file-name` suggests a name with the file's own extension (`top.vhdl` for a
  `.vhdl` file) instead of always `.vhd`.
- `vfl lint` run in `examples/` no longer mixes the configuration lessons with the UART
  design (15 errors from duplicate entities): each lesson of `examples/config` is a
  project of its own (`root = true`), and `examples/uart` is formatted with the default
  style.

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
- **A unit's references are kept in source order**, which the checks that look names up by
  position (`sema/multiple-drivers`, `sema/combinational-loop`) assume. Operators were
  recorded before their operands, so a binary search could miss a name next to one.

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

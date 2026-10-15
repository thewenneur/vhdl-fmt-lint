# Configuration reference

vfl reads its settings from `vfl.toml` (or `.vfl.toml`). Every key has a default, so the file
is optional, and an empty file is the same as no file. This page lists every key. The lint
rules that `[lint.rules]` configures are described in [lints.md](lints.md).

`vfl config show [path]` prints the effective configuration for a file or directory, with
the config files it was built from.

For a guided tour, [examples/config](../examples/config/README.md) has one folder per
topic, each with a `vfl.toml` and a file before and after `vfl fmt` or `vfl lint --fix`.

- [Config files](#config-files): where they are looked up and how they combine
- [`[project]`](#project): sources, standard and libraries
- [`[format]`](#format): the formatter
- [`[format.align]`](#formatalign): column alignment
- [`[lint]`](#lint): presets and per-rule settings
- [`[doc]`](#doc): documentation generation (`vfl doc`)
- [`[doc.diagrams]`](#docdiagrams): the diagrams in the documentation (`vfl schema`)
- [In-source comments](#in-source-comments): suppressing findings, switching the formatter off
- [Default configuration](#default-configuration): every key with its default value

## Config files

### Lookup

For a file, vfl walks up from the file's directory and applies every `vfl.toml` (or
`.vfl.toml`) it finds on the way to the filesystem root. If a directory holds both,
`vfl.toml` is used.

The files are merged from the outermost to the nearest, so a nested config only needs the
keys it changes:

```text
repo/
  vfl.toml           # [lint] preset = "strict"
  tb/
    vfl.toml         # [lint.rules] "sema/unused-signal" = "off"
```

Files under `tb/` are linted with the strict preset, without `sema/unused-signal`.

Tables merge key by key. Any other value, arrays included, replaces the value from the
outer file: an inner `exclude = ["a/**"]` replaces an outer `exclude`, it does not add to it.

### Without a config file

Where no config file applies, every key has its default, except the ones the editor sets:
the lint preset and format style of the VS Code, Notepad++ and Zed integrations, or any key
passed by another LSP client (see [editors.md](editors.md#editor-settings)). A config file
replaces those editor settings entirely, so that a project is linted and formatted the same
in every editor and on the command line. The editors' **Generate project config** command
writes the settings in effect to `vfl.toml`, with every key at its default value as a
comment.

### Special top-level keys

| Key | Type | Meaning |
|-----|------|---------|
| `root` | boolean | `root = true` stops the upward search: config files above this one are ignored. |
| `extends` | string | A path to another config file, relative to this file's directory. That file is loaded first and this file is merged on top of it. It may itself use `extends`; a cycle is an error. A `root` key in the extended file is ignored. |

```toml
# hw/vfl.toml
root = true
extends = "../shared/team-vfl.toml"

[format]
indent = 4   # overrides the team file
```

### Which settings apply where

- `[format]` and `[lint]` are resolved per directory: each file gets the settings of the
  configs above it, so a subtree can use its own style or rules.
- `[project]` is read from the configs that apply to the project root (see
  [`[project]`](#project)).

### Errors

A key that does not exist, or a value of the wrong type, is an error that names the key and
lists the valid ones, and the command stops. An unknown rule id in `[lint.rules]`, or an
invalid rule option, is reported as a warning and ignored, so that a config written for a
newer vfl still works.

## `[project]`

What the project is made of: the files analyzed together, so that names resolve across
them.

The project root is found by walking up from the file or directory vfl was started on:

- The outermost directory holding a `vfl.toml` or `.vfl.toml` is the root, so nested configs
  that only override a few settings do not split the project. The walk stops at a config
  with `root = true`, and does not go above the nearest version-control root (`.git`, `.hg`,
  `.jj`): a config in your home directory is still layered underneath, but does not make
  the home directory a project.
- Without a config, the nearest version-control root is the root; failing that, the starting
  directory itself. The language server starts from its first workspace folder.
- A git submodule is not a version-control root of its own: its files belong to the
  superproject, so opening the submodule (or a file in it) finds the superproject's root
  and `vfl.toml`. Put a `vfl.toml` with `root = true` in the submodule to make it a
  separate project. Linked worktrees stay roots of their own.

To keep two VHDL projects in one repository apart, give each its own `vfl.toml` with
`root = true`.

`[project]` is read from the configs that apply to the root. In a nested config, only
`standard` has an effect: `vfl fmt` and `vfl check` parse the files under it with that
standard (the semantic analysis of `vfl lint` uses the root's).

| Key | Type | Default | Meaning |
|-----|------|---------|---------|
| `standard` | string | `"2008"` | The VHDL standard to parse: `"93"` (or `"1993"`), `"2008"` (or `"08"`), `"2019"` (or `"19"`). |
| `extensions` | array of strings | `["vhd", "vhdl"]` | File extensions, without the dot, of the files treated as VHDL sources. |
| `exclude` | array of strings | `[]` | Gitignore-style globs, relative to the project root, of files and directories to skip. `.gitignore` files are honoured as well. |
| `libraries` | table | `{}` | Explicit library assignments; see below. |
| `vhdl-ls` | string | `"fallback"` | How rust_hdl's `vhdl_ls.toml` places files in libraries: `"fallback"` (only the files inference places nowhere, or over inference when that leaves fewer library conflicts), `"prefer"` (over inference and tool files, below `[project.libraries]`) or `"off"` (not read). See below. |
| `vendor-stubs` | boolean | `true` | Load vfl's bundled declarations of vendor primitive libraries (`unisim`, `xpm`, `altera_mf`, `ecp5u`, `machxo2`, `sb_ice40_components_syn`) for a library the design uses and the project does not hold. See [Vendor libraries](#vendor-libraries). |

### `[project.libraries]`

Normally not needed: vfl puts each file in a library by itself. In order of priority:

1. `[project.libraries]` in `vfl.toml`.
2. Library lists of other tools found in the tree, read without running them: VUnit
   `run.py`, FuseSoC `.core`, Edalize EDAM (`*.eda.yml`, `edam.json`), Vivado `.xpr` and Tcl
   `read_vhdl -library` scripts, Quartus `.qsf` / `.qip`, `.f` file lists (`-makelib` and
   `-work` libraries, nested `-f`/`-F`), hdlmake `Manifest.py` (`library` and `files`) and
   Questa/ModelSim `.mpf` projects (`compile_to`). See
   [design.md](design.md#project-autodiscovery) for what each one is read for.
3. Inference from the code: a unit referenced as `lib.unit` puts the file that declares it
   in `lib`, and `work.unit` references keep files together.
4. rust_hdl's `vhdl_ls.toml` (or `.vhdl_ls.toml`) in the project root, with `$VAR`
   expansion, for the files inference placed nowhere. The libraries it gives them spread
   along `work.unit` references in turn.
5. Files nothing places stay in `work`.

A `vhdl_ls.toml` written by hand often lags behind the code, so by default it only fills
the gaps. When the result has library conflicts (a `lib.unit` or `work.unit` reference to
a unit of the project that is not in that library, or a unit declared twice in one
library), vfl also tries ranking `vhdl_ls.toml` above tool files and inference, and keeps
that if it leaves fewer conflicts; `vfl project` then says so. The language server makes
this choice when it loads the project or a library list changes, not on every edit. This is what happens with
UVVM, whose VIPs each declare packages of the same names. Set `vhdl-ls = "prefer"` to
always rank it there, as earlier versions did, or `vhdl-ls = "off"` to ignore it.

`vfl project` shows the library of each file and why it was chosen. Add an entry here only
when that guess is wrong. Each key is a library name and its value a list of globs,
relative to the project root, of the files in it:

```toml
[project.libraries]
osvvm = ["third_party/osvvm/**/*.vhd"]
uvvm_util = ["third_party/uvvm/uvvm_util/src/*.vhd"]
```

In these globs `*` also matches `/`, so `src/*.vhd` matches files in subdirectories too.
A file can be listed in several libraries; it is then analyzed once per library. A file no
entry matches is still placed by the lower-priority sources.

`vfl project --vhdl-ls` prints the libraries vfl found as a `vhdl_ls.toml`, for use with
rust_hdl.

### Vendor libraries

Designs instantiate FPGA primitives from libraries the project does not hold: they come
with the vendor's tools. Without them, names from such a library are not checked, and
`sema/unknown-library` says so. vfl bundles declarations of the commonly used primitives,
written from the vendors' public documentation, and analyzes them when a file uses the
library (`use unisim.vcomponents.all`, `entity unisim.bufg`) and no file of the project
is in it:

| Library | Package | Primitives |
|---------|---------|------------|
| `unisim` | `vcomponents` | Xilinx 7 series and later: clock buffers (`BUFG`, `BUFGCE`, `BUFGMUX`, `BUFR`, ...), I/O buffers (`IBUF`, `OBUF`, `IOBUF`, `IBUFDS`, `OBUFDS`, ...), flip-flops (`FDRE`, `FDSE`, `FDCE`, `FDPE`), `LUT1`-`LUT6`, `SRL16E`, `SRLC32E`, `CARRY4`, `MUXF7/8`, `ODDR`, `IDDR`, `IDELAYE2`, `ODELAYE2`, `IDELAYCTRL`, `ISERDESE2`, `OSERDESE2`, `MMCME2_BASE/ADV`, `PLLE2_BASE/ADV`, `RAMB18E1`, `RAMB36E1`, `FIFO36E1`, `DSP48E1`, `STARTUPE2`, `DNA_PORT` |
| `xpm` | `vcomponents` | `xpm_cdc_*`, `xpm_fifo_sync`, `xpm_fifo_async`, `xpm_memory_spram/sdpram/tdpram/sprom/dprom` |
| `altera_mf` | `altera_mf_components` | `altsyncram`, `scfifo`, `dcfifo`, `altpll`, `altddio_in`, `altddio_out` |
| `ecp5u`, `machxo2` | `components` | Lattice I/O buffers, flip-flops, `DCCA`, `GSR`, `PUR`, PLLs (`EHXPLLL`, `EHXPLLJ`), oscillators, DDR registers, block RAM (`DP16KD`, `DP8KC`) |
| `sb_ice40_components_syn` | `components` | Lattice iCE40: `SB_IO`, `SB_GB`, `SB_PLL40_CORE`, `SB_RAM40_4K`, oscillators, `SB_LUT4`, `SB_CARRY`, flip-flops |

The components and the entities of the same names then resolve: hover, go to definition and
completion work, and an actual of the wrong type is reported. The declarations hold each
primitive's ports and the generics most often set, not all of them: a primitive, port or
generic they lack is not reported, and port widths are not checked against them. A project
holding a copy of the library (its sources, in that library) uses that instead. Set
`vendor-stubs = false` to go back to leaving these libraries unchecked.

## `[format]`

Settings of `vfl fmt` and of formatting in the language server.

| Key | Type | Default | Meaning |
|-----|------|---------|---------|
| `style` | string | `"default"` | A named set of defaults for the other keys: `"default"`, `"leading-comma"`, `"compact"` or `"classic"`; see [Styles](#styles). Keys set explicitly win over it. |
| `max_width` | integer | `100` | Lines longer than this get their parenthesized lists broken up, one element per line. |
| `indent` | integer | `4` | Columns per indentation level. Also the tab width with `use_tabs`. |
| `continuation_indent` | integer | `indent` | Columns added for a statement continued on the next line, outside parentheses. |
| `use_tabs` | boolean | `false` | Indent with tabs instead of spaces. Alignment padding is always spaces. |
| `keyword_case` | string | `"lower"` | Case of reserved words: `"lower"`, `"upper"` or `"preserve"`. |
| `identifier_case` | string | `"preserve"` | Case of basic identifiers: `"lower"`, `"upper"` or `"preserve"`. Extended identifiers (`\Foo\`) never change. |
| `end_label` | string | `"preserve"` | Whether `end` repeats the construct's name or label: `"add"` (`end entity counter;`, `end process p_count;`), `"remove"` (`end entity;`) or `"preserve"`. |
| `end_keyword` | string | `"preserve"` | Whether `end` repeats the optional construct keyword (`end entity`, `end architecture`, `end package body`, `end function`, ...): `"add"`, `"remove"` or `"preserve"`. |
| `space_before_label_colon` | boolean | `true` | `p_count : process` rather than `p_count: process`. |
| `space_after_keyword` | boolean | `true` | A space between `(` and the keyword before it: `process (clk)`, `port map (`, `if (`. Applies after `process`, `port`, `generic`, `map`, `parameter`, `array`, `block`, `if`, `elsif`, `while` and `until`. Calls and indexing never get a space: `f(x)`, `v(7 downto 0)`. |
| `max_blank_lines` | integer | `2` | Longer runs of blank lines are shortened to this many. |
| `interface_list` | string | `"expand"` | Layout of `generic (...)` and `port (...)` clauses; see [List layouts](#list-layouts). |
| `association_list` | string | `"auto"` | Layout of `generic map (...)` and `port map (...)`; see [List layouts](#list-layouts). |
| `close_paren` | string | `"own-line"` | Where `)` goes in a list laid out one element per line: `"own-line"` (on its own line, at the indentation of the line that opened the list) or `"attached"` (right after the last element). |
| `interface_separator` | string | `"trailing"` | Where the `;` between the elements of `generic (...)`, `port (...)` and subprogram parameter lists goes when they are on separate lines: `"trailing"` (end of the line) or `"leading"` (start of the next line); see [Separators](#separators). |
| `association_separator` | string | `"trailing"` | Where the `,` between the elements of `generic map (...)`, `port map (...)`, calls, aggregates and enumeration types goes when they are on separate lines: `"trailing"` or `"leading"`; see [Separators](#separators). |
| `case_alternative` | string | `"preserve"` | `"preserve"` keeps a first statement written on the `when ... =>` line there; `"break"` starts every statement of a case alternative on its own line. |
| `line_ending` | string | `"auto"` | `"auto"` keeps the line endings the file uses (LF for new files), `"lf"` or `"crlf"` converts. |
| `on_type` | boolean | `true` | Lets editors format the line you just finished as you type: the line of a `;`, or the line above a new line. Editors have their own switch as well (`vfl.format.onType` in VS Code, **Format on type** in Notepad++), off by default; `false` here turns it off for everyone working on the project. In a block larger than 16 KB (with `[format.align] scope = "block"`), the line aligns with the lines near it rather than the whole block, to stay fast; formatting the file or a selection aligns the rest. |

`end_label` and `end_keyword` apply to entities, architectures, packages, package bodies,
contexts, configurations, components, subprogram bodies, processes, blocks, `if`, `case`,
loops, generate statements, records, physical types and protected types. A clause holding
a comment is left alone.

### Verilog and SystemVerilog

The same settings format Verilog and SystemVerilog files, where they have a meaning:

- `max_width`, `indent`, `continuation_indent`, `use_tabs`, `max_blank_lines`,
  `line_ending` and `on_type` apply as for VHDL.
- `interface_list` lays out port lists and `#(...)` parameter lists, `association_list`
  the connections of an instance and its `#(...)` parameter values, and `close_paren`
  places their `)`. `interface_separator` places the `,` of port and `#(...)` parameter
  lists, `association_separator` that of connections, parameter values and calls.
- `space_after_keyword` puts a space between `(` and `if`, `for`, `foreach`, `while`,
  `case`, `repeat` and `wait`.
- `keyword_case` and `identifier_case` do not apply: Verilog is case-sensitive. Nor do
  `end_label`, `end_keyword`, `space_before_label_colon` and `case_alternative`.

`begin` and `else` stay where they are written: on the line of the `if` or `always`, or
on a line of their own. A statement that is the body of an `if`, a loop or an `always`
without `begin` is indented one level when it starts a line. Compiler directives keep the
indentation they have, and code in an `` `ifdef `` branch that is not compiled is kept as
written. `// vfl: off` and `// vfl: on` leave the lines between them alone.

### List layouts

`interface_list` and `association_list` take:

- `"expand"`: always one element per line, with `(` ending the first line.
- `"auto"`: one element per line if the list already spans several lines or does not fit in
  `max_width`; otherwise kept on one line.
- `"preserve"`: keep the line breaks as written; expand only if it does not fit.

With the defaults:

```vhdl
entity counter is
  generic (
    WIDTH     : natural := 8;
    RESET_VAL : natural := 0
  );
  port (
    clk   : in  std_logic;
    count : out unsigned(WIDTH - 1 downto 0)
  );
end;
...
  u0 : entity work.sub
    port map (clk => clk, data_in => cnt, q => open);
```

With `interface_list = "auto"`, `association_list = "expand"`, `close_paren = "attached"`:

```vhdl
entity counter is
  generic (WIDTH : natural := 8; RESET_VAL : natural := 0);
  port (
    clk   : in  std_logic;
    count : out unsigned(WIDTH - 1 downto 0));
end;
...
  u0 : entity work.sub
    port map (
      clk     => clk,
      data_in => cnt,
      q       => open);
```

### Separators

`interface_separator` and `association_separator` take:

- `"trailing"`: the separator ends the line, as above.
- `"leading"`: the separator starts the next line. In a list whose first element starts
  a line, the separator sits left of the elements so that they line up: with `indent = 2`
  it is under the `(` line's first character, with `indent = 4` two columns further in.

A comment after a separator stays at the end of its line, and the separator moves to the
start of the next element; formatting with `"trailing"` moves it back. A list kept on one
line is not changed: `f(a, b)` stays as it is. With `interface_separator = "leading"` and
`association_separator = "leading"`:

```vhdl
entity counter is
  generic (
    WIDTH     : natural := 8 -- bits
  ; RESET_VAL : natural := 0
  );
  port (
    clk   : in  std_logic
  ; count : out unsigned(WIDTH - 1 downto 0)
  );
end;
...
  u0 : entity work.sub
    port map (
      clk     => clk
    , data_in => cnt
    , q       => open
    );
```

### Styles

`style` picks a set of defaults for the other `[format]` and `[format.align]` keys. Any key
set explicitly, in the same file or in another config file that applies, wins over the
style:

```toml
[format]
style = "leading-comma"
association_list = "auto" # wins over the style's "expand"
```

| Style | Sets |
|-------|------|
| `"default"` | Nothing: every key has the default listed above. |
| `"leading-comma"` | `interface_separator = "leading"`, `association_separator = "leading"`, `association_list = "expand"`. |
| `"compact"` | `indent = 2`, `interface_list = "auto"`, `association_list = "auto"`, `close_paren = "attached"`, `max_blank_lines = 1`, and every `[format.align]` key off except `trailing_comment`, which aligns consecutive lines only (`scope = "contiguous"`). |
| `"classic"` | `indent = 4`, `keyword_case = "upper"`, `end_label = "add"`, `end_keyword = "add"`, `association_list = "expand"`, `case_alternative = "break"`, and alignment of consecutive lines only, without the `;` (`scope = "contiguous"`, `semicolon = false`). |

## `[format.align]`

Which columns the formatter lines up, and across which lines. Every token is aligned by
default, across the whole block around it (`scope = "block"`).

| Key | Type | Default | Aligns |
|-----|------|---------|--------|
| `port_colon` | boolean | `true` | `:` in port clauses and subprogram parameter lists. |
| `port_mode` | boolean | `true` | The subtype after the mode (`in`, `out`, ...) in port clauses and parameter lists: `clk : in  std_logic;` / `q : out std_logic;`. |
| `generic_colon` | boolean | `true` | `:` in generic clauses. |
| `interface_default` | boolean | `true` | `:=` default values in generic and port clauses. |
| `assignment` | boolean | `true` | `<=` and `:=` of assignment statements. |
| `association` | boolean | `true` | `=>` in port maps, generic maps and aggregates laid out one element per line. |
| `declaration` | boolean | `true` | `:` and `:=` in object declarations and record elements, and `is` in type and subtype declarations. |
| `case_choice` | boolean | `true` | `=>` of case alternatives: all of them with `scope = "block"`, else those whose statement is on the same line. |
| `when_clause` | boolean | `true` | `when` in conditional and selected assignments. |
| `conditional_continuation` | boolean | `true` | Continuation lines of conditional and selected assignments start under the first value instead of one indentation level in. |
| `trailing_comment` | boolean | `true` | Comments at the end of lines. |
| `semicolon` | boolean | `true` | The `;` ending a line that has another aligned token: interface elements, declarations and statements each line up their `;` among themselves. |
| `scope` | string | `"block"` | Which lines align together: `"block"` or `"contiguous"`; see below. |
| `block_boundary` | string | `"blank-line"` | With `scope = "contiguous"`, what ends a run of aligned lines; see below. |

In Verilog and SystemVerilog files, `port_mode`, `port_colon`/`generic_colon` and
`interface_default` line up the types, names and default values of ports and
parameters; `declaration` those of net, variable, port and parameter declarations (each
kind among themselves) and of struct members; `assignment` the `=`, `<=` and compound
operators of consecutive assignments; `association` the `(` of named connections
(`.clk  (clk)`); and `case_choice` the statements after the `:` of case items.

`scope` takes:

- `"block"`: a token lines up with the same token everywhere in the block around it, even
  with other lines, blank lines or comments in between: every `<=` of a process (at any
  `if` depth), every `=>` of the port maps of an architecture, every `:` of a declarative
  part. A block is a process, an architecture, a block or generate body, a subprogram, an
  entity, a package or package body, a component, a record or protected type, and each
  generic, port or parameter list (in Verilog: a module, interface, program or package, an
  `always`/`initial` procedure, a function or task, a generate block, a struct, and each
  port or parameter list). `block_boundary` does not apply.
- `"contiguous"`: only consecutive lines of the same construct line up, until a
  `block_boundary`. This was the default before 0.5.

```vhdl
-- scope = "block" (the default), semicolon = true
process (clk)
begin
    if rising_edge(clk) then
        if rst = '1' then
            counter_value <= (others => '0');
        else
            q             <= a              ; -- out
        end if;
    end if;
end process;

-- scope = "contiguous", semicolon = false
process (clk)
begin
    if rising_edge(clk) then
        if rst = '1' then
            counter_value <= (others => '0');
        else
            q <= a; -- out
        end if;
    end if;
end process;
```

To get the layout of earlier versions back:

```toml
[format]
indent = 2

[format.align]
scope = "contiguous"
semicolon = false
```

`block_boundary` takes:

- `"blank-line"`: a blank line starts a new block.
- `"comment"`: a blank line or a full-line comment starts a new block.
- `"never"`: blank lines and comments do not end a block; any other line without the aligned
  token does. Use this to align a whole port list split into commented groups.

```vhdl
-- conditional_continuation = true, when_clause = true
y <= a when sel = '1' else
     b when sel2 = '1' else
     c;

-- conditional_continuation = false
y <= a when sel = '1' else
  b    when sel2 = '1' else
  c;
```

## `[lint]`

| Key | Type | Default | Meaning |
|-----|------|---------|---------|
| `preset` | string | `"recommended"` | The starting level of every rule: `"none"` (only `syntax/error`), `"recommended"` (likely bugs) or `"strict"` (everything except `naming/*`). The level of each rule in each preset is in [lints.md](lints.md). |
| `rules` | table | `{}` | Per-rule settings, keyed by rule id; see below. |
| `waiver` | array of tables | `[]` | Findings silenced from the config instead of the sources; see [`[[lint.waiver]]`](#lintwaiver). |
| `baseline` | string | unset | A baseline file, relative to the project root, written by `vfl lint --write-baseline`: `vfl lint` does not report the findings it lists (see [cli.md](cli.md#baselines)). `--baseline` overrides it. |

### `[lint.rules]`

Each entry is either a level, or a table with an optional `level` and rule options:

```toml
[lint.rules]
"sema/unused-port" = "off"                     # a level: "off" | "info" | "warn" | "error"
"style/line-length" = { level = "error", max = 100 }
"naming/signal" = { prefix = "s_", case = "snake" }   # options alone turn a rule on at warn
```

- A level replaces the preset's level for that rule.
- A table with options and no `level` keeps the preset's level, or uses `warn` if the preset
  has the rule off. This is how `naming/*` rules are turned on.
- Rules with options: `style/line-length` (`max`), `sema/unassociated-port` (`undeclared`,
  for its quick fix), `sema/missing-branch-assignment` (`clocked`, to check clocked
  processes too) and every `naming/<kind>` rule (`prefix`, `suffix`, `case`, `pattern`). See [lints.md](lints.md) for what they do.

`info` findings are shown as notes. `vfl lint` exits with a failure status when there are
`error` findings, or `warn` findings too with `--deny-warnings`.

### `[[lint.waiver]]`

A waiver silences the findings that match every key it sets, like a `-- vfl-ignore`
comment but written outside the sources: for generated or vendor files, or to review
exceptions in one place. Waivers apply on the command line and in editors.

| Key | Type | Default | Meaning |
|-----|------|---------|---------|
| `files` | array of strings | `[]` (every file) | Gitignore-style globs, relative to the project root: `src/legacy/**`, `*_tb.vhd` (a pattern without `/` matches at any depth), `ip/` (a directory and what is under it). |
| `rules` | array of strings | `[]` (every rule) | Rule ids or categories (`naming`, `style/*`). |
| `lines` | array | `[]` (every line) | Lines the finding starts on: numbers (`12`) or inclusive ranges (`"40:52"`), one-based. |
| `message` | string | unset | A regular expression ([Rust `regex` syntax](https://docs.rs/regex/latest/regex/#syntax)) that must match part of the message. |
| `reason` | string | unset | Why the findings are waived. Not used by vfl; for the reader. |

```toml
[[lint.waiver]]
files = ["ip/vendor/**"]
reason = "vendor code, not ours to change"

[[lint.waiver]]
files = ["src/top.vhd"]
rules = ["sema/unused-signal"]
message = "`spare_"
reason = "spare signals kept for the next board revision"

[[lint.waiver]]
files = ["src/legacy_fifo.vhd"]
rules = ["style/line-length"]
lines = ["120:180"]
```

An invalid waiver (a bad glob, regular expression or line range) is reported as a
configuration warning and ignored. Like every array, the `waiver` list of a nested config
replaces the outer one's (see [Lookup](#lookup)); a shared list can come from a file
that the configs [`extends`](#special-top-level-keys).

## `[doc]`

`vfl doc [path]` writes the documentation of the project: an index with the design
hierarchy, the block diagram of the top level and the list of entities and packages, a page per entity (description, block
symbol, generics, ports, who instantiates it, and per architecture its instances, processes,
state machines, signals, constants, types and subprograms) and a page per package. Options
on the command line (`--out`, `--format`, `--title`) override these keys. `[doc]` is read
from the configs that apply to the project root.

| Key | Type | Default | Meaning |
|-----|------|---------|---------|
| `out_dir` | string | `"doc"` | Where `vfl doc` writes, relative to the project root. Existing files there are overwritten, others are kept. |
| `format` | string | `"html"` | `"html"`: self-contained pages with the diagrams inline, light and dark. `"markdown"`: `.md` pages, with the diagrams as `.svg` files next to them (or Mermaid blocks, see `[doc.diagrams]`). `"asciidoc"`: `.adoc` pages for Asciidoctor (or Antora), with the diagrams as `.svg` files next to them. |
| `title` | string | unset | The title of the index page. Unset: the name of the project directory. |
| `comments` | string | `"auto"` | Which comments document a declaration: `"marked"` (only comments starting with `--` and `comment_marker`, as in TerosHDL and Doxygen), `"all"` (every comment), `"auto"` (marked comments in files that have any, all comments in the others) or `"none"`. |
| `comment_marker` | string | `"!"` | What follows `--` in a marked comment: `"!"` for `--!`. |
| `exclude` | array of strings | `[]` | Gitignore-style globs, relative to the project root, of files whose units are left out of the documentation (their entities still count when resolving instances). |
| `testbenches` | boolean | `true` | Document entities without ports. |
| `signals` | boolean | `true` | List the signals of architectures and packages. |
| `constants` | boolean | `true` | List constants. |
| `types` | boolean | `true` | List types and subtypes. |
| `subprograms` | boolean | `true` | List functions and procedures. |
| `processes` | boolean | `true` | List processes, with their sensitivity list, clock and the signals they drive. |
| `instances` | boolean | `true` | List the instances of each architecture. |
| `fsm` | boolean | `true` | Detect state machines and list their states and transitions. |

A comment documents the declaration, port, generic, process or instance directly below it
(no blank line between), or the one it ends the line of. Comment lines are joined, and a
blank comment line starts a new paragraph. Separator lines (`-----`) and `vfl-ignore` and
`vfl:` directives are skipped.

```vhdl
--! A FIFO with a configurable depth.
--!
--! Writes to a full FIFO are dropped.
entity fifo is
  generic (
    DEPTH : positive := 16  --! Number of words
  );
  port (
    clk : in std_logic;     --! Rising edge clock
    --! Synchronous, active high
    rst : in std_logic;
```

A state machine is a `case` on a signal or variable of an enumeration type (or on an element
of a record, such as `engine.state`) whose alternatives assign literals of that type to it, or
to a next-state object of the same type. Both one-process and two-process machines are found,
whether the process is clocked with a synchronous or an asynchronous reset, combinational
(the next-state logic of a two-process machine) or written with `wait` statements; so are
next-state logic in concurrent conditional (`next <= A when c else B`) and selected
(`with state select`) assignments, and `if state = S` tests outside of a `case`. A
transition's condition is made of the `if` conditions around the assignment. The initial state
is the literal the reset branch assigns, else the declaration's default.

[examples/uart](../examples/uart) is a small design with its generated documentation in
`examples/uart/doc`.

### `[doc.diagrams]`

`vfl schema <kind> [entity]` draws one of these diagrams alone, as SVG (the default), or for
`hierarchy` and `fsm` as Mermaid (`-f mermaid`) or Graphviz DOT (`-f dot`). The kinds are
`symbol`, `hierarchy`, `block`, `port-map` (with `--instance label`), `fsm`, `dataflow`,
`clocks`, `resets`, `dependencies` (no entity) and `wave` (the argument is a WaveJSON file).

A doc comment can hold a timing diagram as a fenced block in the
[WaveDrom](https://wavedrom.com/tutorial.html) WaveJSON syntax, drawn by vfl itself:

````vhdl
--! ```wavedrom
--! { signal: [
--!   { name: "valid", wave: "010.." },
--!   { name: "data",  wave: "x=x..", data: ["byte"] }
--! ] }
--! ```
````

Lanes, groups, spacers, `head` and `foot` text and `config.hscale` are drawn; the wave
characters are `0 1 l h L H u d` (levels), `p n P N` (clocks), `x` (unknown), `z`, `= 2-9`
(data), `.` (repeat) and `|` (gap). Edges, `node` arrows and skins are not drawn.

| Key | Type | Default | Meaning |
|-----|------|---------|---------|
| `symbol` | boolean | `true` | Draw each entity (and each component of a package) as a block: inputs on the left, outputs and bidirectional ports on the right. |
| `hierarchy` | boolean | `true` | Draw the instantiation tree on the index (and the instances of each architecture when `block` is off). A subtree already drawn higher up is not drawn again. |
| `block` | boolean | `true` | Draw each architecture with instances as a block diagram: its instances as blocks, wired to each other and to the ports of the entity by the signals of their port maps. Signals between neighbouring columns are drawn as wires, longer ones as named stubs. Architectures with more than 80 instances are left to the instance table. |
| `top` | boolean | `true` | Draw the top level on the index: the block diagram (or, without instances, the symbol) of each entity at the root of the hierarchy, or for a testbench of the entities it instantiates. |
| `port_map` | boolean | `true` | Draw each instance of an architecture as the block it instantiates, with the actual connected to each port (`open` and defaulted inputs muted) and its generic map. |
| `dataflow` | boolean | `true` | Draw each architecture with processes or concurrent assignments as its processes, assignments and instances, wired by the signals they read and drive, with each process's clock and reset. |
| `clocks` | boolean | `true` | For each architecture with two or more clocks, draw its clock domains (processes and instances grouped by clock, through combinational logic) and the signals crossing them, also listed in a table. |
| `resets` | boolean | `true` | Draw a reset tree on the index for each top-level entity: where each reset port goes, through assignments and port maps, down to the processes, with their kind (synchronous or asynchronous, active high or low). |
| `dependencies` | boolean | `true` | Draw on the index which entities and packages use which packages of the project. |
| `waveforms` | boolean | `true` | Draw the ```` ```wavedrom ```` blocks of doc comments as timing diagrams (else they stay code blocks). |
| `fields` | boolean | `true` | Draw the bit layout of each record type whose field widths are known (from their subtypes, constants and generics), and of each signal with aliases of its slices. |
| `fsm` | boolean | `true` | Draw a state diagram per state machine. Past ten labelled transitions, the conditions are left to the transition table. |
| `port_types` | boolean | `true` | Write the port types next to the pins of a symbol. |
| `generics` | boolean | `true` | Write the generics in a symbol. |
| `markdown` | string | `"svg"` | How Markdown pages draw the hierarchy and state diagrams: `"svg"` files, or `"mermaid"` blocks, which GitHub and GitLab render. Symbols are always SVG. |

## `[generate]`

Code generated from an entity: `vfl new`, and the commands of the editor integrations. The
text is laid out by the formatter, so `[format]` applies to it too.

| Key | Type | Default | Meaning |
|-----|------|---------|---------|
| `instance_label` | string | `"u_{entity}"` | Label of a generated instantiation. `{entity}` is replaced by the entity name. |
| `testbench_name` | string | `"{entity}_tb"` | Name of a testbench entity, and of its file (with the extension of the entity's file). |
| `testbench_architecture` | string | `"sim"` | Name of a testbench architecture. |
| `testbench_instance` | string | `"dut"` | Label of the design under test in a testbench. |
| `clock_period` | string | `"10 ns"` | Period of the clocks a testbench drives, a VHDL time expression. |
| `reset_cycles` | integer | `5` | Clock cycles a testbench holds the resets active before releasing them. |
| `clock_pattern` | string | see below | Regular expression for the input ports driven as clocks, when no architecture of the entity shows it with `rising_edge(clk)`, `falling_edge(clk)` or `clk'event`. |
| `reset_pattern` | string | see below | Regular expression for the input ports driven as resets. |
| `active_low_pattern` | string | see below | Regular expression for the resets that are active low, when no architecture compares them with `'0'` or `'1'`. |

The default patterns take `clk`, `sys_clk`, `clk100` and `clk_i` as clocks (not `clk_en`);
`rst`, `areset`, `rst_n`, `rstn` and `rst_i` as resets; and `rst_n`, `rstn`, `resetn` and
`rst_ni` as active low:

```toml
[generate]
clock_pattern = '(?i)^(.*_)?(clk|clock)\d*(_i|_in|_p)?$'
reset_pattern = '(?i)^(.*_)?(a|s)?(rst|reset)\d*(n|_n|_b|_i|_in|_ni|_n_i)?$'
active_low_pattern = '(?i)([^i]n|_b|_ni|_n_i)$'
```

A testbench has an entity without ports, a constant for each generic (with a `TODO` where
the generic has no default), a signal for each port, the design under test, a clock for each
clock input, and a stimulus process that holds the resets, waits, and ends the simulation
(`std.env.finish` from VHDL-2008; with VHDL-93 the clocks stop and the simulation runs out
of events).

## `[verilog]`

Verilog and SystemVerilog sources. How much vfl does with them is set by `mode`; by
default, only the names VHDL needs:

| `mode` | What vfl does with Verilog and SystemVerilog files |
|--------|------------------------------------------------------|
| `"off"` | Ignores them: they are not read, checked, formatted or linted. |
| `"names"` (default) | Reads them only for the modules VHDL instantiates, so that `entity work.fifo` resolves and its port and generic maps are checked, hover and go to definition work from VHDL, and `vfl doc` documents the modules. Nothing in the Verilog files themselves is reported, formatted or colored. |
| `"full"` | Everything below: `vfl check`, `vfl fmt` and `vfl lint` work on them, and the editors format, lint, outline and color them. This support is in progress, so vfl says so once: `vfl` on standard error, the language server and the Notepad++ plugin in a message. |

With `mode = "full"`, `vfl check` parses them. In `vfl lint`, `vfl doc`
and the language server, their modules are entities VHDL can instantiate (`entity
work.fifo`, or through a component): the generic and port maps of an entity
instantiation are checked against the module's parameters and ports, and a module goes in the libraries of the units that
instantiate it, as an entity would (`[project.libraries]` patterns apply to Verilog
files too). `vfl fmt` formats them (see [Verilog and SystemVerilog](#verilog-and-systemverilog)
under `[format]`), and `vfl lint` and the language server lint them with the rules of
[lints.md](lints.md#verilog-and-systemverilog), names resolved across the project's
Verilog files and packages. The extension of a file decides how it is parsed: files with
`sv_extensions` as SystemVerilog, files with `extensions` as Verilog. A
`` `begin_keywords `` directive in a file still changes the reserved words for the part
of the file it covers.

Compiler directives are not run as a separate pass: `` `ifdef `` / `` `ifndef `` /
`` `elsif `` / `` `else `` pick the branch that is parsed, from the macros defined by
`defines`, by `` `define `` in the file and in the files it includes. Included files are
looked up in the including file's directory, then in `include_dirs`. Macro uses are not
expanded.

| Key | Type | Default | Meaning |
|-----|------|---------|---------|
| `mode` | string | `"names"` | How much of vfl works on Verilog and SystemVerilog files: `"off"`, `"names"` or `"full"` (see above). |
| `extensions` | array of strings | `["v", "vh"]` | File extensions, without the dot, of Verilog sources. |
| `standard` | string | `"1364-2005"` | The Verilog standard of those files: `"1364-1995"`, `"1364-2001"`, `"1364-2005"` (or `"1995"`, `"2001"`, `"2005"`), or a SystemVerilog one to parse `.v` files as SystemVerilog. |
| `sv_extensions` | array of strings | `["sv", "svh"]` | File extensions, without the dot, of SystemVerilog sources. |
| `sv_standard` | string | `"1800-2017"` | The SystemVerilog standard of those files: `"1800-2005"`, `"1800-2009"`, `"1800-2012"`, `"1800-2017"`, `"1800-2023"` (or `"sv2005"` ... `"sv2023"`). |
| `include_dirs` | array of strings | `[]` | Directories, relative to the project root, searched for `` `include `` files. |
| `defines` | array of strings | `[]` | Macros defined before every file, as `"NAME"` or `"NAME=value"` (only whether a macro is defined matters so far). |

## In-source comments

These are not config keys, but they adjust settings for part of a file.

Lint suppression:

| Comment | Effect |
|---------|--------|
| `-- vfl-ignore rule-a, rule-b` at the end of a line | Silences those rules on that line. |
| `-- vfl-ignore rule-a` on a line of its own | Silences them on the next line of code. |
| `-- vfl-ignore-file rule-a` anywhere | Silences them in the whole file. |

Without rule ids, every rule is silenced. A category (`naming`, `style`, `sema`, `verilog`)
silences all its rules. Ids may be separated by commas or spaces, and the list may be followed
by another `--` comment explaining why. Verilog and SystemVerilog files write the same
comments with `//` (`// vfl-ignore sema/unused-signal // kept for the next board rev`).

```vhdl
signal spare : std_logic;  -- vfl-ignore sema/unused-signal -- kept for the next board rev
```

To silence findings without editing the sources, see [`[[lint.waiver]]`](#lintwaiver).

Formatter regions:

| Comment | Effect |
|---------|--------|
| `-- vfl: off` ... `-- vfl: on` | The lines between are kept exactly as written. |
| `-- vfl: skip` | The next statement or declaration is kept exactly as written, with the comment ending its last line. |

## Default configuration

`vfl config show` in a directory without config files prints the equivalent of:

```toml
[project]
standard = "2008"
extensions = ["vhd", "vhdl"]
exclude = []

[project.libraries]

[format]
style = "default"
max_width = 100
indent = 4
# continuation_indent = 4   # unset: follows `indent`
use_tabs = false
keyword_case = "lower"
identifier_case = "preserve"
end_label = "preserve"
end_keyword = "preserve"
space_before_label_colon = true
space_after_keyword = true
max_blank_lines = 2
interface_list = "expand"
association_list = "auto"
close_paren = "own-line"
interface_separator = "trailing"
association_separator = "trailing"
case_alternative = "preserve"
line_ending = "auto"
on_type = true

[format.align]
port_colon = true
port_mode = true
generic_colon = true
interface_default = true
assignment = true
association = true
declaration = true
case_choice = true
when_clause = true
conditional_continuation = true
trailing_comment = true
semicolon = true
scope = "block"
block_boundary = "blank-line"

[lint]
preset = "recommended"

[lint.rules]

[doc]
out_dir = "doc"
format = "html"
# title = "..."   # unset: the project directory's name
comments = "auto"
comment_marker = "!"
exclude = []
testbenches = true
signals = true
constants = true
types = true
subprograms = true
processes = true
instances = true
fsm = true

[doc.diagrams]
symbol = true
hierarchy = true
block = true
top = true
port_map = true
dataflow = true
clocks = true
resets = true
dependencies = true
waveforms = true
fields = true
fsm = true
port_types = true
generics = true
markdown = "svg"

[generate]
instance_label = "u_{entity}"
testbench_name = "{entity}_tb"
testbench_architecture = "sim"
testbench_instance = "dut"
clock_period = "10 ns"
reset_cycles = 5
# clock_pattern, reset_pattern, active_low_pattern: see above

[verilog]
mode = "names"
extensions = ["v", "vh"]
standard = "1364-2005"
sv_extensions = ["sv", "svh"]
sv_standard = "1800-2017"
include_dirs = []
defines = []
```

Editor settings (the path to `vfl`, tracing, and the lint preset and format style of
projects without a config file) belong to the editor integration, not to `vfl.toml`; see the [VS Code extension](../editors/vscode/README.md) and the
[Notepad++ plugin](../editors/notepadpp/README.md).

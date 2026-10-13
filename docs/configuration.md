# Configuration reference

vfl reads its settings from `vfl.toml` (or `.vfl.toml`). Every key has a default, so the file
is optional, and an empty file is the same as no file. This page lists every key. The lint
rules that `[lint.rules]` configures are described in [lints.md](lints.md).

`vfl config show [path]` prints the effective configuration for a file or directory, with
the config files it was built from.

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

### `[project.libraries]`

Normally not needed: vfl puts each file in a library by itself. In order of priority:

1. `[project.libraries]` in `vfl.toml`.
2. rust_hdl's `vhdl_ls.toml` (or `.vhdl_ls.toml`) in the project root, with `$VAR`
   expansion.
3. Library lists of other tools found in the tree, read without running them: VUnit
   `run.py`, FuseSoC `.core`, Vivado `.xpr` and Tcl `read_vhdl -library` scripts, Quartus
   `.qsf` / `.qip`.
4. Inference from the code: a unit referenced as `lib.unit` puts the file that declares it
   in `lib`, `work.unit` references keep files together, and files nothing names stay in
   `work`.

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

## `[format]`

Settings of `vfl fmt` and of formatting in the language server.

| Key | Type | Default | Meaning |
|-----|------|---------|---------|
| `max_width` | integer | `100` | Lines longer than this get their parenthesized lists broken up, one element per line. |
| `indent` | integer | `2` | Columns per indentation level. Also the tab width with `use_tabs`. |
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
| `case_alternative` | string | `"preserve"` | `"preserve"` keeps a first statement written on the `when ... =>` line there; `"break"` starts every statement of a case alternative on its own line. |
| `line_ending` | string | `"auto"` | `"auto"` keeps the line endings the file uses (LF for new files), `"lf"` or `"crlf"` converts. |

`end_label` and `end_keyword` apply to entities, architectures, packages, package bodies,
contexts, configurations, components, subprogram bodies, processes, blocks, `if`, `case`,
loops, generate statements, records, physical types and protected types. A clause holding
a comment is left alone.

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

## `[format.align]`

Which columns the formatter lines up across consecutive lines of the same shape. All are on
by default.

| Key | Type | Default | Aligns |
|-----|------|---------|--------|
| `port_colon` | boolean | `true` | `:` in port clauses and subprogram parameter lists. |
| `port_mode` | boolean | `true` | The subtype after the mode (`in`, `out`, ...) in port clauses and parameter lists: `clk : in  std_logic;` / `q : out std_logic;`. |
| `generic_colon` | boolean | `true` | `:` in generic clauses. |
| `interface_default` | boolean | `true` | `:=` default values in generic and port clauses. |
| `assignment` | boolean | `true` | `<=` and `:=` in consecutive statements. |
| `association` | boolean | `true` | `=>` in port maps, generic maps and aggregates laid out one element per line. |
| `declaration` | boolean | `true` | `:` and `:=` in object declarations and record elements, and `is` in type and subtype declarations. |
| `case_choice` | boolean | `true` | `=>` of case alternatives whose statement is on the same line. |
| `when_clause` | boolean | `true` | `when` in conditional and selected assignments. |
| `conditional_continuation` | boolean | `true` | Continuation lines of conditional and selected assignments start under the first value instead of one indentation level in. |
| `trailing_comment` | boolean | `true` | Comments at the end of consecutive lines. |
| `block_boundary` | string | `"blank-line"` | What ends a run of aligned lines; see below. |

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
- Rules with options: `style/line-length` (`max`) and every `naming/<kind>` rule (`prefix`,
  `suffix`, `case`, `pattern`). See [lints.md](lints.md) for what they do.

`info` findings are shown as notes. `vfl lint` exits with a failure status when there are
`error` findings, or `warn` findings too with `--deny-warnings`.

## `[doc]`

`vfl doc [path]` writes the documentation of the project: an index with the design
hierarchy and the list of entities and packages, a page per entity (description, block
symbol, generics, ports, who instantiates it, and per architecture its instances, processes,
state machines, signals, constants, types and subprograms) and a page per package. Options
on the command line (`--out`, `--format`, `--title`) override these keys. `[doc]` is read
from the configs that apply to the project root.

| Key | Type | Default | Meaning |
|-----|------|---------|---------|
| `out_dir` | string | `"doc"` | Where `vfl doc` writes, relative to the project root. Existing files there are overwritten, others are kept. |
| `format` | string | `"html"` | `"html"`: self-contained pages with the diagrams inline, light and dark. `"markdown"`: `.md` pages, with the diagrams as `.svg` files next to them (or Mermaid blocks, see `[doc.diagrams]`). |
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

`vfl schema symbol|hierarchy|block|fsm [entity]` draws one of these diagrams alone, as SVG
(the default), or for `hierarchy` and `fsm` as Mermaid (`-f mermaid`) or Graphviz DOT
(`-f dot`).

| Key | Type | Default | Meaning |
|-----|------|---------|---------|
| `symbol` | boolean | `true` | Draw each entity (and each component of a package) as a block: inputs on the left, outputs and bidirectional ports on the right. |
| `hierarchy` | boolean | `true` | Draw the instantiation tree on the index (and the instances of each architecture when `block` is off). A subtree already drawn higher up is not drawn again. |
| `block` | boolean | `true` | Draw each architecture with instances as a block diagram: its instances as blocks, wired to each other and to the ports of the entity by the signals of their port maps. Signals between neighbouring columns are drawn as wires, longer ones as named stubs. Architectures with more than 80 instances are left to the instance table. |
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

## In-source comments

These are not config keys, but they adjust settings for part of a file.

Lint suppression:

| Comment | Effect |
|---------|--------|
| `-- vfl-ignore rule-a, rule-b` at the end of a line | Silences those rules on that line. |
| `-- vfl-ignore rule-a` on a line of its own | Silences them on the next line of code. |
| `-- vfl-ignore-file rule-a` anywhere | Silences them in the whole file. |

Without rule ids, every rule is silenced. A category (`naming`, `style`, `sema`) silences all
its rules. Ids may be separated by commas or spaces, and the list may be followed by another
`--` comment explaining why.

```vhdl
signal spare : std_logic;  -- vfl-ignore sema/unused-signal -- kept for the next board rev
```

Formatter regions:

| Comment | Effect |
|---------|--------|
| `-- vfl: off` ... `-- vfl: on` | The lines between are kept exactly as written. |
| `-- vfl: skip` | The next statement or declaration is kept exactly as written. |

## Default configuration

`vfl config show` in a directory without config files prints the equivalent of:

```toml
[project]
standard = "2008"
extensions = ["vhd", "vhdl"]
exclude = []

[project.libraries]

[format]
max_width = 100
indent = 2
# continuation_indent = 2   # unset: follows `indent`
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
case_alternative = "preserve"
line_ending = "auto"

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
```

Editor settings (the path to `vfl`, tracing) belong to the editor integration, not to
`vfl.toml`; see the [VS Code extension](../editors/vscode/README.md) and the
[Notepad++ plugin](../editors/notepadpp/README.md).

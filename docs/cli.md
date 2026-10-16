# Command-line reference

Every command of the `vfl` binary, its arguments and its options. `vfl <command> --help`
prints the same.

Commands that take paths default to the project around the current directory. Its root
is the outermost directory holding a `vfl.toml`, without going past one with `root = true`
or above the nearest version-control root (a git submodule counts as part of its
superproject); without a config, the nearest version-control
root; failing that, the current directory. Settings come from the `vfl.toml` files that
apply to each file ([configuration.md](configuration.md#lookup)).

| Command | Does |
|---------|------|
| [`vfl fmt`](#vfl-fmt) | Format files in place, check that they are formatted, or format standard input |
| [`vfl lint`](#vfl-lint) | Lint, with names resolved across the whole project, and apply fixes |
| [`vfl check`](#vfl-check) | Report syntax errors only |
| [`vfl rules`](#vfl-rules) | List the lint rules and their level in each preset |
| [`vfl project`](#vfl-project) | Show the project root, its sources and their libraries |
| [`vfl order`](#vfl-order) | Print the files in compile order, as a list, JSON or a GHDL, Questa or Vivado script |
| [`vfl config show`](#vfl-config-show) | Show the effective configuration and where each value came from |
| [`vfl doc`](#vfl-doc) | Write the documentation of the design, with diagrams |
| [`vfl spec`](#vfl-spec) | Write the design specification, from the code and its spec comments |
| [`vfl schema`](#vfl-schema) | Draw one diagram |
| [`vfl new`](#vfl-new) | Generate a testbench, an instantiation or a component declaration |
| [`vfl lsp`](#vfl-lsp) | Run the language server over standard input and output |

## Exit status

`0` when the command succeeded and found nothing to report; `1` when it found problems
(syntax errors, lint findings at the failing level, files that `fmt --check` would change)
or failed (an unreadable file, an invalid `vfl.toml`, an unknown entity), with the error
on standard error.

## `vfl fmt`

```sh
vfl fmt [OPTIONS] [PATHS]...
```

Formats VHDL, Verilog and SystemVerilog files in place with the `[format]` settings
([configuration.md](configuration.md#format)). Paths are files or directories; `-` reads
standard input and writes the result to standard output. A file is formatted as Verilog
or SystemVerilog by its extension, with `[verilog]` `mode = "full"` only (see
[`[verilog]`](configuration.md#verilog)); otherwise Verilog files are left as they are.

| Option | |
|--------|---|
| `--check` | Do not write anything; list the files that would change and exit with `1` if any would. |
| `--diff` | Do not write anything; print a unified diff of the changes (with `a/` and `b/` prefixes, as `git apply` takes it) and exit with `1` if there are any. Works with `-` too. |
| `--stdin-filename PATH` | With `-`: the path of the text read, which decides its language and the `vfl.toml` that applies. Standard input is VHDL otherwise. |
| `--lines FIRST:LAST` | Format only lines `FIRST` to `LAST` (1-based, inclusive), and the whole statements and aligned lines they are part of. Repeatable. Takes a single file or `-`. |

Formatting is idempotent and keeps every token and comment. Regions between
`-- vfl: off` and `-- vfl: on` (`// vfl: off` and `// vfl: on` in Verilog), and the
statement after `-- vfl: skip`, are left as they are.

```sh
vfl fmt                       # the whole project
vfl fmt --check src/          # in CI
vfl fmt --diff                # in CI, showing what to change
vfl fmt - < top.vhd           # from an editor
vfl fmt --lines 40:52 top.vhd # one block
vfl fmt - --stdin-filename fifo.sv < fifo.sv
```

## `vfl lint`

```sh
vfl lint [OPTIONS] [PATHS]...
```

Analyzes the whole project around each path, so names, types and overloads are resolved
across files and libraries, then reports the findings in the given files only. Unless
`[verilog]` `mode` is off (it is `"names"` by default), the project's Verilog and SystemVerilog modules
take part as entities: instances of them are checked against their parameters and ports.
With `mode = "full"`, Verilog and SystemVerilog files are linted too (see
[lints.md](lints.md#verilog-and-systemverilog)). FPGA vendor files under the paths
(Quartus `.qsf` and `.qip`, `.sdc`, Vivado `.xdc` and `.tcl`) are linted with the
[`vendor/*` rules](lints.md#vendor), with the ports of the analyzed design. Which rules
run and at which level comes from `[lint]` ([configuration.md](configuration.md#lint));
every rule is described in [lints.md](lints.md).

| Option | |
|--------|---|
| `--fix` | Apply the safe fixes, then report what is left. |
| `--unsafe-fixes` | With `--fix`, also apply fixes that may change behavior. |
| `--fix-only RULES` | Apply the fixes of these rules only: a comma-separated list of rule ids or categories (`style`, `style/*`). Implies `--fix`. |
| `--output-format FORMAT` | `human` (default: source excerpts), `concise` (one line per finding), `json`, `sarif` (for code scanning dashboards), `github` (annotations in GitHub Actions), `junit` (JUnit XML: a test suite per file, a failed test case per finding) or `gitlab` (a GitLab Code Quality report). |
| `--deny-warnings` | Exit with `1` on warnings too, not only on errors. |
| `--color WHEN` | `auto` (default: when printing to a terminal), `always` or `never`. |
| `--baseline FILE` | Do not report the findings listed in this baseline file (default: [`lint.baseline`](configuration.md#lint)). See [Baselines](#baselines). |
| `--write-baseline FILE` | Write every finding to this baseline file instead of reporting them, and exit with `0`. |
| `--changed-since REF` | Report only the files changed since the git revision `REF`: committed, staged, unstaged or untracked. The whole project is still analyzed, so names resolve as usual. |
| `--changed-lines` | With `--changed-since`: report only the findings that start on an added or changed line. |

```sh
vfl lint                                  # the whole project
vfl lint --fix src/uart                   # fix one directory
vfl lint --output-format github           # in a GitHub Actions step
vfl lint --output-format sarif > vfl.sarif
vfl lint --output-format junit > vfl-junit.xml
vfl lint --output-format gitlab > gl-code-quality-report.json
vfl lint --fix-only style/case-others,naming
vfl lint --changed-since origin/main --changed-lines   # in a pull request
```

A finding can be silenced with `-- vfl-ignore rule-id` at the end of its line or on the
line above, or for a whole file with `-- vfl-ignore-file rule-id`. Without touching the
sources, [`[[lint.waiver]]`](configuration.md#lintwaiver) entries in `vfl.toml` silence
findings by file, rule, line and message; editors apply them too.

### Baselines

To adopt vfl, or a stricter preset, on a code base with many findings, record them once
and fail only on new ones:

```sh
vfl lint --write-baseline vfl-baseline.json   # commit this file
vfl lint --baseline vfl-baseline.json         # or set lint.baseline in vfl.toml
```

The baseline lists each finding with a fingerprint of its file (relative to the project
root), its rule, its message without digits and the text of its line, but not its line
number: adding or removing lines elsewhere in a file does not make its findings new.
Editing the line of a finding does. Each entry matches one finding, so a second identical
finding in the same file is new. The summary counts the findings the baseline matched
(`2 in the baseline`). Write the baseline again to drop the findings that were fixed.
Editors do not read the baseline.

`--changed-since` also limits the report to what a branch changed. It runs `git` in the
project root.

## `vfl check`

```sh
vfl check [PATHS]...
```

Parses the files and reports syntax errors only, without analyzing the project. Faster
than `vfl lint` when only the syntax matters.

With `[verilog]` `mode = "full"` (see [`[verilog]`](configuration.md#verilog)), Verilog
and SystemVerilog files are checked too: by default `.v` and `.vh` files as
Verilog-2005 and `.sv` and `.svh` files as SystemVerilog-2017 (see
[`[verilog]`](configuration.md#verilog)). `` `ifdef `` branches are chosen from the
configured `defines` and the macros defined in the file and in the files it includes;
the branches not compiled are not checked.

## `vfl rules`

```sh
vfl rules [--explain RULE]
```

Prints every lint rule with its level in the `recommended` and `strict` presets, whether
it has a fix, and a one-line summary. `--explain RULE` prints one rule's levels and its
full description from [lints.md](lints.md): what it reports, its options and its fix.

```sh
vfl rules --explain style/line-length
```

## `vfl project`

```sh
vfl project [--vhdl-ls] [PATH]
```

Shows what vfl discovers for the project around `PATH` (default: the current directory):
its root, the config files that apply, every source file, the library each file is in and
why (a `[project.libraries]` entry, a `vhdl_ls.toml`, a tool file such as VUnit's `run.py`,
a FuseSoC `.core` or Edalize EDAM file, a Vivado, Quartus or Questa project, a `.f` file
list, an hdlmake `Manifest.py`, or inference from `library.unit` references). Verilog and SystemVerilog files (unless `[verilog]` `mode`
is off) are listed apart, with the libraries of the units that instantiate their modules.

| Option | |
|--------|---|
| `--vhdl-ls` | Print the libraries as a rust_hdl `vhdl_ls.toml` instead. |

## `vfl order`

```sh
vfl order [OPTIONS] [PATH]
```

Prints the files of the project around `PATH` (default: the current directory) in compile
order: each file, in each of its libraries (the ones `vfl project` shows), comes after the
files declaring the units it uses (use clauses, `entity lib.e` instantiations, an
architecture's entity, a package body's package). A component instantiation's default
binding orders files too, unless that makes a cycle, since tools bind components at
elaboration. Among files free to go in any order, paths sort alphabetically, so the output
is stable. Verilog and SystemVerilog files are listed with the libraries of the units that
instantiate their modules.

Files that use each other's units (a package using a package that uses it) cannot be
compiled in any order: each such group is reported on standard error, the order is still
printed with the group in path order, and the command fails.

| Option | |
|--------|---|
| `-f`, `--format FORMAT` | `list` (default): `library path` per line. `json`: an object with `root`, `standard`, `libraries`, `files` (each with `path`, `library` and `language`) and `cycles`. `ghdl`: a shell script of `ghdl -a --std=.. --work=lib` commands (with `-fsynopsys` when a file uses `ieee.std_logic_arith` and its family; Verilog files are left as comments). `questa`: a `.do` script for Questa or ModelSim, `vlib`/`vmap` for each library then `vcom` and `vlog`. `vivado`: a Tcl script of `read_vhdl -library` and `read_verilog -library` commands. The VHDL standard is `[project]` `standard`. |
| `--top UNIT` | Keep only what this entity or configuration needs, as `name` or `library.name`: the files of the units it uses, transitively, with their architectures and package bodies. A package name works too. |
| `--absolute` | Write absolute paths. By default paths are relative to the project root, and the scripts are run from there. |
| `-o`, `--output FILE` | Write to `FILE` instead of standard output. |

```sh
vfl order --top tb_uart -f ghdl -o compile.sh && sh compile.sh && ghdl -e --std=08 tb_uart
vfl order -f questa -o compile.do      # then `vsim -c -do compile.do`
vfl order -f vivado -o sources.tcl     # then `source sources.tcl` in a Vivado Tcl shell
```

## `vfl config show`

```sh
vfl config show [PATH]
```

Prints the configuration that applies to `PATH` (a file or directory, default: the current
directory), as TOML, with every key, including defaults, and the files it was read from.

## `vfl doc`

```sh
vfl doc [OPTIONS] [PATH]
```

Writes the documentation of the design: an index, a page per entity (generics, ports,
symbol, architectures, instances, processes, state machines and their diagrams) and a page
per package, as HTML, Markdown or AsciiDoc. Unless `[verilog]` `mode` is off, Verilog and SystemVerilog modules get a page
too (parameters, ports, symbol, instances) and appear in the hierarchy. Doc comments are the comments directly above a
declaration or at the end of its line, or only the `--!` ones if the file has any. What is
documented and which diagrams are drawn comes from `[doc]` and `[doc.diagrams]`
([configuration.md](configuration.md#doc)). See [examples/uart/doc](../examples/uart/doc/index.md)
for an example.

| Option | |
|--------|---|
| `-o, --out DIR` | Output directory (default: `doc.out_dir`, relative to the project root). |
| `--format FORMAT` | `html`, `markdown` or `asciidoc` (default: `doc.format`). |
| `--title TITLE` | Title of the index page (default: `doc.title`, else the project directory's name). |
| `--page-for FILE` | Print the page of the first entity (else package) `FILE` declares instead of writing the whole documentation. Editors use it for their diagram views. |

## `vfl spec`

```sh
vfl spec [OPTIONS] [PATH]
```

Writes the design specification: an index of the units and of every requirement, with the
tests that verify it, and a page per entity in numbered sections (overview, interface,
clocks and resets, behavior, state machines, registers, sub-blocks, requirements). The
code gives each port its timing (a clock, a reset, registered or sampled on a clock, or
combinational, through the instances it is connected to), the clocks and resets with what
uses them, the registers each clocked process drives and the state machines. Spec comments
add the rest: `-- @spec text`, `-- @req ID: text` and, in testbenches,
`-- @verifies ID, ID: note`, each applying to the declaration or statement below it or to
the one whose line it ends. A package gets a page if it has spec comments. Spec comments are
left out of `vfl doc`. The [specification guide](spec.md) explains spec comments and the
pages in detail. Settings are under `[doc.spec]`
([configuration.md](configuration.md#docspec)); see
[examples/uart/spec](../examples/uart/spec/index.md) for an example.

Problems (a requirement without an ID or given twice, a test verifying an unknown
requirement) are printed as warnings and listed on the index.

| Option | |
|--------|---|
| `-o, --out DIR` | Output directory (default: `doc.spec.out_dir`, relative to the project root). |
| `--format FORMAT` | `html`, `markdown` or `asciidoc` (default: `doc.spec.format`, else `doc.format`). |
| `--title TITLE` | Title of the index page (default: `doc.spec.title`, else the documentation title followed by "specification"). |
| `--page-for FILE` | Print the page of the first entity (else package) `FILE` declares instead of the index. Editors use it to open the specification of the current file. |
| `--check` | Exit with `1` when there are problems. |

## `vfl schema`

```sh
vfl schema [OPTIONS] KIND [ENTITY]
```

Draws one diagram. `ENTITY` is `name` or `library.name`.

| Kind | Draws |
|------|-------|
| `symbol` | The block symbol of an entity, with its generics and ports. |
| `hierarchy` | The instantiation tree. `ENTITY` is the top (default: every entity no other one instantiates). |
| `block` | The instances of an entity's architecture, wired by their signals. |
| `port-map` | One instance of an architecture, with the actual of each port. |
| `fsm` | A state machine of an entity's architecture. |
| `dataflow` | The processes, assignments and instances of an architecture, wired by the signals they read and drive. |
| `clocks` | The clock domains of an architecture and the signals crossing them. |
| `resets` | Where each reset of an entity goes, down to the processes. |
| `dependencies` | Which units use which packages (`ENTITY` is not used). |
| `wave` | A WaveDrom timing diagram; the argument is a WaveJSON file. |

| Option | |
|--------|---|
| `--architecture NAME` | For `block`, `port-map`, `dataflow`, `clocks` and `resets` (default: the last architecture). |
| `--instance LABEL` | For `port-map` (default: the only instance). |
| `--signal NAME` | For `fsm`, the state signal (default: the entity's only state machine). |
| `-f, --format FORMAT` | `svg` (default), `mermaid` or `dot`. Only `hierarchy` and `fsm` have Mermaid and DOT. |
| `-o, --output FILE` | Write to `FILE` instead of standard output. |
| `--project PATH` | A directory or file of the project (default: the current directory). |

```sh
vfl schema symbol uart_tx -o uart_tx.svg
vfl schema fsm uart_rx --signal state -f mermaid
vfl schema hierarchy -f dot | dot -Tpng > hierarchy.png
```

## `vfl new`

```sh
vfl new [OPTIONS] KIND ENTITY
```

Generates code from an entity of the project. `ENTITY` is `name` or `library.name`.

| Kind | Generates |
|------|-----------|
| `tb` | A testbench: a signal for every port, the design under test, a clock for every clock input, and a stimulus process that holds the resets for `reset_cycles` clock cycles. Written to `<entity>_tb.vhd` next to the entity. |
| `instance` | An entity instantiation (`u_e : entity work.e port map (...)`), printed. |
| `component` | A component declaration, printed. |
| `component-instance` | A component instantiation (`u_e : e port map (...)`), printed. |

| Option | |
|--------|---|
| `-o, --output FILE` | Where to write the code (`-` for standard output). |
| `--force` | Overwrite the output file if it exists. |
| `--project PATH` | A directory of the project (default: the current directory). |

Labels, the testbench's name, architecture and instance, the clock period, the reset length
and how clock and reset ports are recognized come from `[generate]`
([configuration.md](configuration.md#generate)); the layout from `[format]`.

## `vfl lsp`

```sh
vfl lsp
```

Runs the language server over standard input and output, for any editor with an LSP
client. It offers:

- diagnostics as you type, from the syntax, the cross-file analysis and the lint rules;
- quick fixes and a fix-all source action (`source.fixAll.vfl`);
- formatting of the document, of a range, and on type (`;` and Enter, unless
  `format.on_type = false`);
- hover with types and resolved overloads, signature help, completion (including record
  elements, protected type methods, library units and package declarations);
- go to definition (also into the bundled `std` and `ieee` sources, and into the
  Verilog modules VHDL instantiates, whose declarations hover shows), find references,
  document highlights, rename (packages with their bodies, subprograms with their bodies,
  entities with their components, record elements; clashes refused), document and
  workspace symbols;
- go to implementation (an entity's architectures, the entity a component binds to, a
  package's body, a subprogram's or protected type's body), go to type definition (the type
  mark of an object, alias, record element or function result) and go to declaration (from
  a body to what it completes, from an architecture to its entity);
- inlay hints, worked out only for the range the editor shows: the formal name before
  each positional actual of a port map, generic map or subprogram call, and optionally the
  mode and type of each port map actual (`[lsp.inlay_hints]`,
  [configuration.md](configuration.md#lspinlay_hints));
- linked editing of a name and the `end` name that repeats it (entities, architectures,
  packages, components, subprogram bodies, record and protected types, labelled
  statements), and selection ranges from the syntax tree;
- an "N instances" code lens on each entity and component, counted when the editor shows
  it (`codeLens/resolve`); its command, `vfl.showReferences`, is the client's to run, with
  the document URI, the position and the instances' locations as arguments (the VS Code
  extension shows them in the references view);
- call hierarchy: the callers (subprograms, processes, units) and callees of a subprogram,
  and the instance tree, whose items are design entities: the architectures that
  instantiate an entity, and what an architecture instantiates;
- semantic highlighting: signals, ports (with their mode), variables, constants,
  generics, types, subprograms, entities, components, packages and enumeration literals;
- for Verilog and SystemVerilog files, with `[verilog]` `mode = "full"`: lint diagnostics with their fixes, formatting (document, range and on
  type), document symbols (modules, parameters, ports, declarations, functions, tasks,
  instances and named generate blocks), folding ranges, and semantic highlighting of the
  names declared in the file;
- code actions that insert an instantiation or a component declaration, or generate a
  testbench, from an entity;
- the design hierarchy, with the custom `vfl/hierarchy` request
  ([editors.md](editors.md#design-hierarchy));
- settings from the editor for projects without a config file: `vfl.toml` keys under
  `config` in the initialization options or in `workspace/didChangeConfiguration`
  ([editors.md](editors.md#editor-settings)), the kinds of inlay hints under `inlayHints`
  (which win over `vfl.toml`), and a `source.vfl.generateConfig` code action
  (with the `vfl/projectConfig` request) that writes the settings in effect to `vfl.toml`;
- errors in `vfl.toml` (and the files it `extends`) as diagnostics on the file, where the
  problem is, and once in a message. An invalid config file is left out until it is fixed:
  the files under it get the config of the valid files above it, or the editor's settings;
- progress while it loads the project (`$/progress`, with how many files are parsed), in
  editors that show it;
- files created, changed or deleted outside the editor (a `git checkout`, a generator)
  are picked up: through the editor's file watcher where it has one, read in one go
  however many arrive together, and otherwise through the server's own, which looks for
  changes every few seconds;
- files over `[project]` `max-file-size` (2 MB by default), generated netlists say, are
  outlined: only their entities and packages are analyzed, and they are not linted
  ([configuration.md](configuration.md#project)).

Setup for each editor is in [editors.md](editors.md).

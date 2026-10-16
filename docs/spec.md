# Design specifications

`vfl spec` writes a design specification from the VHDL code and from spec comments written
next to it. The code gives what a reader would otherwise copy by hand: the interface, the
timing of every port, the clocks and resets, the registers, the state machines and the
sub-blocks. The comments add what the code cannot say: intent, constraints and
requirements, which the testbenches then claim to verify. Since both live in the sources,
the specification stays true to the design as it changes.

[examples/uart/spec](../examples/uart/spec/index.md) is the specification of the example
design, generated from [examples/uart](../examples/uart).

## Quick start

1. Write requirements and notes next to the code they are about:

   ```vhdl
   --! Serializes bytes.
   --!
   --! @req TX-1: Idles high, then sends a start bit, the data bits LSB first and a stop bit.
   --! @req TX-2: Each bit lasts `CYCLES_PER_BIT` clock cycles.
   entity uart_tx is
     generic (
       CYCLES_PER_BIT : positive := 868
     );
     port (
       clk   : in  std_logic;
       rst_n : in  std_logic;
       data  : in  std_logic_vector(7 downto 0);
       valid : in  std_logic;
       --! @req TX-3: `ready` is high in the cycle `data` is taken.
       ready : out std_logic;
       txd   : out std_logic  -- @spec Driven from a register, never from logic.
     );
   end entity;
   ```

2. Say which test checks them, in the testbench:

   ```vhdl
     -- @verifies TX-1, TX-2: one frame looped back
     stimulus : process is
   ```

3. Run `vfl spec` at the root of the project (or anywhere in it). It prints the page
   written, `spec/index.html` by default:

   ```sh
   vfl spec                      # HTML in spec/
   vfl spec --format markdown    # Markdown, with the diagrams as .svg files
   vfl spec --check              # also fail on duplicate or unknown requirement IDs
   ```

From an editor: **vfl: Generate Specification** in VS Code (also in the explorer context
menu), **Plugins > vfl > Generate specification** in Notepad++, and the
`vfl: generate specification` task in Zed (`ctrl-alt-v s` with the example
[keymap](../editors/zed/keymap.json)). Each writes the specification of the project and
opens the page of the current file's entity.

## Spec comments

There are three tags. `[doc.spec]` can rename them (see [Configuration](#configuration)).

| Tag | Written as | Means |
|-----|------------|-------|
| `@spec` | `-- @spec text` | A note about the design: intent, a constraint, a timing diagram. |
| `@req` | `-- @req ID: text` | A requirement, with an ID that tests refer to. The `:` is optional. |
| `@verifies` | `-- @verifies ID, ID: note` | The test below checks these requirements. IDs are separated by commas or spaces; the note is optional. |

A tag works with or without the doc comment marker: `--! @req` and `-- @req` are the
same, and so is `/* @spec ... */`. The tag must start the comment.

**Several lines.** The comment lines below a tagged one continue it, up to a blank comment
line, another tag or code:

```vhdl
--! @spec The receiver samples each bit in its middle, so it tolerates a
--! clock mismatch of up to 4%.
--!
--! This line is an ordinary doc comment again.
```

A ```` ```wavedrom ```` block in a `@spec` comment is drawn as a timing diagram (blank lines
inside the fence do not end the comment), as in the [documentation](configuration.md#docdiagrams).

**What a comment applies to.** A spec comment on its own line applies to the declaration
or statement below it; one at the end of a line applies to the declaration or statement
on that line. Where it shows depends on that target:

| Target | Shown in |
|--------|----------|
| The entity, or comments before it | 1 Overview |
| A generic or port | 2 Interface, in the *Specification* column (requirements by ID) |
| An architecture | 4 Behavior, under the architecture |
| A state machine's signal, type or process | 5 State machines |
| A signal or port a clocked process drives | 6 Registers, in the *Specification* column |
| An instance | 7 Sub-blocks, in the *Specification* column |
| Anything else in an architecture (a process, an assignment, a constant...) | 4 Behavior, under a line naming it |
| A declaration of a package | The package's own page |

Every requirement is also listed in the last section, *Requirements*, with what it
applies to, where it is written and which tests verify it. When a section is left out
(see `sections` below), the comments it would have shown go to *Behavior*.

**Spec comments are not documentation.** `vfl doc` leaves them out of the doc comments,
so a `--! @req` line between doc comment lines does not end up in the documentation.

## What the specification contains

The index lists the units with their number of requirements, then every requirement of
the project with its unit and the tests that verify it, then the problems found.

Each entity's page has these sections, numbered in the order `sections` gives:

1. **Overview**: the entity's doc comment, its `@spec` notes and its requirements.
2. **Interface**: the symbol, the generics and the ports. Each port has a *Timing*:
   - `clock` for a clock, `reset (asynchronous, active low)` for a reset, with its clock
     when it is synchronous;
   - `registered on clk` for an output a clocked process drives, `sampled on clk` for an
     input one reads;
   - `combinational` when only logic drives or reads it.
   Ports wired to instances take the timing of the instance port, with the clock renamed
   to the signal connected to it, so a structural top level gets its timing from the
   blocks below it.
3. **Clocks and resets**: each clock with its edge and what uses it (processes, or
   instances for a clock that only goes down), each reset with its kind, and the clock
   domain diagram and crossings when there are several clocks.
4. **Behavior**: per architecture, its description, its processes (clocked, with which
   reset, combinational or waiting, and what they drive), and the spec comments about
   its statements and declarations.
5. **State machines**: each detected state machine with its type, process, initial state,
   states, diagram and transitions.
6. **Registers**: the signals and ports clocked processes drive, with their type, clock,
   reset and process.
7. **Sub-blocks**: the block diagram and the instances, linked to their own pages.
8. **Requirements**: the entity's requirements with what they apply to, their source line
   and the tests that verify them.

State machines, registers, sub-blocks and requirements are left out of a page that has none,
and the numbers follow. Testbenches
(entities without ports) get no page unless `testbenches = true`, but their `@verifies`
comments always count. A package gets a page when it has spec comments.

## Checking requirements

`vfl spec` warns about:

- a requirement without an ID;
- a requirement ID given twice;
- a `@verifies` comment with no ID, or naming a requirement that does not exist.

The warnings are printed on standard error and listed on the index. With `--check`, `vfl spec`
exits with `1` when there are any, which makes it usable in CI:

```sh
vfl spec --check -o "$RUNNER_TEMP/spec"
```

A requirement no test verifies is not a problem: its *Verified by* cell is just empty,
and the index counts how many are verified.

## Configuration

Settings are under `[doc.spec]` in `vfl.toml`; the
[configuration reference](configuration.md#docspec) describes each key. For example:

```toml
[doc]
format = "markdown"          # also the default format of the specification

[doc.spec]
out_dir = "docs/spec"
title = "Modem specification"
requirement_tag = "@requirement"
sections = ["overview", "interface", "requirements", "behavior"]
annotated_only = true        # only entities with spec comments get a page
```

The specification shares `comment_marker`, `exclude` and the diagram switches of
`[doc.diagrams]` with the documentation. The command line overrides the output
directory (`-o`), the format (`--format`) and the title (`--title`); see
[`vfl spec`](cli.md#vfl-spec).

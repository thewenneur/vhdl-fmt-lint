# Lint rules

`vhone lint` and the language server report findings from the rules below. `vhone rules` prints
the same list with each rule's level in each preset. Rules are configured in the `[lint]`
table of `vhone.toml`; see [configuration.md](configuration.md#lint).

- [Levels, presets and fixes](#levels-presets-and-fixes)
- [Summary](#summary)
- [`syntax/*`](#syntax): parse errors
- [`sema/*`](#sema): semantic checks across the whole project
- [`style/*`](#style): checks on the text and syntax of one file
- [`naming/*`](#naming): naming conventions

## Levels, presets and fixes

Every rule has a level: `off`, `info` (shown as a note), `warn` or `error`. `vhone lint` exits
with a failure status when there is an `error` finding, or a `warn` finding with
`--deny-warnings`.

The preset gives every rule its starting level:

- `none`: only `syntax/error`.
- `recommended` (the default): the rules that point at likely bugs.
- `strict`: every rule except `naming/*`, which are off until configured.

```toml
[lint]
preset = "strict"

[lint.rules]
"sema/unused-port" = "off"                      # change a level
"style/line-length" = { max = 100 }             # set options
"naming/signal" = { level = "error", prefix = "s_" }
```

Some rules can fix what they report. A fix is either safe (it cannot change behavior) or
unsafe (it can, so it is worth a look). `vhone lint --fix` applies the safe fixes and lints
again; `vhone lint --fix --unsafe-fixes` applies both. In an editor, each fix is offered as a
quick fix, and the `source.fixAll.vhone` code action applies the safe ones.

Editors offer more quick fixes than `--fix` applies: renaming a declaration everywhere to
follow its [naming convention](#naming), adding the `use` clause for an
[undeclared name](#semaunresolved-name) that a package declares, and, for every rule,
silencing it on the line or in the file with a `-- vhone-ignore` comment.

A finding can be silenced in the source with `-- vhone-ignore rule-id` at the end of its line
(or on the line above), or for a whole file with `-- vhone-ignore-file rule-id`; see
[In-source comments](configuration.md#in-source-comments).

## Summary

| Rule | recommended | strict | Fix |
|------|-------------|--------|-----|
| [`syntax/error`](#syntaxerror) | error | error | |
| [`sema/unresolved-name`](#semaunresolved-name) | error | error | |
| [`sema/unresolved-unit`](#semaunresolved-unit) | error | error | |
| [`sema/unknown-library`](#semaunknown-library) | info | info | |
| [`sema/duplicate-declaration`](#semaduplicate-declaration) | error | error | |
| [`sema/duplicate-unit`](#semaduplicate-unit) | warn | warn | |
| [`sema/unassociated-port`](#semaunassociated-port) | error | error | |
| [`sema/type-mismatch`](#sematype-mismatch) | error | error | |
| [`sema/no-matching-overload`](#semano-matching-overload) | error | error | |
| [`sema/unused-signal`](#semaunused-signal) | warn | warn | unsafe |
| [`sema/unused-variable`](#semaunused-variable) | warn | warn | unsafe |
| [`sema/unused-constant`](#semaunused-constant) | warn | warn | unsafe |
| [`sema/unused-port`](#semaunused-port) | off | warn | |
| [`sema/unused-generic`](#semaunused-generic) | off | warn | |
| [`sema/unused-use`](#semaunused-use) | off | warn | unsafe |
| [`sema/unread-signal`](#semaunread-signal) | warn | warn | |
| [`sema/undriven-signal`](#semaundriven-signal) | warn | warn | |
| [`sema/sensitivity-list`](#semasensitivity-list) | warn | warn | unsafe |
| [`sema/unneeded-sensitivity`](#semaunneeded-sensitivity) | off | warn | unsafe |
| [`sema/width-mismatch`](#semawidth-mismatch) | warn | error | |
| [`sema/inferred-latch`](#semainferred-latch) | warn | error | |
| [`sema/clock-domain-crossing`](#semaclock-domain-crossing) | info | warn | |
| [`sema/open-input`](#semaopen-input) | warn | error | |
| [`sema/inout-port`](#semainout-port) | info | warn | |
| [`sema/missing-reset`](#semamissing-reset) | info | warn | |
| [`sema/mixed-reset`](#semamixed-reset) | warn | warn | |
| [`sema/reset-polarity`](#semareset-polarity) | warn | error | |
| [`sema/fsm-unreachable-state`](#semafsm-unreachable-state) | warn | warn | |
| [`sema/fsm-stuck-state`](#semafsm-stuck-state) | info | warn | |
| [`style/no-buffer-ports`](#styleno-buffer-ports) | warn | warn | unsafe |
| [`style/no-std-logic-arith`](#styleno-std-logic-arith) | warn | warn | unsafe |
| [`style/line-length`](#styleline-length) | off | warn | |
| [`style/trailing-whitespace`](#styletrailing-whitespace) | off | warn | safe |
| [`style/tabs`](#styletabs) | off | warn | safe |
| [`style/process-label`](#styleprocess-label) | off | warn | |
| [`style/positional-association`](#stylepositional-association) | off | warn | safe |
| [`style/clock-edge`](#styleclock-edge) | off | warn | unsafe |
| [`style/file-name`](#stylefile-name) | off | warn | |
| [`naming/<kind>`](#naming) | off | off | |

## `syntax`

### `syntax/error`

The file does not parse: a token is missing or unexpected, or a construct is not allowed by
the configured `project.standard` (for example a VHDL-2008 feature in a VHDL-93 project).
The parser recovers, so the rest of the file is still analyzed. This rule is on in every
preset, `none` included, but can still be turned off.

## `sema`

These rules use the semantic model of the whole project: every file is parsed, put in its
library, and names are resolved across files. `vhone lint path` analyzes the project around
`path` but reports only the files under `path`.

The analysis is conservative: what it cannot know (a name from a library outside the
project, a generic type, an attribute it does not model) is treated as valid, so that valid
code gets no false errors.

### `sema/unresolved-name`

A name is not declared or not visible where it is used. This covers simple names, selected
names (`pkg.item`), a port or generic named in a map that the entity does not have, and
`exit` / `next` with a loop label that does not exist.

```vhdl
x <= undeclared;          -- `undeclared` is not declared
```

When a region `use`s something from a library outside the project (see
[`sema/unknown-library`](#semaunknown-library)), names that are not found there are not
reported, since they may come from that library.

In an editor, a quick fix adds the `use` clause of each package that declares the name, in
the project or in the bundled libraries (with its `library` clause when needed).

### `sema/unresolved-unit`

A design unit does not exist: an entity in `entity work.e`, a package in `use lib.pkg.all`,
an architecture in `entity work.e(arch)`, or the entity an architecture is `of`.

```vhdl
u3 : entity work.nothere;  -- no unit `nothere` in library `work`
```

If the unit exists but in another library, check that library's assignment with
`vhone project`.

### `sema/unknown-library`

A `library` clause names a library that no file of the project is in, so nothing from it is
checked. This is an `info` note: it often means a third-party library (OSVVM, UVVM, vendor
primitives) is outside the project root or excluded. `std`, `ieee` and `work` are always
known.

### `sema/duplicate-declaration`

Two declarations in the same declarative region have the same name, such as two signals of
an architecture or two ports of an entity. Overloaded subprograms and enumeration literals,
and a type completing an incomplete type declaration, may share a name.

```vhdl
signal s, s : std_logic;   -- `s` is already declared in this region
```

### `sema/duplicate-unit`

Two files declare a design unit with the same name in the same library. Only one can be
used, so the other is probably a stale copy. The finding names the other location. If both
are needed, put them in different libraries with `[project.libraries]`.

### `sema/unassociated-port`

An instantiation leaves out a port of mode `in` or a generic that has no default value.
Both must be associated (ports of other modes can be left out).

```vhdl
-- entity sub has `generic (N : natural)` without a default
u2 : entity work.sub port map (a => r, y => o2);  -- generic `N` of entity `sub` must be associated
```

### `sema/type-mismatch`

An expression does not have the type its context needs, for example the value of an
assignment, an initial value or an actual in a port map.

```vhdl
signal n : integer;
...
n <= '1';   -- expected `integer`, found the character literal '1'
```

### `sema/no-matching-overload`

A subprogram call or an operator has candidates, but none accepts the arguments.

```vhdl
v <= std_logic_vector(unsigned(v) + true);  -- no `+` operator for `UNRESOLVED_UNSIGNED` and `boolean`
```

`sema/type-mismatch` and `sema/no-matching-overload` come from overload resolution as the
LRM describes it, and are only reported when every candidate is ruled out for a definite
reason.

### `sema/unused-signal`

A signal is never referenced.

### `sema/unused-variable`

A variable is never referenced.

### `sema/unused-constant`

A constant declared in an architecture, process, subprogram or block is never referenced.

For these three rules, declarations directly in an entity or a package are not reported,
because other units can use them; neither are the members of protected types.

Fix (unsafe): removes the name from its declaration, or the whole declaration and its line
when it declares only that name. It is unsafe because the initial value is no longer
evaluated, which matters when it calls an impure function.

### `sema/unused-port`

A port is not referenced by its entity or by any architecture of it. Entities without an
architecture in the project are skipped. Off in `recommended`, because ports are often kept
for interface compatibility.

### `sema/unused-generic`

A generic is not referenced by its entity or by any architecture of it. Entities without an
architecture in the project are skipped. Off in `recommended`.

### `sema/unused-use`

Nothing a use clause makes visible is used by the unit, or by the secondary units
(architectures, package body) that inherit the clause. Clauses of context declarations are
not checked.

```vhdl
use ieee.math_real.all;   -- nothing from `ieee.math_real.all` is used
```

Fix (unsafe): removes the item, or the whole clause and its line when it is the only item.
It is unsafe because an unused clause can still change which declaration a name would hide
or reach.

### `sema/unread-signal`

A signal is assigned but never read. Either the logic driving it is dead, or a read is
missing.

### `sema/undriven-signal`

A signal is read but never assigned, so it keeps its initial value; or an output port (mode
`out` or `buffer`) is never assigned by any architecture of its entity.

```vhdl
signal r : std_logic;
...
u2 : entity work.sub generic map (N => 1) port map (a => r, y => o2);  -- `r` is read but never assigned
```

A signal assigned through a port map (`y => s`) counts as assigned.

### `sema/sensitivity-list`

A process with a sensitivity list reads a signal the list does not name, so simulation and
synthesis disagree.

- A combinational process needs every signal it reads.
- A clocked process, written as a single `if` whose branch tests a clock edge, needs the
  signals read in the conditions up to and including the edge test, and in the branches
  before it (an asynchronous reset). Signals read under the clock edge are sampled on it
  and need not be listed.

```vhdl
comb : process (sel)
begin
  if sel = '1' then
    y <= din(0);          -- the process reads `din` but its sensitivity list does not name it
  end if;
end process;
```

Processes with `process (all)` are not checked, and neither are clocked processes of another
shape. Fix (unsafe): appends the missing signals to the list.

### `sema/unneeded-sensitivity`

A sensitivity list names a signal the process never reads. Harmless for synthesis, but it
wakes the process in simulation for nothing. Off in `recommended`.

Fix (unsafe): removes the signal from the list. There is no fix when it is the only signal,
since a process without a sensitivity list and without a `wait` never stops.

### `sema/width-mismatch`

An assignment, an initial value, or a port or generic association gives a vector a value
with a different number of elements, when both sizes are known statically. Synthesis tools
usually reject this, and simulation fails at run time.

Sizes are known for objects declared with a static index constraint (`(7 downto 0)`,
`(WIDTH - 1 downto 0)` with a constant `WIDTH`), slices, string and bit-string literals
(`"101"`, `x"0F"`, `8x"FF"`) and concatenations (`&`) of these; constants with a static
integer value are followed.

```vhdl
signal narrow : std_logic_vector(3 downto 0) := "101";  -- `narrow` has 4 elements but the value has 3
dout <= din(3 downto 0);   -- `dout` has 8 elements but the value has 4
```

Only one-dimensional arrays of scalars (bit and logic vectors, `unsigned`, `signed`,
strings) are checked; aggregates, function results and anything whose size depends on a
generic or a run-time value are skipped. `warn` in `recommended`, `error` in `strict`.

### `sema/inferred-latch`

A combinational process assigns a signal on some paths but not all, so the signal has to
keep its value: synthesis infers a latch.

A process is combinational when it has a sensitivity list (`all` included) and no `wait`,
or when its only `wait` is a trailing `wait on a, b;`, and it tests no clock edge.

```vhdl
comb : process (all)
begin
  if sel = '1' then
    lat <= din(0);   -- `lat` is not assigned on every path through this combinational process
  end if;
end process;
```

To fix it, assign a default at the top of the process or add the missing `else` / `when
others`. Not reported: signals only assigned in part (an element or a slice), processes
with `after` delays (simulation models), other `wait` statements, and processes that do not
parse. `warn` in `recommended`, `error` in `strict`.

### `sema/clock-domain-crossing`

A signal registered on one clock is read in a process clocked by another, without a
synchronizer. Such reads can go metastable.

```vhdl
process (clk_a) begin
  if rising_edge(clk_a) then a_reg <= sel; end if;
end process;

process (clk_b) begin
  if rising_edge(clk_b) then
    b_reg <= a_reg and sel;   -- `a_reg` is registered on `clk_a` but used here on `clk_b` without a synchronizer
  end if;
end process;
```

Clocked processes are those that test `rising_edge(clk)`, `falling_edge(clk)` or
`clk'event`, within one architecture. Not reported: a plain copy into a register
(`sync1 <= a_reg;`, the first stage of a synchronizer), memories (arrays of arrays, such as
the two ports of a dual-clock RAM), signals registered on several clocks, and processes with
`wait` (testbenches). `info` in `recommended`, `warn` in `strict`.

### `sema/open-input`

An instantiation associates an input port with `open`, and the port has no default value,
so the input floats. VHDL requires an input port that is left open to have a default.

```vhdl
-- entity leaf has `port (a : in std_logic; ...)`
u1 : entity work.leaf port map (a => open, y => y);   -- input port `a` is left open and has no default value
```

Both named (`a => open`) and positional (`open`) associations are checked. Leaving an
input out of the map altogether is [`sema/unassociated-port`](#semaunassociated-port).
`warn` in `recommended`, `error` in `strict`.

### `sema/inout-port`

An `inout` port is only read, or only driven, by its entity and its architectures, so it
could be `in` or `out`.

```vhdl
port (pad : inout std_logic);   -- inout port `pad` is never driven, so it could be `in`
...
y <= pad;
```

An `inout` port passed to an instance's `inout` port counts as both read and driven.
Entities without an architecture in the project are skipped. `info` in `recommended`,
`warn` in `strict`.

### `sema/missing-reset`

A clocked process with a reset assigns a register that the reset branch does not. While
the reset is active, that register has to keep its value, so the reset becomes an enable
on it: extra logic, and a register that cannot use the flip-flop's reset.

```vhdl
process (clk, rst) begin
  if rst = '1' then
    count <= (others => '0');
  elsif rising_edge(clk) then
    count <= count + 1;
    data  <= din;   -- `data` is not reset by `rst` in this process, so the reset acts as an enable on it
  end if;
end process;
```

To fix it, reset the register too, or move it to a process without a reset. A reset is the
first condition of an `if` whose last branch is the clock edge (asynchronous), or the
first condition of an `if` that opens the clocked branch and tests a signal named like a
reset (`rst`, `reset`, `arst_n`, ...; synchronous). A synchronous reset written last in
the clocked branch, so that it overrides what comes before, does not affect the registers
it leaves out and is not reported. Not reported: memories (arrays of arrays), variables,
and processes with `wait`. `info` in `recommended`, `warn` in `strict`.

### `sema/mixed-reset`

A reset signal resets one process synchronously and another asynchronously, within one
architecture. The two kinds of register see the reset at different times, and a reset
used asynchronously should be synchronized on release.

```vhdl
a : process (clk, rst) begin
  if rst = '1' then x <= '0'; elsif rising_edge(clk) then x <= d; end if;
end process;
b : process (clk) begin
  if rising_edge(clk) then
    if rst = '1' then   -- `rst` resets this process synchronously but the process at line 1 asynchronously
      y <= '0'; else y <= x; end if;
  end if;
end process;
```

Processes inside `if` and `case` generate statements are not compared, since they often
pick one style or the other from a generic. `warn` in every preset.

### `sema/reset-polarity`

A reset signal is tested active high in one process and active low in another, within one
architecture; or its name says it is active low (`rst_n`, `resetn`, `nrst`, `rst_l`,
`rst_b`) but a process resets when it is high.

```vhdl
if rst = '1' then ...     -- in one process
if rst = '0' then ...     -- `rst` resets when low here but when high at line 1
if rst_n = '1' then ...   -- `rst_n` is named as active low but resets when high here
```

Resets are found as for [`sema/missing-reset`](#semamissing-reset). A level given by a
constant or generic (`rst = RESET_LEVEL`) is not compared. Processes inside `if` and `case`
generate statements are not compared with each other. `warn` in `recommended`, `error` in
`strict`.

### `sema/fsm-unreachable-state`

A state machine has a state that no transition reaches, from its initial value or reset.

A state machine is a `case` on a signal or variable of an enumeration type declared in the
project, whose alternatives assign literals of that type to it, or to a next-state object
of the same type that is then copied into it. Transitions are the literals assigned within
each alternative; a literal assigned outside the `case` (a default, an abort) can come from
any state, and one assigned in a reset branch is a starting state, as is the initial value
(or the first literal when there is none).

```vhdl
type state_t is (IDLE, RUN, SPARE);
...
case state is
  when IDLE  => if go = '1' then next_state <= RUN; end if;
  when RUN   => next_state <= IDLE;
  when SPARE => next_state <= IDLE;   -- no transition of `state` reaches state `SPARE` of `state_t`
end case;
```

The state machine is left alone when the state is assigned anything but a literal or the
next-state object (another signal, `state_t'succ(state)`, a function), assigned through a
procedure, an aggregate or a port map, or when a choice is a range. Only states named by a
choice are reported. `warn` in every preset.

### `sema/fsm-stuck-state`

A state machine has a reachable state that it never leaves: no transition from that state
goes to another one, so only a reset gets it out.

```vhdl
case state is
  when IDLE => if go = '1' then next_state <= DONE; end if;
  when DONE => null;   -- `state` never leaves state `DONE` once there, except by a reset
end case;
```

State machines are found as for
[`sema/fsm-unreachable-state`](#semafsm-unreachable-state). Nothing is reported when the
state is assigned a literal outside the `case` other than by a reset. A final state that
waits for a reset is sometimes intended, hence `info` in `recommended`; `warn` in `strict`.

## `style`

These rules look at one file's text and syntax tree.

### `style/no-buffer-ports`

A port has mode `buffer`. Since VHDL-2008, ports of mode `out` can be read, and earlier code
can use an internal signal; `buffer` ports complicate connections in older tools.

Fix (unsafe), with `standard = "2008"` or later: changes the mode to `out`. It is unsafe
because the port's actual in each instantiation must then be a signal that is not also
driven elsewhere.

### `style/no-std-logic-arith`

A use clause names one of the Synopsys packages `std_logic_arith`, `std_logic_unsigned` or
`std_logic_signed`, which are not standard and conflict with `ieee.numeric_std`. Use
`numeric_std` (and `numeric_std_unsigned` in VHDL-2008).

Fix (unsafe): names `numeric_std` instead, or `numeric_std_unsigned` for
`std_logic_unsigned` with `standard = "2008"` or later. It is unsafe because arithmetic
written for the Synopsys packages may then need conversions.

### `style/line-length`

A line is longer than `max` columns, counted in characters. Off in `recommended`.

| Option | Type | Default |
|--------|------|---------|
| `max` | integer | `120` |

```toml
"style/line-length" = { max = 100 }
```

This limit is separate from the formatter's `format.max_width` (100 by default), which is
where the formatter starts breaking lists.

### `style/trailing-whitespace`

A line ends with spaces or tabs. Fix (safe): removes them. `vhone fmt` removes them too.

### `style/tabs`

A line's indentation contains a tab. `vhone fmt` with `format.use_tabs = false` (the default)
re-indents with spaces.

Fix (safe): replaces the indentation with spaces, a tab reaching the next multiple of
`format.indent` columns.

### `style/process-label`

A process has no label. Labels name processes in simulators and synthesis reports.

### `style/positional-association`

A component or entity instantiation maps ports or generics by position:
`port map (clk, open, o)`. Named association (`clk => clk`) survives changes to the
entity's port order.

Fix (safe): names the formals of the positional associations, from the instantiated entity
or component when it is in the project.

### `style/clock-edge`

A clock edge is tested as `clk'event and clk = '1'` (or `= '0'`, in either order) instead
of `rising_edge(clk)` / `falling_edge(clk)`.

Fix (unsafe): replaces the expression with `rising_edge(clk)` or `falling_edge(clk)`. It is
unsafe because `rising_edge` also excludes transitions from `'X'` or `'L'`, which changes
simulation for those values.

### `style/file-name`

A file's name, without its extension, differs from the name of every primary unit it
declares (entity, package, context, configuration, package instantiation). The comparison
ignores case. The finding suggests the name of the first unit: `expected top.vhd`.

## `naming`

One rule per kind of declaration, `naming/<kind>`. They are off in every preset; setting
options turns a rule on at `warn`.

| Rule | Applies to |
|------|-----------|
| `naming/entity` | entities |
| `naming/architecture` | architectures |
| `naming/package` | packages (and package instantiations) |
| `naming/component` | components |
| `naming/signal` | signals |
| `naming/variable` | variables |
| `naming/constant` | constants |
| `naming/generic` | generics |
| `naming/port` | ports of every mode |
| `naming/input-port` | ports of mode `in` |
| `naming/output-port` | ports of mode `out` or `buffer` |
| `naming/inout-port` | ports of mode `inout` |
| `naming/type` | types |
| `naming/subtype` | subtypes |
| `naming/function` | functions |
| `naming/procedure` | procedures |
| `naming/alias` | aliases |
| `naming/process` | process labels |
| `naming/instance` | instance labels |
| `naming/generate` | generate statement labels |

Options, all optional:

| Option | Type | Meaning |
|--------|------|---------|
| `prefix` | string | The name must start with this (compared without case). |
| `suffix` | string | The name must end with this (compared without case). |
| `case` | string | The case of the name between the prefix and the suffix: `"snake"` or `"lower"` (no upper-case letters), `"upper"` (no lower-case letters), `"pascal"` (starts upper-case, no `_`), `"camel"` (starts lower-case, no `_`), or `"any"`. |
| `pattern` | string | A regular expression ([Rust `regex` syntax](https://docs.rs/regex/latest/regex/#syntax)) the whole name must match. |

```toml
[lint.rules]
"naming/signal"      = { prefix = "s_", case = "snake" }
"naming/constant"    = { prefix = "C_", case = "upper" }
"naming/type"        = { suffix = "_t" }
"naming/input-port"  = { suffix = "_i" }
"naming/output-port" = { suffix = "_o" }
"naming/instance"    = { pattern = "(u|i)_[a-z0-9_]+" }
```

```vhdl
signal valid : std_logic;   -- signal `valid` should start with `s_`
```

The checks run in the order `prefix`, `suffix`, `case`, `pattern`, and a declaration gets at
most one finding: the first problem found. A port is checked against `naming/port` first,
then against the rule for its mode, so both can be used together. Extended identifiers
(`\name\`) are not checked. A rule with an invalid option (an unknown `case`, a pattern
that does not compile) is reported as a configuration warning and skipped.

In an editor, a quick fix renames the declaration everywhere to a name that follows every
convention that applies to it: the prefixes and suffixes added, the rest of the name in the
convention's case (`dataValid` becomes `s_data_valid` for `{ prefix = "s_", case =
"snake" }`). There is none when no such name matches the `pattern`, or when the new name
would clash with another declaration.

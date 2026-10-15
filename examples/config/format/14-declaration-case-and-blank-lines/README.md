# Case by declaration kind, and blank lines

```toml
[format]
identifier_case = "lower"

[format.declaration_case]
generic = "upper"
constant = "upper"
enum_literal = "upper"

[format.blank_lines]
after_begin = 0
before_end = 0
before_process = { min = 1 }
between_declaration_groups = 1
```

- `[format.declaration_case]` sets the case of names by the kind of their declaration:
  `entity`, `architecture`, `package`, `component`, `generic`, `port`, `constant`,
  `signal`, `variable`, `type`, `subtype`, `enum_literal`, `function`, `procedure`,
  `alias`, `label`. Every use of the name in the file takes the same case. Kinds left out
  follow `identifier_case`, and so do names declared in other files (`std_logic`,
  `rising_edge`).
- `[format.blank_lines]` sets how many blank lines go after `begin`, before `begin` and
  `end`, before a process, block or generate statement, and between two groups of
  declarations of different kinds: a number for exactly that many, or
  `{ min = .., max = .. }`. Comment lines right above a process or a declaration stay with
  it; the blank line goes above them. Keys left out keep the blank lines as written.

To check names rather than rewrite them, see the `naming/*` rules in
[lint/04-naming](../../lint/04-naming).

```sh
vfl fmt before.vhd   # compare with after.vhd
```

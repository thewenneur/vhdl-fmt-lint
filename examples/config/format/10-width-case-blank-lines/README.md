# Line width, case alternatives, blank lines and spaces

```toml
[format]
max_width = 60
case_alternative = "break"
max_blank_lines = 2
association_list = "auto"
space_after_keyword = false
```

- `max_width` (default 120, like the `style/line-length` rule): a line longer than this is broken. An instance first moves its
  `port map` to the next line (`u_short`); a map still too long there gets one
  association per line (`u_long`). With the default `association_list = "expand"`, every
  map gets one association per line; `"auto"` keeps the ones that fit on one line. Long expressions break at their operators, see
  [13-long-expressions](../13-long-expressions). Columns are counted as displayed: CJK
  characters take two.
- `case_alternative = "break"` starts every statement of a `when` on its own line;
  `"preserve"` (default) keeps a statement written on the `when` line there.
- `max_blank_lines` shortens longer runs of blank lines (default 1). For blank lines around
  `begin`, `end` and processes, see
  [14-declaration-case-and-blank-lines](../14-declaration-case-and-blank-lines).
- `space_after_keyword = false` writes `process(...)`, `port(...)` and `if(...)` without
  a space. Calls and indexing never get one.

To limit the line length in `vfl lint` too, see the `style/line-length` rule in
[lint/01-presets-and-levels](../../lint/01-presets-and-levels).

```sh
vfl fmt before.vhd   # compare with after.vhd
```

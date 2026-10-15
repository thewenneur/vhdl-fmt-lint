# Indentation

```toml
[format]
indent = 2
continuation_indent = 4
```

- `indent` is the number of columns per level (default 4).
- `continuation_indent` is what a statement continued on the next line gets, outside
  parentheses. It defaults to `indent`; here the `xor` line is indented 4 more than the
  assignment, so it stands out from the next statement.
- `use_tabs = true` indents with tabs instead, `indent` being the tab width. Alignment
  padding is always spaces.

```sh
vfl fmt before.vhd   # compare with after.vhd
```

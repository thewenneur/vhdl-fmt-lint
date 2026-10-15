# Alignment scope

`before.vhd` is formatted with the defaults: every `:`, `<=`, `=>` and line-ending `;`
lines up across its whole block, even across `if` levels and blank lines. This config
goes back to the layout of vfl 0.4 and earlier:

```toml
[format.align]
scope = "contiguous"
block_boundary = "blank-line"
semicolon = false
```

- `scope = "contiguous"` aligns only consecutive lines of the same construct; `"block"`
  (default) aligns the whole process, architecture, port list...
- `block_boundary` says what ends a run of consecutive lines: `"blank-line"` (default),
  `"comment"` (a blank line or a full-line comment) or `"never"`.
- `semicolon = false` stops padding before the `;` that ends a line.

```sh
vfl fmt before.vhd   # compare with after.vhd
```

# The `compact` style

```toml
[format]
style = "compact"
```

A style sets several keys at once. `compact` sets `indent = 2`, `interface_list = "auto"`,
`association_list = "auto"`, `close_paren = "attached"`, `max_blank_lines = 1`, and turns
every alignment off except end-of-line comments on consecutive lines. `before.vhd` is
formatted with the defaults.

The styles are `"default"`, `"leading-comma"` (see
[05-leading-separators](../05-leading-separators)), `"compact"` and `"classic"` (see
[09-style-classic](../09-style-classic)). Any key you set yourself wins over the style.

```sh
vfl fmt before.vhd   # compare with after.vhd
```

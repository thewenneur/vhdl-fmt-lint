# Generic, port and map layouts

```toml
[format]
interface_list = "auto"
association_list = "expand"
close_paren = "attached"
```

`interface_list` lays out `generic (...)` and `port (...)` clauses, `association_list` the
`generic map (...)` and `port map (...)` of instances. Both take:

- `"expand"`: always one element per line (the default for clauses).
- `"auto"`: one per line if the list already spans several lines or is longer than
  `max_width`, else on one line (the default for maps).
- `"preserve"`: the line breaks as written, expanded only when too long.

Here the one-line `generic (DEPTH ...)` stays on one line, the multi-line `port (...)` stays
expanded, and both maps of `u_fifo` get one association per line. `close_paren =
"attached"` puts `)` right after the last element rather than on its own line.

The extra space in `dout  => dout` is alignment: with the default `scope = "block"`, the
`=>` of the generic map and the port map line up together (see
[06-alignment-scope](../06-alignment-scope)).

```sh
vfl fmt before.vhd   # compare with after.vhd
```

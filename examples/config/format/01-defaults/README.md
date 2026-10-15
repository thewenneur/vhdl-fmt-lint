# Formatting with the defaults

An empty `vfl.toml` formats with every default: lower-case keywords, 4-space indent,
one port or generic per line, spaces around operators, and every `:`, `<=`, `=>`, `:=`,
end-of-line comment and line-ending `;` aligned across the whole block (process,
architecture, port list...).

```sh
vfl fmt before.vhd   # rewrites the file: compare it with after.vhd
vfl config show      # every setting, with its default
```

What changed:

- `LIBRARY`, `ENTITY`, ... are lower-cased (`keyword_case = "lower"`); identifiers keep their
  case (`identifier_case = "preserve"`), so `RTL` stays as written.
- The one-line `generic (...)` and `port (...)` clauses are expanded one element per line
  (`interface_list = "expand"`).
- Everything is indented by 4 (`indent = 4`) and the tokens are aligned
  ([`[format.align]`](../../../docs/configuration.md#formatalign)).
- `end entity counter;` keeps its label (`end_label = "preserve"`).

Not to your taste? The next examples change these one at a time.

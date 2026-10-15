# Case and end labels

```toml
[format]
keyword_case = "upper"
identifier_case = "lower"
end_label = "add"
end_keyword = "add"
```

- `keyword_case` sets the case of reserved words: `"lower"` (default), `"upper"` or
  `"preserve"`.
- `identifier_case` sets the case of names: `"preserve"` (default), `"lower"` or `"upper"`.
  VHDL is case-insensitive, so this never changes meaning. Extended identifiers (`\Foo\`)
  are never touched.
- `end_label = "add"` repeats the name after `end` (`end process p_delay;`), `"remove"`
  drops it, `"preserve"` (default) keeps what is written.
- `end_keyword = "add"` repeats the construct keyword (`end entity`, `end process`),
  `"remove"` drops it where it is optional.

```sh
vfl fmt before.vhd   # compare with after.vhd
```

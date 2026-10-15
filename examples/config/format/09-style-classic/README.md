# The `classic` style, with an override

```toml
[format]
style = "classic"
end_label = "preserve"
```

`classic` sets `keyword_case = "upper"`, `end_label = "add"`, `end_keyword = "add"`,
`association_list = "expand"`, `case_alternative = "break"`, and aligns consecutive lines
only, without the `;`. Keys set explicitly win over the style: here `end_label =
"preserve"` keeps `end;` without the name, while `end_keyword = "add"` from the style still
adds `ENTITY` and `ARCHITECTURE`.

```sh
vfl fmt before.vhd   # compare with after.vhd
vfl config show      # shows the keys the style expands to
```

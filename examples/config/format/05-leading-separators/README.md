# Leading separators

```toml
[format]
interface_separator = "leading"
association_separator = "leading"
association_list = "expand"
```

- `interface_separator = "leading"` puts the `;` between generics, ports and parameters at
  the start of the next line.
- `association_separator = "leading"` does the same for the `,` of maps, calls, aggregates
  and enumeration types.

A list that stays on one line is left alone, so `association_list = "expand"` makes sure
the instance's maps are laid out one per line. A comment after a separator (`-- words`)
stays at the end of its line. `style = "leading-comma"` sets these three keys at once.

```sh
vfl fmt before.vhd   # compare with after.vhd
```

# Presets and rule levels

```toml
[lint]
preset = "strict"

[lint.rules]
"style/file-name" = "off"
"sema/unused-port" = "error"
"style/line-length" = { level = "warn", max = 80 }
```

- `preset` gives every rule its starting level: `"none"` (only syntax errors),
  `"recommended"` (default: likely bugs) or `"strict"` (everything except `naming/*`).
  `vfl rules` lists each rule's level in each preset.
- `[lint.rules]` changes rules one by one: a level (`"off"`, `"info"`, `"warn"`,
  `"error"`), or a table with a `level` and the rule's options.

`vfl lint` fails when there is an `error` finding (or a `warn` one, with
`--deny-warnings`). Here `sema/unused-port` is raised to `error` so that unused ports
fail CI.

```sh
vfl lint before.vhd   # the findings are in findings.txt
```

None of these rules has an automatic fix, so `after.vhd` is fixed by hand: the reset is
used, the unused `debug` port is gone, the process has a label and the long comment
has a line of its own. `vfl lint after.vhd` reports nothing.

Every rule is described in [lints.md](../../../docs/lints.md).

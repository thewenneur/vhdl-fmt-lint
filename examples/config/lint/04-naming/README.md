# Naming conventions

```toml
[lint.rules]
"naming/signal" = { prefix = "s_", case = "snake" }
"naming/constant" = { prefix = "C_", case = "upper" }
"naming/type" = { suffix = "_t" }
"naming/input-port" = { suffix = "_i" }
"naming/output-port" = { suffix = "_o" }
"naming/process" = { prefix = "p_" }
"naming/instance" = { level = "error", pattern = "u_[a-z0-9_]+" }
```

There is one `naming/<kind>` rule per kind of declaration, off in every preset. Giving a
rule options turns it on at `warn`; `level` sets another level. The options are `prefix`,
`suffix`, `case` (`"snake"`, `"upper"`, `"pascal"`, `"camel"`, `"lower"`, `"any"`) and
`pattern`, a regular expression the whole name must match.

```sh
vfl lint before.vhd   # the findings are in findings.txt
```

`vfl lint --fix` does not rename: a rename touches every use, in every file. In an editor,
the quick fix on a finding renames the declaration everywhere to a name that follows the
convention (`ledPhase` becomes `s_led_phase`). `after.vhd` is the result, and
`vfl lint after.vhd` reports nothing.

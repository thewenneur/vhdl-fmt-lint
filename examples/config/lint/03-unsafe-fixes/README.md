# Unsafe fixes

```toml
[lint.rules]
"style/clock-edge" = "warn"
"sema/unneeded-sensitivity" = "warn"
```

Some fixes can change behavior, so they are worth a look before you keep them. `vfl lint
--fix` leaves them alone; `vfl lint --fix --unsafe-fixes` applies them:

- `style/no-std-logic-arith`: `ieee.std_logic_unsigned` becomes
  `ieee.numeric_std_unsigned` (VHDL-2008).
- `sema/unused-signal`: the unused `spare` declaration is removed.
- `style/clock-edge`: `clk'event and clk = '1'` becomes `rising_edge(clk)`, which also
  ignores transitions from `'X'`.
- `sema/sensitivity-list`: `load`, read by the combinational process, is added to its
  list.
- `sema/unneeded-sensitivity`: `value`, which the process never reads, is removed.

The first three are on in the default preset; the config turns on the other two.

```sh
vfl lint before.vhd                         # the findings are in findings.txt
vfl lint --fix --unsafe-fixes before.vhd    # compare with after.vhd
```

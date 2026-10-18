# Bus widths and index ranges

```toml
[lint.rules]
"sema/width-mismatch" = { level = "error", generics = true }
"sema/index-range" = { level = "error", generics = false }
```

Widths and bounds are worked out through generics and constants: `unsigned(WIDTH downto
0)` has `WIDTH + 1` elements, whatever `WIDTH` is. So vfl knows that `acc(WIDTH + 1)` is
always outside `acc`, and at an instantiation it replaces the generics of the instantiated
entity by the actuals of the generic map (or their defaults): with `WIDTH => DATA_W`, the
port `sum` has `DATA_W + 1` elements, and `total` has `DATA_W`.

- `generics` of `sema/width-mismatch` (on by default): when two widths differ by an
  amount that depends on generics (`DEPTH` and `WIDTH`), compare them with the generics at
  their default values, and say so in the finding. Off, only mismatches that hold for every
  value of the generics are reported.
- `generics` of `sema/index-range` and of `sema/comparison-width` work the same way. Here
  `sema/index-range` keeps only what holds for any generics.

Findings that use default values are not reported under an `if` or `case` generate, which
usually exists for other values.

```sh
vfl lint before.vhd   # the findings are in findings.txt
```

`after.vhd` is fixed by hand: the counter counts, the top bit is `acc(WIDTH)`, and `total`
is one bit wider. `vfl lint after.vhd` reports nothing.

Every rule is described in [lints.md](../../../docs/lints.md).

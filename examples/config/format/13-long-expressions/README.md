# Long expressions

```toml
[format]
max_width = 72
operator_position = "leading"
continuation_indent = 8
```

- A line longer than `max_width` first gets its port or generic map broken up (see
  [10-width-case-blank-lines](../10-width-case-blank-lines)). If it is still too long, its
  expressions are broken at the operators that bind loosest: the `else` of a conditional
  assignment, then `and`/`or`, then `=`/`<`..., then `+`/`-`/`&`, then `*`/`/`. All the
  operators of the same kind in a row break together, and what is in fewer parentheses
  breaks first: the `if` condition breaks at `or`, then, as the part in parentheses is
  still too long, at its `and`s.
- `operator_position = "leading"` starts continuation lines with the operator;
  `"trailing"` ends lines with it; `"preserve"` (default) keeps the breaks written in the
  file where they are and puts the ones vfl adds after the operator.
- `continuation_indent` is how far the continuation lines go in. The values of a
  conditional assignment line up under the first one instead
  (`[format.align] conditional_continuation`).

A line that fits is never changed, and line breaks written in the file are kept.
`break_expressions = false` turns this off: only lists are broken.

```sh
vfl fmt before.vhd   # compare with after.vhd
```

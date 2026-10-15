# Safe fixes

```toml
[lint.rules]
"style/trailing-whitespace" = "warn"
"style/tabs" = "warn"
"style/positional-association" = "warn"
```

These rules are off in the default preset, so the config turns them on. Their fixes cannot
change behavior, so `vfl lint --fix` applies them:

- trailing spaces and tabs are removed;
- tab indentation is replaced by spaces (`format.indent` columns per tab stop);
- `port map (x0, x1, y)` names its formals: `port map (a => x0, b => x1, y => y)`.

```sh
vfl lint before.vhd         # the findings are in findings.txt
vfl lint --fix before.vhd   # rewrites the file: compare it with after.vhd
```

In an editor, each fix is a quick fix, and "Fix all" (`source.fixAll.vfl`) applies the
safe ones. The `[project] exclude` line only keeps `after.vhd` out of the project, so that
the two copies of the same entities do not clash.

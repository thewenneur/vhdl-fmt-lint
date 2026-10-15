# Silencing findings in the source

```toml
[lint]
preset = "strict"

[lint.rules]
"style/file-name" = "off"
```

When a finding is intended, a comment silences it where it is, instead of turning the rule
off for the whole project:

- `-- vfl-ignore rule-id` at the end of a line silences the rule on that line;
- `-- vfl-ignore rule-id` on a line of its own silences it on the next line of code;
- `-- vfl-ignore-file rule-id` anywhere silences it in the whole file.

Several ids can be listed, a category (`style`, `sema`, ...) silences all its rules, and
no id silences every rule. Another `--` comment after the ids says why.

```sh
vfl lint before.vhd   # the findings are in findings.txt
vfl lint after.vhd    # nothing left
```

In an editor, every finding has quick fixes that add these comments.

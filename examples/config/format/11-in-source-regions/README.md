# Keeping code as written

Comments in the source turn the formatter off for part of a file, whatever the config
says:

- `-- vfl: off` ... `-- vfl: on`: the lines between are kept exactly as written. Here the
  ROM table keeps its four values per line.
- `-- vfl: skip`: the next statement or declaration is kept exactly as written.

The rest of the file is formatted as usual (the config is empty: the defaults).

```sh
vfl fmt before.vhd   # compare with after.vhd
```

In Verilog and SystemVerilog the same comments are written with `//`.

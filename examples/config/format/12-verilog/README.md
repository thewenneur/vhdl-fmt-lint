# Verilog and SystemVerilog

```toml
[verilog]
mode = "full"

[format]
indent = 2
association_list = "expand"
association_separator = "leading"
```

vfl formats Verilog and SystemVerilog files only with `[verilog] mode = "full"`; by
default it reads them just for the modules VHDL instantiates. The `[format]` keys that
have a meaning in Verilog apply to it as well: `indent`, `max_width`, the list layouts
and separators (`interface_list` for port and `#(...)` parameter lists,
`association_list` for instance connections), and the `[format.align]` switches.
`keyword_case`, `identifier_case` and the end label keys do not: Verilog is
case-sensitive.

```sh
vfl fmt before.sv   # compare with after.sv
```

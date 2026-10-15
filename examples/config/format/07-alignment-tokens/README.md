# Choosing which tokens align

Each aligned token has a switch in `[format.align]`. This config keeps ports, generics and
declarations aligned, but not assignments, case choices, end-of-line comments or `;`:

```toml
[format.align]
port_colon = true          # ":" of ports and parameters
port_mode = true           # the type after in/out/inout
generic_colon = true       # ":" of generics
interface_default = true   # ":=" of default values
declaration = true         # ":" and ":=" of signals, constants, variables, record elements
assignment = false         # "<=" and ":="
case_choice = false        # "=>" of case alternatives
trailing_comment = false   # comments at the end of lines
semicolon = false          # the ";" ending a line
```

The other switches are `association` (`=>` of maps and aggregates), `when_clause` and
`conditional_continuation` (conditional and selected assignments). `before.vhd` is
formatted with the defaults, where all of them are on.

```sh
vfl fmt before.vhd   # compare with after.vhd
```

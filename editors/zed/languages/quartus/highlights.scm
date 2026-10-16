; Quartus settings and SDC/XDC constraints: Tcl, with the vendor commands,
; options, assignment names and pins standing out.
; Later patterns take precedence.

(comment) @comment

(command name: (simple_word) @function)

((command name: (simple_word) @function.builtin)
  (#match? @function.builtin "^(set_(global|instance|location|io|power_file)_assignment|set_parameter|set_user_option|create_(generated_)?clock|set_(false|multicycle)_path|set_(max|min)_delay|set_(input|output)_delay|set_clock_(groups|uncertainty|latency|transition|sense)|set_property|derive_(pll_)?clocks|derive_clock_uncertainty|set_max_skew|set_net_delay|set_disable_timing|set_case_analysis|set_time_format|create_pblock|add_cells_to_pblock|resize_pblock|set_bus_skew|group_path)$"))

((command name: (simple_word) @function.builtin)
  (#match? @function.builtin "^(get_[a-z_]+|all_(clocks|inputs|outputs|registers|fanin|fanout))$"))

((simple_word) @property
  (#match? @property "^-[A-Za-z_][A-Za-z0-9_]*$"))

((simple_word) @constant
  (#match? @constant "^(PIN_[A-Za-z]*[0-9]+|[A-Z][A-Z0-9_]*_[A-Z0-9_]+)$"))

"proc" @keyword
[
  "if"
  "else"
  "elseif"
  "while"
  "foreach"
  "set"
  "global"
  "namespace"
  "catch"
  "try"
  "finally"
  "error"
] @keyword

(procedure name: (_) @function)
(set (id) @variable)
(variable_substitution) @variable
(quoted_word) @string
(escaped_character) @string.escape
(number) @number

((simple_word) @number
  (#match? @number "^[+-]?[0-9]+(\\.[0-9]+)?$"))

[
  "{"
  "}"
  "["
  "]"
] @punctuation.bracket

";" @punctuation.delimiter

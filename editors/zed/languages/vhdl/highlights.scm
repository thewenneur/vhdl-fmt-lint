; Adapted from the Helix queries of tree-sitter-vhdl
; (https://github.com/jpt13653903/tree-sitter-vhdl, MIT License,
; Copyright (c) 2024 John-Philip Taylor), with Zed's capture names.
; Later patterns take precedence.
(line_comment) @comment

(block_comment) @comment

(identifier) @variable

[
  "access"
  "after"
  "alias"
  "architecture"
  "array"
  ; "assume"
  "attribute"
  "block"
  "body"
  "component"
  "configuration"
  "context"
  ; "cover"
  "disconnect"
  "entity"
  ; "fairness"
  "file"
  "force"
  "generate"
  "generic"
  "group"
  "label"
  "literal"
  "map"
  "new"
  "package"
  "parameter"
  "port"
  "property"
  "range"
  "reject"
  "release"
  ; "restrict"
  "sequence"
  "transport"
  "unaffected"
  "view"
  ; "vmode"
  ; "vpkg"
  ; "vprop"
  "vunit"
] @keyword

[
  (ALL)
  (OTHERS)
  "<>"
  (DEFAULT)
  (OPEN)
] @constant.builtin

[
  "is"
  "begin"
  "end"
] @keyword

(parameter_specification
  "in" @keyword)

[
  "process"
  "wait"
  "on"
  "until"
] @keyword

(timeout_clause
  "for" @keyword)

[
  "function"
  "procedure"
] @keyword

[
  "to"
  "downto"
  "of"
] @keyword

[
  "library"
  "use"
] @keyword

[
  "subtype"
  "type"
  "record"
  "units"
  "constant"
  "signal"
  "variable"
] @keyword

[
  "protected"
  "private"
  "pure"
  "impure"
  "inertial"
  "postponed"
  ; "strong"
  "guarded"
  "out"
  "inout"
  "linkage"
  "buffer"
  "register"
  "bus"
  "shared"
] @keyword

(mode
  "in" @keyword)

(force_mode
  "in" @keyword)

[
  "while"
  "loop"
  "next"
  "exit"
] @keyword

(for_loop
  "for" @keyword)

(block_configuration
  "for" @keyword)

(configuration_specification
  "for" @keyword)

(component_configuration
  "for" @keyword)

(end_for
  "for" @keyword)

"return" @keyword

[
  "assert"
  "report"
  "severity"
] @keyword

[
  "if"
  "then"
  "elsif"
  "case"
] @keyword

(when_element
  "when" @keyword)

(case_generate_alternative
  "when" @keyword)

(else_statement
  "else" @keyword)

(else_generate
  "else" @keyword)

[
  "with"
  "select"
] @keyword

(when_expression
  "when" @keyword)

(else_expression
  "else" @keyword)

(else_waveform
  "else" @keyword)

(else_expression_or_unaffected
  "else" @keyword)

"null" @constant.builtin

(user_directive) @preproc

(protect_directive) @preproc

(warning_directive) @preproc

(error_directive) @preproc

(if_conditional_analysis
  "if" @preproc)

(if_conditional_analysis
  "then" @preproc)

(elsif_conditional_analysis
  "elsif" @preproc)

(else_conditional_analysis
  "else" @preproc)

(end_conditional_analysis
  "end" @preproc)

(end_conditional_analysis
  "if" @preproc)

(directive_body) @preproc

(directive_constant_builtin) @constant.builtin

(directive_error) @preproc

(directive_protect) @preproc

(directive_warning) @preproc

[
  (condition_conversion)
  (relational_operator)
  (sign)
  (adding_operator)
  (exponentiate)
  (variable_assignment)
  (signal_assignment)
  "*"
  "/"
  ":"
  "=>"
] @operator

[
  (unary_operator)
  (logical_operator)
  (shift_operator)
  "mod"
  "not"
  "rem"
] @keyword

[
  "'"
  ","
  "."
  ";"
] @punctuation.delimiter

[
  "("
  ")"
  "["
  "]"
  "<<"
  ">>"
] @punctuation.bracket

"@" @punctuation.special

[
  (decimal_integer)
  (string_literal_std_logic)
] @number

(decimal_float) @number

(bit_string_length) @type

(bit_string_base) @type.builtin

(bit_string_value) @number

(based_literal
  (based_base) @type.builtin
  (based_integer) @number)

(based_literal
  (based_base) @type.builtin
  (based_float) @number)

(string_literal) @string

(character_literal) @string.special

(library_constant_std_logic) @constant.builtin

(library_constant) @constant.builtin

(library_function) @function.builtin

(library_constant_boolean) @boolean

(library_constant_character) @string.special

(library_constant_debug) @keyword

(unit) @keyword

(library_constant_unit) @keyword

(label) @label

(generic_map_aspect
  "generic" @keyword
  "map" @keyword)

(port_map_aspect
  "port" @keyword
  "map" @keyword)

(selection
  (identifier) @property)

(_
  view: (_) @type)

(_
  type: (_) @type)

(_
  library: (_) @type)

(_
  package: (_) @type)

(_
  entity: (_) @type)

(_
  component: (_) @type)

(_
  configuration: (_) @type)

(_
  architecture: (_) @type)

(_
  function: (_) @function)

(_
  procedure: (_) @function)

(_
  attribute: (_) @attribute)

(_
  constant: (_) @constant)

(_
  generic: (_) @variable.special)

(_
  view: (name
    (_)) @type)

(_
  type: (name
    (_)) @type)

(_
  entity: (name
    (_)) @type)

(_
  component: (name
    (_)) @type)

(_
  configuration: (name
    (_)) @type)

(library_type) @type.builtin


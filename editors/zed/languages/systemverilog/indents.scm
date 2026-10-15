; The language server also re-indents the line on Enter and `;` (on-type
; formatting) when `[verilog]` `mode = "full"`; these rules indent before it
; answers.

(_ "(" ")" @end) @indent
(_ "[" "]" @end) @indent
(_ "{" "}" @end) @indent
(_ "begin" "end" @end) @indent

[
  (module_declaration)
  (interface_declaration)
  (package_declaration)
  (class_declaration)
  (function_declaration)
  (task_declaration)
  (case_statement)
] @indent

[
  "endmodule"
  "endinterface"
  "endpackage"
  "endclass"
  "endfunction"
  "endtask"
  "endcase"
] @end

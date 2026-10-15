; The language server also re-indents the line on Enter and `;` (on-type
; formatting); these rules indent before it answers.

(_ "(" ")" @end) @indent

[
  (entity_declaration)
  (architecture_definition)
  (package_declaration)
  (package_definition)
  (component_declaration)
  (process_statement)
  (block_statement)
  (for_generate_statement)
  (if_generate_statement)
  (case_generate_statement)
  (if_statement_block)
  (case_statement)
  (loop_statement)
  (subprogram_definition)
  (record_type_definition)
] @indent

[
  (end_entity)
  (end_architecture)
  (end_package)
  (end_package_body)
  (end_component)
  (end_process)
  (end_block)
  (end_generate)
  (end_if)
  (end_case)
  (end_loop)
  (end_record)
  (subprogram_end)
] @end

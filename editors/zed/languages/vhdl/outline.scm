(entity_declaration
  "entity" @context
  entity: (_) @name) @item

(architecture_definition
  "architecture" @context
  architecture: (_) @name
  "of" @context
  entity: (_) @context) @item

(package_declaration
  "package" @context
  package: (_) @name) @item

(package_definition
  "package" @context
  "body" @context
  package: (_) @name) @item

(component_declaration
  "component" @context
  component: (_) @name) @item

(type_declaration
  "type" @context
  type: (_) @name) @item

(subprogram_declaration
  [
    (function_specification "function" @context function: (_) @name)
    (procedure_specification "procedure" @context procedure: (_) @name)
  ]) @item

(subprogram_definition
  [
    (function_specification "function" @context function: (_) @name)
    (procedure_specification "procedure" @context procedure: (_) @name)
  ]) @item

(process_statement
  (label_declaration (label) @name)
  "process" @context) @item

(component_instantiation_statement
  (label_declaration (label) @name)) @item

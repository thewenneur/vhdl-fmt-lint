(module_declaration
  [
    (module_ansi_header (module_keyword) @context name: (_) @name)
    (module_nonansi_header (module_keyword) @context name: (_) @name)
  ]) @item

(interface_declaration
  [
    (interface_ansi_header "interface" @context name: (_) @name)
    (interface_nonansi_header "interface" @context name: (_) @name)
  ]) @item

(package_declaration
  "package" @context
  name: (_) @name) @item

(class_declaration
  "class" @context
  name: (_) @name) @item

(function_declaration
  "function" @context
  (function_body_declaration name: (_) @name)) @item

(task_declaration
  "task" @context
  (task_body_declaration name: (_) @name)) @item

(module_instantiation
  instance_type: (_) @context
  (hierarchical_instance
    (name_of_instance instance_name: (_) @name))) @item

(generate_block
  name: (_) @name) @item

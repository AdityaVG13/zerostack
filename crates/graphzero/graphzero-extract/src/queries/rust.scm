;; Rust definition queries

(function_item name: (identifier) @def_name) @def_node
(struct_item name: (type_identifier) @def_name) @def_node
(enum_item name: (type_identifier) @def_name) @def_node
(trait_item name: (type_identifier) @def_name) @def_node
(type_item name: (type_identifier) @def_name) @def_node
(impl_item type: (type_identifier) @def_name) @def_node
(mod_item name: (identifier) @def_name) @def_node
(const_item name: (identifier) @def_name) @def_node
(static_item name: (identifier) @def_name) @def_node

;; Rust call expressions
(call_expression function: (identifier) @call_name) @call_node
(call_expression function: (field_expression field: (field_identifier) @call_name)) @call_node
(call_expression function: (scoped_identifier name: (identifier) @call_name)) @call_node

;; Rust use and import statements
(use_declaration argument: (scoped_identifier) @import_path) @import_node
(use_declaration argument: (identifier) @import_path) @import_node
(use_wildcard (identifier) @import_path) @import_node

;; Rust trait implementation blocks
(impl_item trait: (type_identifier) @trait_name) @impl_node

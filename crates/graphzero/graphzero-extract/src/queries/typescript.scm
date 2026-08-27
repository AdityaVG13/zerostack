;; TypeScript definition queries (FR-004)
(function_declaration name: (identifier) @def_name) @def_node
(generator_function_declaration name: (identifier) @def_name) @def_node
(class_declaration name: (type_identifier) @def_name) @def_node
(interface_declaration name: (type_identifier) @def_name) @def_node
(type_alias_declaration name: (type_identifier) @def_name) @def_node
(method_definition name: (property_identifier) @def_name) @def_node
(variable_declarator name: (identifier) @def_name) @def_node
(public_field_definition name: (property_identifier) @def_name) @def_node

;; TypeScript call expressions (FR-006)
(call_expression function: (identifier) @call_name) @call_node
(call_expression function: (member_expression property: (property_identifier) @call_name)) @call_node

;; TypeScript import statements (FR-008)
(import_statement source: (string) @import_path) @import_node

;; TypeScript implements (FR-009)
;; implements_clause has no fields, children are type_identifier
(implements_clause (type_identifier) @trait_name) @impl_node

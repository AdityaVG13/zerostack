;; Python definition queries (FR-004)
(function_definition name: (identifier) @def_name) @def_node
(class_definition name: (identifier) @def_name) @def_node

;; Python call expressions (FR-006)
(call function: (identifier) @call_name) @call_node
(call function: (attribute attribute: (identifier) @call_name)) @call_node

;; Python import statements (FR-008)
(import_statement name: (dotted_name) @import_path) @import_node
(import_from_statement module_name: (dotted_name) @import_path) @import_node

;; Python inheritance (FR-009) — Python uses inheritance, not explicit implements.
;; Resolve each base to its rightmost identifier, including qualified and generic bases.
(class_definition name: (identifier) @def_name superclasses: (argument_list (identifier) @trait_name)) @impl_node
(class_definition name: (identifier) @def_name superclasses: (argument_list (attribute attribute: (identifier) @trait_name))) @impl_node
(class_definition name: (identifier) @def_name superclasses: (argument_list (subscript value: (identifier) @trait_name))) @impl_node
(class_definition name: (identifier) @def_name superclasses: (argument_list (subscript value: (attribute attribute: (identifier) @trait_name)))) @impl_node

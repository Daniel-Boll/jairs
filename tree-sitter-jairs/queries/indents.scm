; Jairs tree-sitter indents query
; Defines indentation rules for editors.

; Opening braces increase indent
[
  (block)
  (field_list)
  (param_list)
  (arg_list)
] @indent

; Closing braces decrease indent
[
  "}"
  ")"
] @dedent

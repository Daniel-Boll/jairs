; Jairs tree-sitter indents query
; Defines indentation rules for editors.

; Opening delimiters increase indent.
[
  (block)
  (field_list)
  (param_list)
  (arg_list)
] @indent.begin

; A closer ends the indentation range opened by its containing node.
(block
  "}" @indent.end)

(field_list
  "}" @indent.end)

(param_list
  ")" @indent.end)

(arg_list
  ")" @indent.end)

; Reindent a line that begins with a closer before inserting it.
[
  "}"
  ")"
] @indent.branch

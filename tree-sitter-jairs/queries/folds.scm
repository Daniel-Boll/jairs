; Jairs tree-sitter folds query
; Defines foldable regions for editors.

; Blocks (procedure bodies, if/while bodies, nested blocks)
(block) @fold

; Struct field lists
(field_list) @fold

; Parameter lists (for long signatures)
(param_list) @fold

; Argument lists (for long calls)
(arg_list) @fold

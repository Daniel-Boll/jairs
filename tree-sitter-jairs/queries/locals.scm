; Jairs tree-sitter locals query
; Defines scopes and variable definitions/references for semantic highlighting
; and goto-definition support.

; ---- Scopes -----------------------------------------------------------------

; The source file is the top-level scope
(source_file) @local.scope

; Blocks introduce new scopes
(block) @local.scope

; Procedures introduce a scope (for parameters)
(proc) @local.scope

; ---- Definitions ------------------------------------------------------------

; Constant declarations define a name
(const_decl
  name: (name (identifier) @local.definition))

; Variable declarations define a name
(var_decl
  name: (name (identifier) @local.definition))

; Parameters define names in the procedure scope
(param (identifier) @local.definition)

; ---- References -------------------------------------------------------------

; Name expressions are references
(name_expr (identifier) @local.reference)

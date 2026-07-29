; Jairs tree-sitter highlights query
; Covers: keywords, types, functions, strings, numbers, comments, operators,
; punctuation, directives, and reserved keywords.

; ---- Comments ---------------------------------------------------------------

; Documentation reads differently from an aside (ADR-0027 §5). The grammar is
; unchanged: `///` and `//!` are still `line_comment` to tree-sitter, so this is a
; text predicate rather than a new node. Order matters -- the later capture wins in
; Neovim, so the specific patterns come after the general one.
(line_comment) @comment
(block_comment) @comment
((line_comment) @comment.documentation
  (#lua-match? @comment.documentation "^///[^/]"))
((line_comment) @comment.documentation
  (#lua-match? @comment.documentation "^//!"))

; ---- Directives (a defining feature of Jairs) --------------------------------

(directive) @keyword.directive

; ---- Keywords ----------------------------------------------------------------

[
  "struct"
  "if"
  "else"
  "while"
  "return"
  "break"
  "continue"
  ; `cast` is real syntax as of ADR-0037, not a reserved word. It is listed here rather
  ; than in the reserved-keyword match below, which would have kept colouring it as
  ; "arrives in a later wave" after it arrived.
  "cast"
  ; `enum` likewise, as of ADR-0041 — and it was in the reserved match, so this is the
  ; second time the same trap has been walked into. The reserved list is the thing to check
  ; whenever a keyword becomes real.
  "enum"
  ; `enum_flags`, new in ADR-0043 rather than promoted from the reserved list — it was never
  ; reserved, so there was nothing to remove.
  "enum_flags"
  ; `union`, real as of ADR-0045 and promoted *out* of the reserved match below — the third
  ; time that pairing has come up, after `cast` and `enum`. Checked rather than discovered
  ; this time, because ADR-0045's Consequences named it in advance.
  "union"
  ; `xx`, real as of ADR-0046 and out of the reserved match too — four for four.
  "xx"
  ; `operator`, new in ADR-0048 and never reserved, so there was nothing to remove — the same
  ; position `enum_flags` was in, which is why both sit outside `is_reserved_keyword`'s range.
  "operator"
] @keyword

; Boolean literals are keywords in Jairs
(true) @boolean
(false) @boolean

; Reserved keywords — highlight so users see them as reserved
; These lex as identifiers but we match them by text.
((identifier) @keyword.reserved
  (#match? @keyword.reserved "^(for|defer|using|null)$"))

; ---- Literals ----------------------------------------------------------------

(integer_literal) @number
(float_literal) @number.float
(string_literal) @string

; Uninit expression
(uninit_expr) @constant.builtin

; ---- Types -------------------------------------------------------------------

; Named types (identifiers used in type position)
(name_type (identifier) @type)
(pointer_type "*" @operator)

; Struct keyword in struct type
(struct_type "struct" @keyword.type)

; Enum keyword, and a member name — which is a *constant*, matching how a `::` declaration's
; name is captured, because that is what an enum member is (ADR-0012, ADR-0041 §3).
(enum_type "enum" @keyword.type)
(enum_type "enum_flags" @keyword.type)
(member name: (identifier) @constant)

; ---- Declarations ------------------------------------------------------------

; Constant declaration name
(const_decl name: (name (identifier) @constant))

; Variable declaration name
(var_decl name: (name (identifier) @variable))

; Procedure name (constant whose value is a proc)
(const_decl
  name: (name (identifier) @function)
  value: (proc))

; Struct type name
(const_decl
  name: (name (identifier) @type)
  value: (struct_type))

; ---- Procedure signatures ---------------------------------------------------

; Parameter names
(param (identifier) @variable.parameter)

; Return type arrow
(ret_type "->" @operator)

; ---- Expressions ------------------------------------------------------------

; Name references
(name_expr (identifier) @variable)

; Function calls
(call_expr function: (name_expr (identifier) @function.call))
(call_expr function: (field_expr field: (identifier) @function.call))

; Field access
(field_expr field: (identifier) @property)

; ---- Operators ---------------------------------------------------------------

; Binary operators
(binary_expr "+" @operator)
(binary_expr "-" @operator)
(binary_expr "*" @operator)
(binary_expr "/" @operator)
(binary_expr "%" @operator)
(binary_expr "+%" @operator)
(binary_expr "-%" @operator)
(binary_expr "*%" @operator)
(binary_expr "==" @operator)
(binary_expr "!=" @operator)
(binary_expr "<" @operator)
(binary_expr "<=" @operator)
(binary_expr ">" @operator)
(binary_expr ">=" @operator)
(binary_expr "&&" @operator)
(binary_expr "||" @operator)

; Unary operators
(unary_expr "-" @operator)
(unary_expr "!" @operator)
(unary_expr "*" @operator)

; Dereference
(deref_expr ".*" @operator)

; Assignment operators
(assign_stmt "=" @operator)
(assign_stmt "+=" @operator)
(assign_stmt "-=" @operator)
(assign_stmt "*=" @operator)
(assign_stmt "/=" @operator)
(assign_stmt "%=" @operator)
(assign_stmt "+%=" @operator)
(assign_stmt "-%=" @operator)
(assign_stmt "*%=" @operator)

; ---- Punctuation -------------------------------------------------------------

"(" @punctuation.bracket
")" @punctuation.bracket
"{" @punctuation.bracket
"}" @punctuation.bracket

"," @punctuation.delimiter
";" @punctuation.delimiter
":" @punctuation.delimiter

"::" @punctuation.special
":=" @punctuation.special
"->" @punctuation.special
"." @punctuation.delimiter
".*" @operator

; ---- Import and run directives -----------------------------------------------

; #import path
(import_decl
  directive: (directive) @keyword.import
  path: (string_literal) @string.special.path)

; #run at top level
(run_decl
  directive: (directive) @keyword.directive)

; #run as expression
(run_expr
  directive: (directive) @keyword.directive)

; #foreign attribute
(foreign_attr
  directive: (directive) @keyword.directive
  lib: (identifier) @variable
  symbol: (string_literal) @string)

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
] @keyword

; Boolean literals are keywords in Jairs
(true) @boolean
(false) @boolean

; Reserved keywords — highlight so users see them as reserved
; These lex as identifiers but we match them by text.
((identifier) @keyword.reserved
  (#match? @keyword.reserved "^(enum|union|for|defer|using|cast|xx|null)$"))

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

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
  ; `for` and `defer`, real as of ADR-0049 and promoted *out* of the reserved match below.
  ; That makes five for five on the same pairing — `cast`, `enum`, `union`, `xx`, and now
  ; these two — which is why ADR-0049's Consequences named the trip in advance instead of
  ; leaving it to be discovered by a keyword still colouring as "arrives in a later wave".
  "for"
  "defer"
  ; `using`, real as of ADR-0050 and the **last** keyword to leave the reserved match — the block
  ; is now empty of anything Jairs has implemented, so `null` is all that remains. Seven for seven
  ; on this pairing: `cast`, `enum`, `union`, `xx`, `for`, `defer`, `using`.
  "using"
  ; `context`, real as of ADR-0057 and never reserved, so there was nothing to remove — the same
  ; position `enum_flags` and `operator` were in, which is why all three sit outside
  ; `is_reserved_keyword`'s range.
  ;
  ; Worth a separate word: `context` is the one keyword so far that the *grammar* would have
  ; accepted without a rule, because it is a legal identifier. So the failure mode was not an
  ; ERROR node — the corpus parsed and `context.allocator` was a field access on a name nobody
  ; declared, colouring as a variable. A node the two parsers agree on is what makes it a keyword
  ; here rather than a text predicate.
  "context"
  ; `push_context`, real as of ADR-0063 and never reserved — the same position `context`,
  ; `enum_flags` and `operator` were in. Like `context` (above), it is a legal identifier, so the
  ; failure mode of omitting it here is not an ERROR node but a silent mis-colour: `push_context`
  ; would highlight as a variable. The `push_context_stmt` node is what makes it a keyword here
  ; rather than a text predicate — the same node-not-text argument `context` records.
  "push_context"
  ; `switch` and `case`, real as of ADR-0067 and never reserved. Both are legal identifiers, so the
  ; failure mode of omitting them is not an ERROR node but a silent mis-colour — the same trap
  ; `context` and `push_context` record. The `switch_stmt`/`switch_arm` nodes are what make them
  ; keywords here rather than a text predicate.
  "switch"
  "case"
  ; `variant`, real as of ADR-0068 and never reserved. A legal identifier, so omitting it here is a
  ; silent mis-colour rather than an ERROR node — the trap `context`, `push_context` and `switch` each
  ; record. The `variant_type` node is what makes it a keyword rather than a text predicate.
  "variant"
] @keyword

; Boolean literals are keywords in Jairs
(true) @boolean
(false) @boolean

; `null` is a literal (ADR-0060 §1), coloured like the boolean literals. It used to be matched as a
; reserved *identifier* here — that rule is dead now that `null` lexes as its own `NULL_KW` and
; parses as a `(null)` node, so the reserved-keyword highlight block is now empty of anything Jairs
; refuses. The next reserved word to be added will bring it back.
(null) @constant.builtin

; A loop label and the name a `break`/`continue` targets (ADR-0049 §2). Captured as a label
; rather than a variable because it names a *loop* and is deliberately not in the resolve
; map — a reader who saw it coloured as a variable would look for a declaration of it.
(loop_label label: (identifier) @label)
(break_stmt label: (identifier) @label)
(continue_stmt label: (identifier) @label)

; A `for`'s bindings are variables the body reads, so they are captured as such.
(for_stmt value: (identifier) @variable.parameter)
(for_stmt index: (identifier) @variable.parameter)

; The range's `..`, which exists only here (ADR-0049 §1).
(range_expr ".." @operator)

; The `#c_call` attribute (ADR-0057 §3). Captured for the same reason `scope_decl` is: it is a
; literal token rather than a `(directive)` node, so the general `(directive) @keyword.directive`
; rule above does not reach it and it would silently have no colour at all.
(c_call_attr) @keyword.directive

; And `#no_abc` (ADR-0058 §3), for exactly the same reason. Two rules rather than one alternation
; because the node kinds are two: a query naming a node the grammar has not got exits 1 with
; `Invalid node type` (ADR-0025 §4), so a separate rule per kind is what makes gate 6 able to see a
; missing one.
(no_abc_attr) @keyword.directive

; And `#must` (ADR-0151 §1) — the whole node, like the two above, because the rule holds nothing but
; the directive token.
(must_attr) @keyword.directive
(c_variadic_attr) @keyword.directive

; And the two field layout attributes (ADR-0144 §1), for the same reason again — `#align` and
; `#place` are literal tokens inside their nodes, so nothing else colours them. Only the directive
; token is captured, because the operand is an ordinary expression and colouring it as a keyword
; would make `#align ALIGNMENT` read as two keywords.
(align_attr "#align" @keyword.directive)
(place_attr "#place" @keyword.directive)

; And `#soa` on a struct (ADR-0147 §1), for the same reason once more: it is a literal token inside
; its node, so nothing else colours it. The count is an ordinary expression and is left alone.
(soa_attr "#soa" @keyword.directive)

; And `#simd` on a vector type (ADR-0148 §1), for the third time in this wave and the same reason: a
; literal token inside its own node, so nothing else colours it. The lane count and the element type
; are an ordinary expression and an ordinary type, and are left to their own rules.
(vector_type "#simd" @keyword.directive)

; A visibility marker (ADR-0054 §1). Captured as `@keyword.directive` like every other directive,
; because that is what it is — the scope rule is semantic and nothing about it is visible in colour.
(scope_decl) @keyword.directive

; A named argument's name (ADR-0053 §1). Captured as a parameter rather than a variable, because
; that is what it names — a reader seeing it coloured as a variable would look for a declaration of
; it in the current scope.
(named_arg name: (identifier) @variable.parameter)

; A destructuring target (ADR-0052 §2). Captured as a variable, because that is what each one is —
; and a `_` discard is captured too, deliberately: it is an ordinary identifier to the grammar, and
; special-casing it here would need a text predicate for no visual gain.
(target_list (identifier) @variable)

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

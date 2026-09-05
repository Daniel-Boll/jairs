; The outline panel and breadcrumbs, a Zed feature with no Neovim counterpart here.
;
; Everything a Jairs file declares at file scope is a `const_decl` or a `var_decl` (ADR-0012), so the
; outline is those two plus a struct's fields and an enum's members — the things a reader scrolls to
; find.
;
; `@context` captures the tokens that make the entry readable without the body: `::` says at a glance
; that the name is a constant and `:=` that it is a variable, which is the distinction this language
; cares most about.

; A procedure. Matched before the general constant rule below so the more specific one wins.
(const_decl
  name: (name (identifier) @name)
  "::" @context
  value: (proc)) @item

; A struct, union, variant or enum type.
(const_decl
  name: (name (identifier) @name)
  "::" @context
  value: [(struct_type) (union_type) (variant_type) (enum_type)]) @item

; Any other file-scope constant.
(const_decl
  name: (name (identifier) @name)
  "::" @context) @item

; A file-scope variable (ADR-0186).
(var_decl
  name: (name (identifier) @name)) @item

; A struct's fields and an enum's members, so a type's shape is navigable rather than just its name.
;
; A `field`'s identifier is **positional**, not a named field — the grammar spells it
; `seq(optional("using"), $.identifier, ":", field("type", ...))`, so `name:` is an impossible
; pattern and the query would not compile. A `member`'s *is* named. The asymmetry is the grammar's,
; and both spellings are here because `tree-sitter query` refused the wrong one.
(field
  (identifier) @name) @item

(member
  name: (identifier) @name) @item

; A doc comment annotates the entry that follows it, which is what lets Zed show the prose beside the
; name (ADR-0027 §5 made `///` distinct from an aside).
((line_comment) @annotation
  (#match? @annotation "^///"))

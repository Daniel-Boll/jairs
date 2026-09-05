; Bracket matching, a Zed feature with no Neovim counterpart in this repository.
;
; `@open`/`@close` drive both the matching highlight and the rainbow colouring.

("{" @open "}" @close)
("(" @open ")" @close)
("[" @open "]" @close)

; **No rule for a string's quotes**, and that is the grammar's shape rather than an omission: a
; `string_literal` is a single token here, so `"` is not a node and a query naming it fails to compile
; with `Invalid node type "\""`. Found by running `tree-sitter query` over this file, which is the
; check ADR-0025 §4 added for exactly this class of mistake.

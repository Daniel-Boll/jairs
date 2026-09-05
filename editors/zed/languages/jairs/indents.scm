; Jairs indentation, in Zed's dialect.
;
; Not generated from `tree-sitter-jairs/queries/indents.scm`, and the reason is that the two dialects
; disagree about more than spelling. Neovim's captures a node as `@indent` and each closing token as
; `@dedent`; Zed's `@indent` names a *range* and `@outdent` ends the innermost one at the start of the
; captured node, so translating token-for-token would indent one level too many at every closer.
;
; Written rather than translated, and small enough to read at a glance. The nodes are the same four
; the Neovim query names, so a grammar change that renames one breaks both — and gate 6 runs
; `tree-sitter query` over this file for exactly that reason (ADR-0199 §12).

[
  (block)
  (field_list)
  (param_list)
  (arg_list)
] @indent

; The closing token ends the range, rather than each closer dedenting on its own.
[
  "}"
  ")"
] @outdent

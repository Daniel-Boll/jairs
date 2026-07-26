# ADR-0010: Hand-written compiler parser; tree-sitter is editor-only

- **Status:** Accepted
- **Date:** 2026-07-25
- **Deciders:** dboll

## Context

There are two independent audiences for "parse Jairs": the *compiler*, which
needs a typed tree it can hang analysis on and diagnostics good enough to be the
LSP's error quality; and *editors*, which need fast, incremental,
recovery-tolerant highlighting. It is tempting to serve both with one grammar.
The candidates for the compiler's frontend:

1. **tree-sitter as the compiler frontend.** One grammar for everything. But
   tree-sitter produces an *untyped* CST with nowhere to attach type
   information, its error recovery is tuned for keeping highlighting alive rather
   than for producing good diagnostics, and it pulls in a C dependency.
2. **A parser generator.** Removes hand-written boilerplate, but delivers worse
   error recovery than a hand-written recursive-descent parser — and error
   recovery is precisely where LSP diagnostic quality comes from.
3. **A hand-written, error-recovering recursive-descent parser**, with
   tree-sitter kept as a *separate* editor-only grammar.

The two-grammar arrangement has one obvious risk — the grammars drift — and
`PLAN.md` §5 names it. The mitigation is a shared corpus and a CI gate.

## Decision

The compiler's parser is **hand-written, recursive-descent, and
error-recovering**, producing a lossless `rowan` CST with typed AST accessors.
`tree-sitter-jairs` is a **separate, editor-only grammar** used for
highlighting, never by the compiler. Agreement between the two is enforced by the
shared `tests/corpus/` and the `corpus-drift` CI job: every `valid/` file must
parse with zero errors in the compiler *and* produce zero `ERROR` nodes in
tree-sitter, and every grammar change requires a corpus file.

## Consequences

### Positive

- The compiler owns a typed tree it can attach types and analysis to, and error
  recovery tuned for *diagnostics* — which is the source of LSP error quality.
- No C dependency in the compiler's parse path.
- Editors still get fast tree-sitter highlighting.

### Negative

- Two grammars exist and must be kept in agreement — genuine ongoing cost.
- The recursive-descent parser and its recovery are hand-maintained rather than
  generated.

### Follow-on work this forces

- **Into the slice:** the hand-written lexer + parser + rowan CST + typed AST live
  in `jr-syntax`; `tree-sitter-jairs` is built in parallel; the shared corpus and
  the `corpus-drift` CI gate exist from Jairs-0. Every future grammar change, in
  every wave, must add a corpus file (`tests/corpus/README.md`).

## Alternatives considered

- **tree-sitter as the compiler frontend.** Rejected: an untyped CST has nowhere
  to hang type information, its recovery is tuned for highlighting rather than
  diagnostics, and it introduces a C dependency into the compiler.
- **A parser generator.** Rejected: worse error recovery than a hand-written
  recursive-descent parser, and error recovery is exactly where LSP quality is
  won or lost.

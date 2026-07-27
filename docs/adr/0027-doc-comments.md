# ADR-0027: `///` and `//!` doc comments

- **Status:** Accepted
- **Date:** 2026-07-27
- **Deciders:** dboll

## Context

Hover reported `(s64, s64) -> s64` for a procedure and nothing else. Comparing it against
rust-analyzer, which shows a container path, a full signature and a description, three of
those four lines were reconstructible from what the compiler already retains — and the
fourth was not, because **Jairs has no doc comments.**

What exists today: `LINE_COMMENT` and `BLOCK_COMMENT` are lexed as trivia, kept in the
CST, and preserved by `jr fmt`. Nothing in `jr-hir` or below sees a comment at all. The
`Basic` module already writes prose above every declaration (`// Write a string to standard
output.`), so the *content* is there; the association is not.

Three candidate sources were weighed. Harvesting the leading `//` block from the CST would
have worked on the existing corpus with no language change, but it makes every comment
above a declaration load-bearing — including the ones that are asides rather than
documentation, of which `modules/Basic/module.jr` has several dozen lines. The decision
was to add the language feature and be able to say which comments are documentation.

## Decision

### 1. `///` and `//!` are distinct trivia kinds

Two new `SyntaxKind`s, `DOC_COMMENT` and `MODULE_DOC_COMMENT`, both in the trivia class.
`SyntaxKind::is_trivia` returns true for them, so **the parser does not change** and no
grammar rule can require or forbid one.

`////` (four or more slashes) stays `LINE_COMMENT`, following Rust: a row of slashes is a
visual rule, and this file's own section dividers use one. `//!` documents the enclosing
module, which is what `Basic`'s header block already is in intent.

**Rejected: keep `LINE_COMMENT` and check for three slashes at each point of use.** No
lexer change, and one fewer kind. Rejected because the distinction would then live in
whoever remembers to check, re-derived independently in `jr-fmt`, `jr-db` and `jr-lsp`.
This project's recurring bug is a construct with no representation on some path; a kind
that exists in the lexer's output is a representation.

**Rejected: a non-trivia token the parser attaches to a declaration.** Ownership would be
structural in the CST rather than inferred, and a misplaced doc comment would be a parse
error. Rejected for the cost: a grammar rule at every declaration position, new error-
recovery paths, and a matching change to `grammar.js`, in exchange for rejecting a comment
in the wrong place — which §3 decides not to reject at all.

### 2. Attached text lives in a side table with its own query

`jr_db::file_docs(db, file) -> Arc<FileDocs>`, mapping `ItemId` to doc text. It depends on
`parse_file` for the trivia and `file_hir` for item spans, and **nothing depends on it but
the language server.**

`Item` already carries `span` (the whole item) and `name_span`, and every named
declaration — including a procedure, whose `ItemKind` is a `Const` holding a `ConstValue` —
is an `Item`. So one table keyed by `ItemId` covers procedures, structs, constants and
variables without a per-kind field.

**Rejected: a `docs: Option<Symbol>` field on each HIR item.** One fewer query. Rejected
because it puts prose inside the structure the type checker consumes: `jr-sema` would be
able to read documentation, and every future item kind has to remember the field. The
layering statement worth being able to make is that documentation is not part of the typed
program.

**Rejected: no compiler representation — the LSP walks the CST itself.** Rejected because
the association rule ("which item does this comment belong to") would live in the editor
layer, and a future `jr doc` would have to reimplement it and could disagree.

### 3. A `///` that precedes nothing is silently ignored

A doc comment inside a body, or before a closing brace, is not attached and not reported.
No diagnostic, so **E0231 is still the first free code.**

**Rejected: a warning.** It would catch documenting the wrong side of a brace. Rejected
because it is the first diagnostic Jairs would emit about prose, and the cost — a code to
keep true forever — is out of proportion to a mistake the hover card makes visible anyway
(the docs are missing from where you expected them).

**Rejected: an error.** A comment able to fail a build, and error recovery would have to
invent a position.

### 4. `jr fmt` must handle the new kinds explicitly, and this is the risk

Every one of `jr-fmt`'s six comment sites matches `LINE_COMMENT | BLOCK_COMMENT` and ends
in `_ => {}`. Adding a kind without touching them would make the formatter **silently
delete every doc comment** — a construct the grammar allows, with no representation on the
formatting path, falling into a catch-all that is a legitimate branch for the tokens it was
written for. That is precisely the failure mode `AGENTS.md` describes twice.

So `SyntaxKind::is_comment()` is added and the six sites are converted to it, rather than
each growing two more arms. A round-trip test formats a file whose every declaration is
documented and asserts the doc comments survive, because `--check` passing on a corpus
that has no doc comments would prove nothing.

### 5. Highlighting gets a capture, but the grammar does not change

`///` is already a line comment to tree-sitter, so `grammar.js` and the corpus parse are
unaffected. `queries/highlights.scm` gains a `@comment.documentation` capture predicated on
the text, so a doc comment reads differently from an aside. Gate 6's query validation
covers it.

## Consequences

### Positive

- Hover can show a description, which is what prompted this.
- The distinction between documentation and an aside is expressible, so `Basic`'s long
  explanatory blocks stay asides rather than becoming API documentation by position.
- A future `jr doc` has a query to read and an association rule it does not own.

### Negative

- Two more kinds in a class that had three, and six formatter sites converted to a helper
  they did not need before.
- Every `///` is one more thing that can rot relative to the code beside it, and nothing
  checks that it is true. Doc tests are not in Jairs-0 and are not planned.
- The corpus's existing prose does **not** become documentation. Turning the useful parts
  of `Basic`'s comments into `///` is a manual edit, done in this wave for the declarations
  hover will be asked about and not for the rest.

### Follow-on work this forces

- **A doc-comment section in `docs/spec/01-lexical.md`**, since this is a lexical change and
  chapter 01 is the record of what the lexer accepts.
- **`jr doc`**, eventually, or the decision that Jairs has no documentation generator. Not
  in the slice, and not scheduled.

# ADR-0031: code actions, signature help, and inlay hints

- **Status:** Accepted
- **Date:** 2026-07-27
- **Deciders:** dboll

## Context

The server has nine capabilities and every one of them *answers a question*. None of them
**offers to change the code**, and none of them tells the user something the source does
not already say. Those are the two gaps this wave closes, and they are one wave rather
than three because they share the same two pieces of machinery: `locate`, which turns a
cursor into a HIR node, and `render`, which turns a declaration into text.

What already exists and is relied on here:

- `ResolveMap` maps every `Expr::Name` to a `Res` — and, crucially, **only** an
  `Expr::Name` (§2 below is entirely about that word "only").
- `WorkspaceFiles` (ADR-0029) enumerates the `*.jr` files that exist.
- `DefId` and the reference scan (ADR-0030) find a declaration and everywhere it is used.
- `TypeMap` holds the type of every expression and local; `file_consts` holds the value of
  every constant a `#run` produced.
- `Decl::card` is the one renderer, and ADR-0028 §1 forbids a second.

`PLAN.md` §7 listed the pieces of this wave as a bulleted wish. Two of them turned out to
rest on a claim that is false, which §2 and §6 record rather than quietly work around.

## Decision

### 1. A suggestion lives in the diagnostic, not in the code action

"Did you mean `y`?" for E0218 (no such field) and E0212 (unknown type name) is computed
where the error is *raised* — in `jr-sema` — and attached as a `help:` note. The code
action reads the note back off the diagnostic it was invoked on.

This is ADR-0007's claim applied one more time: the LSP is a *consumer* of the compiler's
analysis, never a second front end. The alternative puts the "what did they probably
mean" logic in `jr-lsp`, where `jr check` cannot reach it — so a user compiling on the
command line gets `no field 'y' on type 'Point'` and a user in an editor gets a
suggestion, from two implementations of the same guess that would drift the first time one
of them learned about case-insensitivity.

**Rejected: compute candidates in the code-action handler.** Cheaper — no sema change, no
re-snapshotting the type-error corpus — and self-contained. Rejected because it makes
`jr check` permanently worse than the editor at explaining the same error, and because the
candidate set *is* semantic information: it is "the fields this type has", which only the
checker knows.

The metric is Levenshtein distance, bounded at `max(1, len / 3)` and capped at 2, with the
single nearest candidate offered and ties broken by declaration order. One suggestion, not
a list: a `help:` line offering three alternatives is a line the reader has to think
about, and rustc's practice here is worth copying because it was arrived at by complaint.

### 2. An unused import cannot be decided from `ResolveMap`, and that is a trap

`ResolveMap` covers `Expr::Name` and nothing else. A **type** annotation is a
`TypeRef::Name`, resolved separately by `jr-sema`'s `resolve_type_name`, and it never
appears in the resolve map at all. So this file:

```jr
#import "Shapes";

main :: () {
    r: Rect;          // `Rect` is a TypeRef::Name — invisible to ResolveMap
    r.w = 3;
}
```

has an import that a `ResolveMap`-only check calls unused. That is
`tests/corpus/imports/valid/001-import-directory-module.jr`, which exists today, so the
naive implementation would have shipped a warning telling the user to delete an import
their program needs — and the quick fix beside it would have broken the build on one
click.

Therefore: **`jr-sema` records which import supplied each resolved type name**, as a new
`FileSignatures::type_name_imports` side table, and the unused-import query is the union
of the resolve map's `Res::Imported` and that table.

**Rejected: re-derive type-name resolution inside the query.** It would mean a second copy
of ADR-0014 §3's shadowing order — builtins, then this file, then imports — and a second
copy of a resolution order is exactly the drift ADR-0022 §2 refused for arithmetic. A
divergence would present as a spurious unused-import warning, which is a wrong answer the
user is invited to act on.

What is implemented is the conservative direction: an import is reported only when it
contributes no name the file actually *uses*, in either position.

Two corpus files are reported by this rule, and both are correct rather than tolerated:

- `004-local-shadows-import.jr` imports `Colors` and then declares its own `blend`, so the
  import contributes nothing — the use resolves to the local. The warning is actionable and
  removing the import changes no meaning, which is exactly the property a quick fix needs.
- `006-duplicate-import-is-idempotent.jr` gets the warning on the **second** `#import
  "Colors";` only. A duplicate is idempotent (ADR-0014 §6), so the second line is the one
  that does nothing, and it is reported whether or not the name is used.

Where this stays silent on purpose: an import that failed to resolve (E0210 already said
so, and a second complaint about one problem points at the wrong fix), and a self-import
(ADR-0014 §6 skips it everywhere else; "unused" would be true and useless).

### 3. Unused imports are a warning, at E0231

A new `unused_imports` query in `jr-db`, raised as a `Severity::Warning` under **E0231**,
the first free code. `jr check` reports it; the code action removes it.

**Rejected: the code action with no diagnostic.** Most clients only surface a lightbulb
where a diagnostic already sits, so an action with nothing under it is nearly
undiscoverable — the user would have to already suspect the import was unused.

**Rejected: silence, on the grounds that Jai does not warn about unused imports.** This is
a genuine language-design position and not merely plumbing, and it is being taken
knowingly: Jairs warns. The reason is ADR-0014's flat merge. An unused import in a language
with qualified paths costs a line; in Jairs it silently enlarges the name space every
identifier in the file resolves against, and can turn a later declaration into an E0211
ambiguity from a module the file does not use. That makes it a correctness hazard rather
than untidiness.

It is a warning and not an error because a file mid-edit legitimately has one.

### 4. Every code action is offered from a diagnostic it can point at, except two

The set:

| Action | Trigger | Kind |
|---|---|---|
| `#import "M";` for an unresolved name | E0201 | `quickfix` |
| Remove this unused import | E0231 | `quickfix` |
| Remove all unused imports in this file | E0231 | `source.organizeImports` |
| Did you mean `<field>`? | E0218 | `quickfix` |
| Did you mean `<type>`? | E0212 | `quickfix` |
| Give this procedure a body | E0203 | `quickfix` |
| Make this comment documentation (`//` → `///`) | cursor on a `//` above a declaration | `refactor.rewrite` |

The last is the one with no diagnostic, which is why it is a `refactor` rather than a
`quickfix`: nothing is wrong with an ordinary comment. It is offered only when the comment
is immediately above a *named* declaration, because that is the only place `///` means
anything to `file_docs` (ADR-0027 §2).

An action's edit is always a `TextEdit` on a span the compiler produced. No action
re-parses, re-indents, or reflows: the formatter owns that, and an action that formatted
would be a second formatter.

### 5. Auto-import parses discovered modules at request time

Answering "which module exports `print`" means having the exports of every discovered
module, and ADR-0029 deliberately yielded *paths*, not loaded files. So the handler loads
and parses the discovered modules on the request, exactly as ADR-0030's rename does, and
for the same stated reason: the cost lands on the caller who asked, not on every keystroke.

This is now the **fourth** claimant on the latency measurement `PLAN.md` §7 has carried as
"smaller, also open" for three waves. It is no longer smaller. Recorded here so the next
wave cannot treat it as new information.

**Rejected: offer only from already-loaded modules.** Free, and wrong in the common case —
a name from a module the editor has not opened yields no offer at all. That is the exact
"confident wrong answer" shape this project hit in three consecutive waves (the empty
workspace in ADR-0030's implementation, the canonicalising walk, the missing
`workspaceFolders`), and it is the one failure mode a quick fix must not have, because the
absence of an offer is indistinguishable from "there is nothing to import".

**Rejected: an exported-name index keyed on `WorkspaceFiles`.** Fastest at request time,
and the honest long-term answer. Rejected *now* because it is the reverse index ADR-0030
declined to build without measurement, arriving through a side door — and because
inverting it correctly requires deciding what invalidates it, which is the same question
the latency measurement is supposed to inform.

Only a module that **actually exports the name** is offered, and where several do, all are
offered as separate actions rather than one guess.

### 6. `signatureHelp` needs the call, not the cursor's innermost node

`locate` returns the *innermost* expression containing the offset, which inside `add(2, |)`
is the argument, or nothing at all when the cursor sits on whitespace between the comma
and the paren. Signature help needs the **enclosing call** and the index of the argument
the cursor is in. Neither is a narrowing scan.

So a new `enclosing_call` walks the expression arenas for the innermost `Expr::Call` whose
span contains the offset, and computes the active parameter by counting the argument spans
that end before the cursor. The counting is deliberately span-based rather than textual:
the buffer usually does not parse mid-call, but the arguments already typed do, and their
spans are what lowering recorded.

`activeParameter` is clamped to the last parameter rather than left out of range, so that a
call with too many arguments still highlights something — the alternative is a client
showing no highlight at all at the moment the user most needs to see which parameter they
have overrun.

### 7. Inlay hints show only what the source does not say

Two kinds, and no more:

- **`:=` type hints** — `n := add(2, 3)` gets `: s64` after the name, from `TypeMap`. Not
  shown when the local has an explicit annotation, because the type is already on screen.
- **`#run` value hints** — `COMPUTED :: #run add(2, 3)` gets `= 5`, from `file_consts`.

The second is the one nothing else in this ecosystem can offer, and it is the reason inlay
hints are in this wave rather than the next: it makes compile-time execution *visible*,
which is a claim `PLAN.md` §1.4 has been able to assert only through a MIR snapshot.

A hint is never emitted for a type that renders `<unknown>`. A hint saying `: <unknown>`
is noise that looks like a compiler bug, and the absence of a hint already means "nothing
useful is known".

Hints are computed for a requested **range**, as the protocol intends, so a large file does
not render its whole body on every scroll.

## Consequences

- `jr check` gains a warning class it did not have. Every corpus file with an import is now
  a test of §2's correctness: three of them use an imported name in type position only, and
  they must stay silent.
- The type-error corpus snapshots change, because E0218 and E0212 now carry a `help:` line
  where a near-miss exists. That is a wanted change, and reviewing the snapshot diff is how
  the suggestion quality gets checked at all.
- `FileSignatures` grows a side table, so it grows in every file's memoised signature. It
  holds one entry per resolved type-name-from-an-import, which is bounded by the number of
  annotations in the file.
- **Nothing here works on a type annotation's own span.** `TypeRef::Name` still carries no
  span (`jr_hir::TypeRef`), so the E0212 quick fix replaces the range the *diagnostic*
  points at — which the diagnostic gets from the enclosing declaration. That is why §1's
  suggestion is attached to the diagnostic rather than computed from a cursor position: the
  cursor cannot find a type annotation, and pinning that is
  `hovering_a_type_annotation_returns_nothing_today`'s job.
- The latency question is now blocking four features rather than three (§5).

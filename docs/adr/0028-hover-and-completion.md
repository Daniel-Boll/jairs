# ADR-0028: the hover card, and completion

- **Status:** Accepted
- **Date:** 2026-07-27
- **Deciders:** dboll

## Context

Hovering a procedure produced `(s64, s64) -> s64` and nothing more. Not a bug in the
renderer: `handlers::hover` never consults the declaration. It takes the offset, finds the
expression, asks `checked`'s `TypeMap` for that expression's **type**, and renders the type.
For a procedure the type *is* its signature shape, so the name, the parameter names and the
origin were never in scope to be lost.

Meanwhile `goto_definition`, twenty lines below in the same file, already walks `resolved`'s
`ResolveMap` to a declaration — `Res::{Local, Param, Item, Imported, Error}` — and already
follows `Res::Imported` into another file. Hover had the harder half of its job already
written next to it.

Completion did not exist at all: no capability advertised, and `PLAN.md` §2.1 assigned it to
wave W9. It is pulled forward into this wave because a completion item wants exactly the
renderer a good hover card needs, and building them apart guarantees two renderings that
drift.

What the compiler retains, checked rather than assumed: `Param` carries `name` and
`name_span`; `Proc` carries `params` and `ret`; `Item` carries `name`, `span` and
`name_span`; modules are named by directory and are flat, one segment. What it does not
have: any notion of visibility. There is no `pub` in Jairs — every top-level item is what
an importer may see, which is what `FileSignatures` means.

## Decision

### 1. One renderer, three consumers

`jr-lsp` gains `render.rs`, producing a declaration card from `(ItemId, FileHir, FileDocs,
TypeMap)`. Hover renders it, a completion item's `detail` and `documentation` render from
it, and `completionItem/resolve` renders the expensive half of it. There is no second
formatting path.

**Rejected: let each handler format its own.** Shorter for the first handler and wrong by
the third. The specific failure this avoids is a completion list and a hover disagreeing
about the same procedure's signature, which is invisible in tests written per handler.

### 2. The card is Jairs syntax, not Rust's

```
Basic
─────
print :: (s: string)

Write a string to standard output.
```

rendered as a `jr` code fence holding container and signature, a `---` rule, then the doc
text as markdown.

The signature line is a **Jairs declaration**: `print :: (s: string)`, not
`pub fn print(s: string)`. There is no `fn` and no `pub` to show, and inventing them would
make the card a description of a language this is not. A procedure with no return type
shows none, because Jairs writes none.

**Rejected: mimic the `pub fn` form for familiarity.** Rejected outright. A hover card is
documentation of the program in front of you.

### 3. The container line is the module, or the file stem

`Basic` for an imported item; `024-hello` for one declared in the file you are in. Always
present, so the card does not change shape with the item's origin.

Jairs modules are flat, so this is one segment where Rust's is a path. That makes it a
weaker signal, which is why the file stem is shown for local items rather than nothing: it
tells you which file goto-definition would take you to.

**Rejected: omit it for same-file items.** Quieter, but a card whose line count depends on
provenance is harder to read at a glance than one with a constant shape.

### 4. Hover resolves a declaration first, and falls back to the type

Order: resolve the name under the cursor to a declaration and render the card; if the
cursor is not on a name — `4 + 5`, a literal, an operator — render the type as today.

Rendered per kind: a procedure gets its signature; a struct gets `Point :: struct { x: s64;
y: s64 }`; a constant gets its type and, when computed, its value; a parameter or local
gets its declared type.

Hovering a declaration's **own name** works too, which it did not before — and getting
there required a second lookup that this ADR originally failed to notice was needed.
[`locate`] scans expression arenas, and the `add` in `add :: (a: s64)` is an
`Item::name_span`, not an `Expr::Name`; the same is true of a parameter's name and a
local's. So there is now `locate_declaration`, consulted **only when `locate` answers
nothing**, returning a `DeclSite` of `Item`, `Param` or `Local`. Name spans and expression
spans do not overlap, so the order is a preference rather than a conflict: where a name is
*used*, following it to what it means is the better answer than describing where the
cursor is.

This is the hole `verify.lua`'s first draft had asserted as correct: it hovered `sum` in
`sum := add(…)`, got nothing, and made the nothing the expected value.

**Rejected: type first, declaration only as a fallback.** Preserves today's behaviour for
every existing case. Rejected because the type is the less informative answer whenever
both exist, which is every name.

**A type annotation still gets no hover, and that is a HIR limitation rather than a
choice.** `jr_hir::TypeRef::Name` carries a `Symbol` and no `Span`, so there is nothing to
match a cursor inside the `Point` of `p: Point` against — no scan can succeed, however it
is written. Giving `TypeRef` a span is a `jr-hir` change with its own ripples (three
arenas hold type references, and `Proc::type_refs` is documented as always empty pending
exactly that refactor), so it is recorded as owed work and pinned by a test that fails the
day it starts working.

### 5. Completion ships the full surface, including snippets and resolve

Advertised with trigger characters `.` and `#`. Sources, in order: locals and parameters in
scope, then file items, then imported module items, then keywords and builtin types. After
`.`, the receiver's checked type gives struct fields. After `#`, the directives.

A procedure completes as a **call snippet** with placeholder parameters, using the names
`Param::name` already holds. `completionItem/resolve` supplies documentation lazily, so the
list stays cheap when a module is large.

**Rejected: no snippets.** Safer — a snippet guesses that you want a call, and editors cache
completion items. Accepted anyway because the parameter names are real rather than invented,
and a call is what a procedure name is followed by in every case Jairs-0 can express (there
are no procedure values yet; calling through a procedure pointer is `Unsupported` in both
engines).

**The trap this creates:** resolve is a second code path over the same item, so a resolved
item can disagree with the one in the list. §1's single renderer is the mitigation, and a
test asserts the resolved documentation equals what the renderer produces directly.

## Consequences

### Positive

- The four lines that prompted this — container, signature with parameter names, rule,
  description — all render, from data the compiler already had.
- Hovering a declaration works, closing a hole a test had been asserting as correct.
- Completion exists a wave earlier than planned, sharing the renderer rather than growing a
  parallel one.

### Negative

- Two new capabilities means two new ways for the server to be slow, and neither has a
  latency number. ADR-0013's `AstIdMap` trigger — keystroke-to-diagnostic — now has a
  second reason to be measured: completion runs the same O(nodes) offset scan per keystroke,
  and a completion request happens far more often than a hover.
- Snippets are a guess about intent that editors cache, and reversing the decision later
  will not un-cache them.
- The container line will be wrong the day Jairs grows nested modules, and it will be wrong
  quietly, as a cosmetic line rather than a failure.

### Follow-on work this forces

- **`jr_hir::TypeRef` needs a span**, or hover and goto-definition will never work on a
  type annotation. §4 records why no amount of care in `jr-lsp` can substitute.
- **`signatureHelp`** is now an obvious gap rather than an absent feature: once a call
  snippet has placed the cursor inside `add(|)`, the parameter list is what you want next.
- **A latency measurement**, per ADR-0013, before deciding whether the span scan survives.
- **The VS Code extension inherits both capabilities** and must not reimplement the
  renderer.

## Postscript: two things this ADR got wrong before the code existed

Recorded here rather than silently corrected, because the pattern is now the project's
most reliable one — four consecutive waves have had an ADR's rationale falsified by
running something.

1. **§4's claim that hovering a declaration "works"** was written as though resolving a
   name covered it. It does not: a declaration's name is not an expression, and the
   feature needed a whole second lookup (`locate_declaration`) that this ADR did not
   mention. Corrected in §4 above before this ADR was committed.
2. **Nothing in this ADR anticipated the deadlock.** `jr-lsp` reads the type pool behind a
   `Mutex`, and a query locks that same pool; holding the lock across a query call is
   therefore a self-deadlock. The first `completion` implementation did it and hung the
   test run with no output — which is what a deadlock looks like from outside, and is far
   less legible than a panic. The rule ("every query call happens before the pool is
   locked") is now a comment at the site and a trap in `PLAN.md` §7.

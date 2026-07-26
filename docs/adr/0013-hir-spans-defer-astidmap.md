# ADR-0013: HIR nodes carry spans; `AstIdMap` is deferred

- **Status:** Accepted
- **Date:** 2026-07-25
- **Deciders:** dboll

## Context

Every HIR node needs a way to point back at source, so that `jr-sema` can say
"expected `s64`, found `string`" and underline the offending expression, and so
the language server can map a type error to a range in the editor.

There are two established ways to do this.

**Store a `Span` on every HIR node.** Simple, direct, and every diagnostic is a
field access away.

**Store a stable `AstId` and look the span up on demand** — rust-analyzer's
approach. A per-file `AstIdMap` assigns each syntax node an identity that is
stable under edits *elsewhere* in the file, so HIR does not change when
unrelated text moves.

The difference matters because of ADR-0007. Under salsa, a query is invalidated
when its inputs change. If HIR embeds absolute byte offsets, then inserting a
single space at the top of a file changes the span of **every** node below it,
so the HIR value changes, so everything downstream — resolution, types, and
eventually codegen — is invalidated. The edit was semantically empty, but the
whole file is re-analysed.

With an `AstIdMap`, that edit changes the map and nothing else: HIR compares
equal, and salsa's backdating stops the invalidation at the boundary.

## Decision

**For the Jairs-0 slice, HIR nodes store `Span` directly. `AstIdMap` is
deferred.**

The tradeoff is recorded in `crates/jr-hir/src/lib.rs` as a `TODO(AstIdMap)`
note so it cannot be mistaken for an oversight.

Revisit when either becomes true:

1. Incremental re-analysis cost is **measured** and whitespace-edit invalidation
   is a real cost — not assumed to be. The natural trigger is the language
   server (wave W9), where keystroke latency is directly observable.
2. HIR grows large enough that re-lowering a file is expensive. At the slice's
   scale — files of a few hundred lines and no type checking yet — re-lowering
   is microseconds.

Nothing about storing spans now makes the migration harder later: it is a
mechanical change from a `Span` field to an `AstId` field plus a lookup, and the
diagnostic construction sites are the only consumers.

## Consequences

### Positive

- Diagnostics are trivial to construct, which matters while the whole point of
  the front end is diagnostic quality.
- `jr-hir` stays a pure function of the syntax tree with no side table to thread
  through, keeping it testable without a database (ADR-0007 keeps salsa at the
  edge).
- No `AstIdMap` to design, maintain, or get subtly wrong before there is any
  evidence it is needed.

### Negative

- **A whitespace-only edit invalidates a file's entire downstream analysis.**
  This is the real cost and it is accepted knowingly.
- Salsa cannot backdate HIR, so `no_eq`-style coarse invalidation is the norm
  for now.
- The eventual migration touches every HIR node type, even if each change is
  mechanical.

### Follow-on work this forces

- Wave W9 (tooling depth) must measure keystroke-to-diagnostic latency before
  declaring the language server acceptable; that measurement is what decides
  whether this ADR gets superseded.
- Any future `AstIdMap` work must preserve `jr-hir`'s purity — the map is an
  input to lowering, not a global.

## Alternatives considered

**Build `AstIdMap` now.** Rejected as premature. It is real complexity bought
against an unmeasured cost, at a stage where no type checker, no code generator,
and no language server exist to benefit. rust-analyzer needed it because it
operates on multi-million-line workspaces interactively; we currently operate on
a 25-file corpus in a batch driver.

**Store no location in HIR and re-derive it from the syntax tree by position.**
Rejected: it requires HIR nodes to keep a pointer back into the tree anyway, and
recovering "which syntax node produced this HIR node" after the fact is exactly
the problem `AstIdMap` solves properly. This would be the worst of both.

**Store spans only on nodes that can be blamed in a diagnostic.** Rejected as a
false economy — practically every expression can appear in a type error, so the
"blameable" set is nearly everything, and guessing wrong means an awkward
retrofit for the one node kind that turns out to need it.

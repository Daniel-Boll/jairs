# ADR-0007: salsa from the first slice; the LSP is a consumer of the same queries

- **Status:** Accepted
- **Date:** 2026-07-25
- **Deciders:** dboll

## Context

The single most common way language projects end up with a bad IDE experience is
building a batch compiler first — one that assumes "parse everything, then check
everything" — and bolting on IDE support later. The batch model and the IDE model
are fundamentally different: an IDE must be *incremental* (re-check only what
changed) and *error-tolerant* (produce useful results from broken code), and a
batch compiler is architected for neither. Retrofitting incrementality means, in
practice, writing a second frontend, and now two frontends drift.

`PLAN.md` §5 lists "LSP as a fork of the compiler" as a standing risk with
exactly this failure mode. The alternative is to make the compiler *itself*
incremental and on-demand from the start, and have the LSP be a thin consumer of
the same queries.

## Decision

`salsa` is adopted from the **first slice**, not added later. There is exactly
**one frontend**: file → tokens → CST → HIR → types are all salsa queries, and
the LSP is a *consumer* of those same queries — it never re-implements analysis.
Analysis is lazy and on-demand as a direct consequence.

The `salsa` version is pinned exactly (see ADR-0009); the query database lives in
`jr-db` and is shared by the batch driver and the language server.

## Consequences

### Positive

- The LSP and the batch compiler cannot disagree, because they are literally the
  same queries; there is no second frontend to drift.
- Lazy, on-demand analysis is available from day one — which is also precisely
  what makes wave W4's mutually-recursive Sema↔comptime tractable, since a pass-
  ordered checker cannot express "types need `#run` which needs types".
- Incremental recompute is a property of the architecture, not a later project.

### Negative

- Every analysis must be written as a salsa query with tracked inputs, which is a
  stricter discipline than free-form passes and constrains how state is threaded.
- `salsa`'s API is not semver-stable, so the pin (ADR-0009) is mandatory and
  upgrades are deliberate work.

### Follow-on work this forces

- **Into the slice:** `jr-db` and the query graph exist in Jairs-0; the LSP
  (`jr-lsp`) is built as a query consumer from the start, and Sema is written
  lazy/on-demand rather than as ordered passes.
- **Into wave W4:** the lazy query model is the prerequisite for the
  mutually-recursive Sema/comptime cycle and its cycle-detection diagnostics.

## Alternatives considered

- **Batch compiler now, IDE support later.** Rejected explicitly: `PLAN.md` §5
  names this as a known failure mode. It produces two frontends that drift, and
  it forecloses the lazy on-demand analysis that W4's comptime recursion depends
  on. Paying the salsa discipline cost up front is cheaper than the rewrite.

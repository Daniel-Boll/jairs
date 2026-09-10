# ADR-0240: Dynamic-array completion exposes its three semantic pseudo-fields

- **Status:** Accepted
- **Date:** 2026-09-10
- **Decider:** dboll

## Context

ADR-0136 defines `[..]T` as a compiler-known, caller-owned three-word value with three public,
writable pseudo-fields:

- `data: *T`
- `count: s64`
- `capacity: s64`

Sema accepts all three. LSP field completion separately dispatches on the receiver's pool type and
handled `string`, fixed arrays, views and nominal structs, but not `DynamicArrayType`. Consequently
the exact working stack shape from ADR-0231:

```jr
stack: [..]*Node;
stack.
```

offered no members.

## Decision

`jr-lsp::fields_at` gains the missing `DynamicArrayType` arm. It constructs `.data` from the
dynamic array's concrete element type, so `[..]*Node.data` is described as `**Node`, and offers
`.count` and `.capacity` as `s64`.

At an incomplete `stack.`, checking has resolved the element type but need not have interned the
pointer type represented by `.data`: no field name exists yet to make sema ask for it. Completion
therefore renders the detail as `*` plus the existing element-type spelling. It does not mutate the
shared type pool merely to describe a candidate.

The regression test drives completion through an incomplete `stack.` expression, matching an
editor request rather than testing a lower-level helper.

## Rejected alternative

### First centralise every field-like surface shared by sema and LSP

Rejected for this repair. A canonical field-enumeration seam is desirable because completion
currently repeats part of sema's receiver dispatch, but building it now would also force decisions
about vectors, unions, variants, `Context`, and `using`-promoted fields. That is the broader
canonical-completion wave already queued in `PLAN.md`, not a prerequisite for restoring the three
fields ADR-0136 already made public.

### Eagerly intern `*T` whenever `[..]T` is interned

Rejected because the compiler does not need that pointer merely to represent a dynamic array, and
completion is a read-only query over an incomplete expression. Changing the pool's construction
invariant for display would hide the actual requirement: an editor must be able to describe a
candidate before the program has selected it.

## Consequences

- `stack: [..]*Node; stack.` offers `capacity`, `count`, and `data`.
- Completion detail preserves the element type: `data` is `**Node` in that example.
- No parser, type checker, MIR, engine, or protocol surface changes.
- The duplicated sema/LSP field inventory remains an explicit optimisation-queue item.

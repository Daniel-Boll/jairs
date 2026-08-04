# ADR-0106: `size_of`, `typed` and `untyped` make heap storage reachable — without widening `cast`

- **Status:** Accepted
- **Date:** 2026-08-04
- **Deciders:** dboll
- **W7 sub-wave 4.** ADR-0105 named **typed allocation** as the first of three things blocking a real dynamic
  array, and said it was a language decision rather than a library's. This is that decision. It also fixes a
  **pre-existing miscompile** in store-to-load forwarding that only this feature could reach.

## Context

`malloc` returns `*u8`; `cast(*s64, p)` is **E0232**, because a general pointer cast makes a wrong pointee type
a *silent wrong read* (ADR-0045 §1). So heap storage was unreachable, and the refusal is right and should stay.
What was missing was a way to get a **typed** pointer to fresh memory that is not a general cast.

## Decision

### 1. Three intrinsics: `size_of(T)`, `typed(T, p)`, `untyped(p)`

```jai
p := malloc(n * size_of(s64));
d := typed(s64, p);          // *u8 -> *s64
(d + 1).* = 20;              // ordinary pointer arithmetic and store
free(untyped(d));            // *s64 -> *u8
```

**The library allocates and only the *retyping* is an intrinsic**, and that split is an amendment made while
building. The original plan was a single `alloc(T, n)` — but **MIR has no way to reach `malloc`**: a `#foreign`
procedure is resolved by name in *its own file's* signatures, and the builder has no channel for "call this
library procedure I invented". The split is better anyway: `Basic.malloc` keeps doing what it already does, and
the language contributes exactly the one thing a library cannot express.

**`size_of(T)` folds in sema** from `layout_of` — the same function `type_info(T).size` uses, so the two cannot
disagree about how large a type is. It arrives now because typed allocation *asked for it*: nothing could name
`n * size_of(T)` before, and a facility with no caller is what ADR-0080 §3 declined to build.

**Why an intrinsic rather than relaxing `cast` for `*u8` → `*T`.** The relaxation permits exactly the
wrong-pointee read ADR-0045 §1 refused — a `*u8` may point at anything — and the narrowness of the hole does not
change what goes through it. What `typed` adds is not *safety*: `typed(s64, p)` on a four-byte allocation is
still wrong. It adds **visibility**: the target type is a type *argument* at a named boundary a reader can grep
for, exactly as ADR-0076 §1 permitted an erasing conversion only at an `Any` boundary.

`typed` requires a **`*u8` specifically** (E0279), not any pointer, because allowing `*T` → `*U` would be the
general cast reached by another spelling. `untyped` is the safe direction and is still an intrinsic, so that
both directions are searchable and neither widens `cast`.

**`untyped` exists because a facility that can allocate and not free leaks by construction.** `Basic.free` takes
a `*u8`.

### 2. A pre-existing miscompile in store-to-load forwarding, which only this could reach

Retyping is a **store then load through a slot** — the mechanism ADR-0076 §1 built, since a pointer's bits do
not depend on its pointee and no conversion node exists. And **store-to-load forwarding deleted exactly that
step**: it replaced the load with a `Use` of the stored `*u8`, in a destination typed `*s64`. The verifier caught
it as `use changes type`, which is the good outcome — but the pass was *wrong*, not merely unlucky, because the
store and load here **are** the conversion rather than a redundant pair.

Nothing before this sub-wave stored one type into a slot of another and read it back in the same block, so the
pass had never had the opportunity to be wrong. It is now type-checked before forwarding.

**The first attempt at that check was too broad** and cost a real optimisation: requiring the stored type to
*equal* the loaded type killed forwarding of struct **field** loads, where the slot is a `Point` and the load is
an `s64`. The snapshot caught it immediately — `hello`'s optimized MIR went from 5 blocks to 14, with the whole
`Point` construction reappearing. The check now compares the stored **value's** type against the load's, and
skips a constant operand, whose type comes from its context and cannot disagree.

That is the snapshot doing the job AGENTS.md describes: an optimisation quietly not happening is invisible to
every other gate.

## Consequences

- **A heap-backed, indexable, typed array is now expressible**: `malloc`, `typed`, pointer arithmetic, `free`.
  `valid/087` writes one and reads it back in both engines.
- **One new diagnostic code, E0279**, covering both of the boundary's refusals — one code because they are one
  boundary's two directions, the argument ADR-0099 made for E0277. **E0280 is the first free code.**
- **No engine changed.** Both back ends already lower a store and a load; the conversion has no instruction of
  its own, which is what made ADR-0076 §1's slot trick worth reusing rather than inventing a node.
- **`cast` is unchanged**, and that is the point: the unsound general conversion is still refused, and the one
  place a pointee type may change is spelled, searchable, and documented.
- **What this unblocks and what it does not.** A dynamic array can now hold `*T` storage and grow. Still
  deferred: **cross-file parameterised structs** and **inference through them** (ADR-0085 §5) — so a growable
  array remains per element type, exactly as `Array` is today. `realloc` is deferred to the sub-wave that
  actually grows something, since it wants a size the caller must remember.

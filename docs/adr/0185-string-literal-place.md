# ADR-0185: a string literal's `.data` and `.count` lower

- **Status:** Accepted
- **Date:** 2026-09-02
- **Deciders:** dboll
- Not a wave. One missing arm in one guard, found by writing real graphics code, and recorded because
  the *shape* of the bug is one this project has now met three times.

## Context

`"literal".data` refused the whole body:

```
warning[E0245]: the compiler could not lower the body of `main`
  = the lowering step reported: a memory reference has no place
```

while this worked:

```jairs
title := "glctx";
sdl_create_window(title.data, 0, 0, 200, 150, 10);
```

**A one-line surprise with no rule behind it.** A literal and a local holding one are the same two
words, so a reader who hits this learns nothing except that the compiler has a hole. PLAN §7 had
recorded it as costing "one confused build". It cost another, in the *first* SDL call of the
GL-context probe — because `title.data` is how a Jairs program hands a C function a string, so every
`#foreign` boundary meets this.

## Decision

### §1 — `Item::StringType` joins the spill guard

`field_place` already spills an aggregate-valued receiver with no place into a fresh slot, so a
projection has an address. The guard listed three item kinds:

```rust
None if matches!(
    self.pool.item(receiver_ty),
    Item::StructType { .. } | Item::UnionType { .. } | Item::VariantType { .. }
) => { /* spill, then project */ }
```

A `string` is `Item::StringType`, so it fell to `None` and the body was refused.

**The address was always available.** A `string` is a two-word `{data, count}` aggregate (ADR-0004),
and *both back ends already materialise a string constant into a stack slot* to build one —
`Translator::string_constant` in `jr-codegen-clif` does exactly the spill this arm needs, and has
since the slice. Only the guard withheld it.

### §2 — Why not teach `place()` about literals directly

Rejected. The receiver is not always a literal: `V.data` where `V :: "text";` is a *constant* with
the same problem, and so is a `string` returned by value from a call. Widening the existing guard
covers all three, because what they have in common is the thing the guard tests — an aggregate value
with no place. A literal-specific arm would have fixed one spelling of a general gap.

### §3 — The shape, stated for next time

Three instances now, each an aggregate with no place:

| Receiver | Fixed by | Reported as |
|---|---|---|
| an aggregate **parameter**'s field | spill at entry (ADR-0017) | a garbage pointer handed to `write` |
| `type_info(s64).id`, a struct **returned by value** | this arm (ADR-0075 §2) | "a memory reference has no place" |
| `"literal".data`, a **string** value | this ADR | "a memory reference has no place" |

**The guard is a list of aggregate kinds, and a list is what goes stale.** The next aggregate that
can be a value without a place — a view, a dynamic array, an `#soa` struct — will hit the same arm.
It is *not* replaced with "anything the pool says is an aggregate", because that is not obviously
right: a scalar with no place is a **real** refusal (there is nothing to project) and the guard is
what keeps the distinction, so widening it to a predicate would need that predicate to be exactly as
careful. Recorded rather than pre-solved.

## Consequences

- `"literal".data` and `"literal".count` work in both engines.
- No new diagnostic, no new code, and **no test count change** — the coverage is a corpus program,
  because what changed is what a program can observe.
- PLAN §7's owed-items list loses an entry.

## Verification

- Both engines agree on a program that reads `"abc".count`, takes `"hello".data`, and dereferences
  it to check the first byte is `104`. Exit 7, VM and native.
- The GL-context probe that found it now builds and runs without the local-binding workaround, which
  is the actual acceptance test: `"glctx".data` passed straight to `SDL_CreateWindow`.
- The workspace suite passes with no snapshot change beyond the expected one.

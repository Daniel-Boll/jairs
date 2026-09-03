# ADR-0191: A pointer type as an intrinsic's type argument

- **Status:** Accepted
- **Date:** 2026-09-03
- **Deciders:** dboll

## Context

ADR-0189 §2 changed what an implicitly coerced argument describes: `f(*p)` where `f` wants an `Any`
now yields an `Any` whose type is `*Point` rather than `Point`. That made a **pointer the common thing
to find in an `Any`** — and there was no spelling that read one back. `any_as(a, Point)` correctly
traps on it, and `any_as(a, *Point)` was `E0261: any_as needs a type as its second argument`.

Found while migrating `a_pointer_coerces_to_any_at_a_call_in_both_engines`, which had to be rewritten
to avoid reading the value back at all. A test working around a gap is evidence the gap is on a path
people use.

## Decision

### 1. `described_type` gains a prefix-`*` arm

An intrinsic's type argument is an **expression**, not a `TypeRef`: the parser cannot know that this
particular call is in a type position, so `size_of(Slot(s64, s64))` arrives as a *call* and `*Point`
arrives as a **unary address-of applied to a name**. `described_type` is the one function every
intrinsic asks, and it already had an arm for the parameterised form for exactly this reason. The
pointer arm sits beside it and calls itself, so `**s64` works — `valid/142` asserts that, because a
one-level implementation passes every single-star test.

This fixes `any_as`, `size_of`, `type_info` and every other intrinsic that takes a type, in one place,
which is the argument for putting it here rather than in `any_as`.

### 2. Safety is unchanged, and that is checked

`any_as` compares the recovered type's id against the `Any`'s and traps on a mismatch (ADR-0076 §2,
ADR-0077). Adding a spelling for a pointer type does not weaken that: reading a `*s64` back as
`*Point` traps, verified by probe, because `pointer_to(s64)` and `pointer_to(Point)` are distinct
interned types. The arm adds a way to *name* a type, not a way to skip the check.

### 3. Both spellings stay, and both are asserted

`valid/142` reads an implicitly coerced pointer back with `any_as(a, *Point)` **and** an explicitly
erased one with `any_as(a, Point)`, in one program. Asserting both is what stops one silently becoming
the other — the same reasoning ADR-0189 §2's migrated test rests on, one level down: that test pins
that the two spellings *describe* different types, and this file pins that both *read back*.

The differential test's doc now says its omission is a choice rather than a limitation, and names the
corpus file that covers the round trip. A comment claiming a gap that has been closed is the stale
claim this project keeps paying for.

## Consequences

- An `Any` produced by the implicit coercion can be read back, which was ADR-0189 §2's missing half.
- `size_of(*T)` and `type_info(*T)` work, which nothing had asked for and which fall out for free.
- Still owed, and deliberately not built: `[]T` and `[N]T` as an intrinsic's type argument. Both are
  expressions too, and neither has been wanted — a view's element type is reachable through
  `Type_Info.element` and an array's through the same. Building them speculatively would be three arms
  for one probe's worth of evidence.

## Alternatives considered

**Special-casing `any_as`.** Rejected: the gap is in how an intrinsic reads a *type*, and four
intrinsics share that function. Fixing one would leave `size_of(*u8)` refused for no reason a caller
could name.

**Requiring a named alias — `PtrPoint :: *Point; any_as(a, PtrPoint)`.** Rejected: a file-scope alias
of a *builtin* is E0201 today, so the workaround does not even work for `*u8`, and it asks a caller to
name something the language can already spell.

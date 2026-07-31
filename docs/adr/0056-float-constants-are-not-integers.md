# ADR-0056: A compile-time float result is interned as a float, not as an integer

- **Status:** Accepted
- **Date:** 2026-07-30
- **Deciders:** dboll
- **Scope:** One line in `jr-db`'s `reduce`, plus the corpus file that would have caught it. No
  language change.
- **Closes the last case** where the comptime VM and the native back end disagreed about what
  *compiles* — the class of gap ADR-0051 existed to close for aggregate returns.

## Context

`R :: 0.5; main :: () { if R == 0.5 { } }` made `jr build` panic inside Cranelift's verifier —
"internal error: entered unreachable code", from `iconst_bounds` — while `jr run` computed the right
answer. It was found by ADR-0055's corpus program, which was the first thing in the project to put a
float constant in a comparison.

Four facts locate the bug, and the fourth is why it survived.

- **`Item::IntValue { ty, bits }` and `Item::FloatValue { ty, bits }` are distinct pool items**, and
  `jr-codegen-clif`'s `constant` dispatches on which: an `IntValue` becomes `iconst`, a `FloatValue`
  becomes `f32const`/`f64const`.
- **`jr-db`'s `reduce` copies a compile-time result out of the VM**, mapping a `Value` to a `Raw`
  which is then interned. It mapped *every* `Value::Scalar` to `Raw::Int`.
- **A float is a scalar in the VM.** ADR-0040 §3 decided that "a float is its bits, and the
  interpretation comes from the type", so `jr-vm` has no float `Value` variant. **This is the fact
  that made the bug**: `reduce`'s `Scalar` arm cannot tell a float from an integer by looking at the
  value, and it did not look at the type.
- **The VM reads it back correctly**, because it also takes the interpretation from the type. So the
  wrong pool item produced the right answer under `jr run` and a verifier panic under `jr build`.
  **That asymmetry is why nothing caught it**: `differential.rs` compares two engines' *output*, and a
  program that does not build has none.

## Decision

### 1. `reduce` asks the type whether the result is a float

`Raw` gains a `Float(u64)` variant, and `reduce` takes an `is_float` flag computed from the result
type before the VM borrows the pool. A float scalar becomes `Raw::Float`, which interns through
`Pool::float_value`.

**The flag is passed in rather than computed inside**, because `reduce` runs while the VM holds the
pool and cannot ask it. That is a constraint of the existing borrow structure rather than a choice,
and it is recorded in a comment at both ends so the next reader does not try to tidy it.

**Rejected: giving `jr-vm` a float `Value` variant.** It would make `reduce`'s dispatch fall out for
free, and it reverses ADR-0040 §3 — whose whole argument is that a float needs no variant because
the type already says how to read the bits. Reversing an accepted decision to fix a one-line seam in
a different crate is the wrong trade, and it would touch every `Value` match in the interpreter.

**Rejected: checking in `jr-codegen-clif` instead.** The back end could notice an `IntValue` with a
float type and emit `f64const`. That treats the symptom: the *pool* would still hold an item whose
`ty` and variant disagree, and every future consumer would have to know. Interning it correctly means
no consumer has to.

### 2. The corpus gets a float constant, in both widths

`tests/corpus/valid/045-float-constants.jr` exercises a float `::` constant in a comparison, in
arithmetic, negative, integral-valued, and at `float32` width.

**The integral-valued case is deliberate**: `WHOLE :: 2.0` is the one most likely to *look* right while
carrying integer bits, because `2.0` and `2` are visually the same number. A corpus that only tested
`0.5` would catch this bug, and one that only tested `2.0` might not have.

**A `float32` *constant* turns out to be unwritable** — `NARROW :: 0.75` infers `float64`, and
`NARROW : float32 = 0.75` is a *variable*, not a constant. So the narrow path is exercised through a
local, and the gap is recorded rather than papered over: there is no syntax for a typed constant.

## Consequences

- **`Raw` gains a variant**, so its match arms are a compile error until taught — one site.
- **A corpus file now exists for float constants**, which is what should have caught this eleven waves
  ago. Verified by reverting the fix: the file panics.
- **No new diagnostic code**, no language change, and no ADR is superseded. ADR-0040 §3's "a float is
  its bits" stands; this is about a *seam* that forgot to ask the type.
- **`PLAN.md`'s claim that the two engines agree about what compiles is true again.** It was stated in
  the README and had one exception nobody knew about; ADR-0051 closed the case it knew of.
- **A typed float constant is owed.** `NARROW : float32 :: 0.75` does not parse and there is no other
  spelling, so a `float32` constant cannot be written at all. Recorded in §7.

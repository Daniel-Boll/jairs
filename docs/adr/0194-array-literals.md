# ADR-0194: `T.[a, b, c]` — fixed array literals

- **Status:** Accepted
- **Date:** 2026-09-03
- **Deciders:** dboll

## Context

Reading the three real Jai repositories for ADR-0185 counted this **39 times** — the single most used
construct Jairs lacked, by a wide margin over the next.

ADR-0039 §6 deferred array literals, and named exactly why: `[1, 2, 3]` "needs decisions about inferred
versus declared length, whether elements must be comptime-constant, and how ADR-0016 §1's context typing
reaches an element — a separate ADR's worth".

## Decision

### 1. Naming the element type answers all three deferred questions by construction

`s64.[10, 20, 30]` is a `[3]s64`. That spelling is Jai's, and it is why this was buildable now when
`[1, 2, 3]` was not:

- **Length**: it *is* the element count. Nothing is inferred and nothing is declared twice.
- **Constant elements**: a non-question. An element is an ordinary expression, so `s64.[n, n * 2]`
  works, because there was never an inference depending on the elements being known.
- **Context typing**: the named type is the **expectation** for each element — ADR-0016 §1's existing
  mechanism, unchanged. `u8.[1, 2, 3]` types its literals as `u8`, and `u8.[1, 2, 300]` is E0204 at the
  element that overflows.

That third point is the one that makes the named type more than decoration: without it those literals
would land on `s64` and `b` would be a different array entirely.

The parser decides on **one token** past the `.`: a field name is always an `IDENT` and can never be a
`[`. So `ARRAY_LITERAL` joins the postfix chain beside `FIELD_EXPR`, at the same precedence, and
`T.[1, 2].count` reads the way a reader expects.

### 2. The element type is carried as an **expression**, and that is what kept the wave small

`Expr::ArrayLit { elem_ty: ExprId, elems, span }` — not a `TypeRefId`.

Sema resolves it through `described_type`, the one function every intrinsic already asks for a type
argument. So `Point.[…]`, `Slot(s64, s64).[…]`, `(*u8).[…]` and `type_of(x).[…]` all work with **no code
for any of them** — the last two only because ADR-0191 and ADR-0192 had just put those arms there. The
parser also needs no way to tell a type from a value before the `.`, which it could not do: both are a
bare name.

Two refusals. E0261 for an element type that will not resolve, reusing the message every intrinsic
gives. **E0295** for an empty literal: a `[0]T` cannot be indexed, has size zero, and a `for` over it
runs no iterations, so every operation on one is an error or a no-op. Its own code rather than E0261's,
because that one means "I do not know what this holds" and this means "it holds nothing" — a reader
chasing the first would go looking for a misspelled type name.

Resolution needs the element type in a **type position** and the elements not, which is the one
asymmetry inside the node: without the flag, `s64.[1, 2]` is `unresolved name s64`.

**And that needed two halves, not one — found after the feature was merged.** The recursive walk's flag
makes the *body* form work, which is what every test exercised. At **file scope** the top-level expression
arena is walked **flat** (ADR-0180 §4), so the loop reached `s64` as an expression in its own right before
ever reaching the literal that makes it a type: `A :: s64.[1, 2];` reported `unresolved name s64`, and worse,
that **masked** §4's honest refusal behind it. `intrinsic_argument_exprs`' skip set gained the element type.

The array literal is the **second** construct to need both halves, so the shape is now stateable: anything
that puts a type in an expression arena at file scope needs an entry in the skip set *and* a flag in the
recursive walk. `valid/146` pins the working half and `imports/invalid/022` the message.

### 3. MIR: a slot, one store per element, and a spill that fixed something older

The literal lowers to a slot of the array's type, a `Store` per element at a constant index, and a
`Load` of the whole slot as the value. **No `BoundsCheck` is emitted**, and that is safe by construction
rather than by omission: the indices are `0..n` where `n` *is* the length, both from the same list, so no
input can put one out of range. Emitting them would be checks the optimiser then removes and a reader of
the MIR would have to work out why they were there.

The literal is **not a place**, in both sema and MIR: the slot is an implementation detail, so
`s64.[1, 2][0] = 5` is refused rather than assigning into a temporary nobody can read back.

That made `for v: s64.[1, 2, 3]` fail — `for_bounds` calls `place()` and got `None`, reporting "a `for`
over something with no length" on a program with an obvious length. Fixed by **spilling a sequence that
is a value**, once, before the loop. That also fixes `for x: f()` over an array-returning call, which had
the same shape and the same misleading message, and which nothing in this wave needed: a general
improvement fell out of a specific one.

### 4. No compile-time value, refused rather than placeheld

`A :: s64.[1, 2, 3];` at file scope is refused with a message naming the gap. A thunk produces one
`Operand`, and an array's value is a run of bytes that would have to be interned as a static array and
referred to by address. The pool *can* build one — `static_array` is what the field and member tables
use — but wiring it here means deciding what a `ConstValue` holding an aggregate is, which no caller has
needed. Every one of the 39 counted uses is inside a body.

Refused, not placeheld, which is this project's first named failure mode (a construct the grammar allows
with no representation on the lowering path, filled in with a legitimate-looking value).

### 5. The formatter deleted the whole literal, and the emitter needed **two** entries

`a := s64.[10, 20, 30];` formatted to `a := ;`. Fifteenth wave in seventeen to need an emitter entry,
and the most destructive so far: not a dropped attribute but the **value**. The same silent deletion
`cast` suffered in ADR-0037's wave and `ENUM_TYPE` in another.

Two entries, not one: the `ARRAY_LITERAL` arm, *and* `is_expr_kind`. The arm alone leaves it unemitted at
every nesting site, which is the second half of the trap the comment beside `CAST_EXPR` already
describes.

Tree-sitter needed a rule (six `ERROR` nodes without it), at precedence 7 to match the hand-written
parser's chain, and the checked-in Neovim parser needed rebuilding — the step ADR-0148 recorded.

## Consequences

- The most used construct real Jai code has and Jairs lacked now works: as an initialiser, a call
  argument, a `for` sequence, and with any element type an intrinsic can name.
- `for x: f()` over an array-returning call works, which it did not before and which nothing asked for.
- `valid/146` and `imports/invalid/022` pin the file-scope behaviour together: one that the body form
  works, one that the refusal reports itself honestly. The fixture is in `imports/invalid/` because E0230 is
  `jr-db`'s const-eval code and `type-errors/`'s harness runs sema only — the file moved to meet a
  directory's contract rather than the contract bending, for the seventh time in this project.
- Still owed: a **compile-time** array literal (§4), and a struct literal `Point.{1, 2}` — ADR-0039 §6's
  other half, which needs field-order decisions this wave's element-count answer does not supply.

## Alternatives considered

**`[1, 2, 3]` with an inferred element type.** Rejected, and it is what ADR-0039 §6 deferred: the element
type would come from the elements, so `[1, 2, 3]` in a `u8` context needs the context to reach *through*
the literal into each element, and `[]` has no type at all. Naming the type removes the question rather
than answering it.

**A `TypeRefId` for the element type.** Rejected in §2: it would need the parser to know a bare name is
a type before the `.`, and it would not compose with `type_of(x).[…]` or a parameterised type without
duplicating what `described_type` does.

**Emitting bounds checks for the element stores.** Rejected in §3 — the indices and the length come from
the same list.

**Allowing `T.[]`.** Rejected in §2. Every operation on a `[0]T` is an error or a no-op.

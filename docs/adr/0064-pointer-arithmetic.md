# ADR-0064: pointer offset (`p + n`, `p - n`) is element-scaled, unchecked, and lowers to an indexed address

- **Status:** Accepted
- **Date:** 2026-07-31
- **Deciders:** dboll
- **Lifts a refusal ADR-0060 §5 deferred**, which said `p + 1` on a `*u8` "is still refused (E0223)…
  Changing the arithmetic rule is its own ADR with its own argument." This is that ADR.
- **Unblocks temporary storage**, W3's last feature: a *bump* allocator advances a pointer by the
  bytes it hands out, which is `p + n`, and without it an allocator can only wrap `malloc`.

## Context

A pointer is a real type (`*T`, ADR-0011) and `malloc`/`free` exist (ADR-0060), but nothing can move a
pointer: `p + 1` is E0223, "operator `+` is not supported for `*u8`". Every W3 handoff has named this,
and ADR-0062 §5 and ADR-0063's handoff both flagged it as the blocking gap for temporary storage —
the corpus's `recording_alloc` counts bytes but still delegates to `malloc`, because it cannot carve a
region itself.

The pieces are already in place, which is what makes this small. `*x[i]` — the address of an indexed
element — works in both engines today and scales the index by the element's stride (`Projection::Index`
does, in `jr-codegen-clif/src/body.rs` and `jr-vm/src/lower.rs`, from one shared layout computation).
So `p + n` is not a new machine operation; it is the address of `p.*` indexed by `n`, a shape both
engines lower correctly already.

## Decision

### 1. Three offset operations — `p - q` is deferred

| form | result | meaning |
|---|---|---|
| `p + n`, `n + p` | `*T` | the pointer advanced by `n` elements |
| `p - n` | `*T` | the pointer moved back by `n` elements |

`p` is `*T`; `n` is any integer. Everything else on a pointer stays E0223: `p * 2`, `p / 2`, `p + q`,
`p % n` have no meaning, and `<` `>` between pointers are deliberately left out (ordering two pointers
into different objects is unspecified in C and Jairs has no reason to pick an answer yet). Comparison
with `==`/`!=` and against `null` already works and is unchanged.

**`p - q` (the pointer *difference*) is deliberately deferred to its own wave**, and §5 records why:
its result is a count of *elements*, so it must divide the byte distance by the element stride — and
the stride is layout, which ADR-0017 §5 keeps out of `jr-mir` entirely (the back ends scale a
`Projection::Index`, `jr-mir` never sees a size). So `p - q` needs either a new MIR node the back ends
compute or a layout query `jr-mir` does not have, which is a decision of its own and buys nothing the
motivating use case — a bump allocator, which only ever *advances* a pointer — needs. `n - p` is E0223
regardless: the distance is `p - n`, the other order.

### 2. Element-scaled, like C — `p + 1` advances one `T`, not one byte

`p: *s64; p + 1` points at the next `s64`, eight bytes along. This is C's rule and the one a Jairs
programmer coming from any systems language expects; the alternative — byte arithmetic, where `p + 1`
is one byte regardless of `T` — was rejected because it makes the common case (`p + 1` to step to the
next element) the verbose one (`p + sizeof(T)`) and invites the bug where a stride is forgotten.

**A `*u8` is the case where the two rules coincide**, and it is the common one (a byte buffer, an
allocator's arena), so most uses would not tell the difference — which is exactly why the rule must be
chosen for the case that *does*: a `*s64` walked with `+ 1`.

### 3. Unchecked — a pointer has no length to check against

`p + n` emits **no bounds check**, because a raw pointer carries no length: there is nothing to compare
`n` against. This is not a hole in ADR-0003, it is the boundary of it — ADR-0003's checks are for
arrays and views, which *know* their length, and a pointer is the type you reach for precisely when you
are managing memory whose extent the language does not track (an allocator's arena is the motivating
case). Walking a pointer past its allocation is undefined behaviour, the same trade `--no-bounds-check`
offers for an array (ADR-0058 §1), and it is undefined by construction rather than by a build flag.

**Rejected: a checked pointer type that carries a length.** That type exists — it is `[]T`, the view
(ADR-0044) — and a program that wants bounds-checked walking should use one. Adding a *second* length
to a raw pointer would make `*T` no longer one machine word, which is the whole point of a pointer.

### 4. Lowered to the address of an indexed dereference — no new MIR node

`p + n` lowers to `Address(Deref(p) indexed by n)`: the pointer's pointee place, projected by
`Projection::Index(n)`, then addressed. That is the same `Rvalue::Address` of a
`Place::deref(p).project(Projection::Index(n))` that `*p.*[n]` would build — and both back ends already
scale a `Projection::Index` by the element stride, so the multiply is theirs and this crate adds no
arithmetic of its own.

**The one difference from `index_place` is the missing `BoundsCheck`** (§3), so `p + n` does *not* go
through `index_place`: it builds the indexed place directly and skips the check that a raw pointer has
no length for. `p - n` lowers as the same indexed address with the offset **negated** — an ordinary
`Rvalue::Unary` on the integer, emitted before the index — so there is one scaled-address path, not
two, and it needs no size in `jr-mir`.

### 5. What is deliberately absent

- **`p - q`, the pointer difference.** Its result is a count of *elements*, so it must divide the byte
  distance by the element stride — and the stride is layout, which ADR-0017 §5 keeps out of `jr-mir`:
  the back ends scale a `Projection::Index`, and `jr-mir` never handles a size. So `p - q` needs a new
  MIR node (a "difference, scaled by the pointee") that the back ends compute, or a layout query
  `jr-mir` does not have — a real decision, and one the motivating use case does not force, since a
  bump allocator only advances a pointer. Deferred to its own wave rather than smuggled in; keeping
  the "no new MIR node" property true for what *does* land is worth more than the operation here.
- **`++`/`--` or `+=` on a pointer beyond what compound assignment already desugars to.** `p += 1` is
  `p = p + 1` and works through the existing compound-assignment lowering once §1 makes `p + 1` legal;
  no separate decision.
- **Pointer ordering (`<`, `>`).** Left out (§1); its meaning across separate objects is unspecified.
- **Indexing a raw pointer, `p[n]`.** That is a separate surface (a raw pointer is not an array), and
  `(p + n).*` is the spelling this ADR makes available. A `p[n]` sugar is a later decision.

## Consequences

- **No new diagnostic code**, and no new MIR node, `Statement`, or back-end primitive. The type rules
  are new arms in `check_binary`'s `Add`/`Sub` handling; the lowering is a new arm in `jr-mir`'s binary
  handling that builds an indexed address. **E0258 is still the first free code.**
- **A bump allocator becomes writable**, which is what unblocks temporary storage. The corpus gains a
  program that carves a region from one `malloc` and hands out element-aligned slices by advancing a
  pointer — the shape `recording_alloc` could not have.
- **Both engines must agree on the scaled result**, and the differential harness is what checks it: a
  stride computed differently in the two back ends would be a different address and a different
  observable byte. Because both already scale `Projection::Index` from the same `jr-pool` layout, the
  agreement is structural rather than re-derived.
- **`p - q` is E0223 for now**, since the difference operation is deferred (§5) — a reader who writes
  it gets "operator `-` is not supported for `*T`", which is honest: the operation does not exist yet.
  When its wave lands, that refusal lifts.
- **`n - p` and non-integer offsets are E0223** with their own messages: "cannot subtract a pointer
  from …" points at the reversed operands, and "a pointer can only be offset by an integer" at a
  `float` or pointer offset. Same code, because each is an operator applied to operands it does not
  fit, but the message names the specific mistake.

# ADR-0136: `[..]T` — a compiler-known dynamic array

- **Status:** Accepted
- **Date:** 2026-08-17
- **Deciders:** dboll
- **Wave 6 of eight.** ADR-0128 was wave 1, ADR-0129 wave 2, ADR-0130–0132 wave 3,
  ADR-0133/0135 wave 4, ADR-0134 wave 5. This is the sixth of PLAN §7's remaining eight, and
  the wave the parser refused with `dynamic arrays [..]T arrive in a later wave` since ADR-0039.
- No design fork was put to the decider. PLAN §7's table had already decided the shape:
  *compiler-known layout both engines agree on, ops in Jairs, ADR-0107's doubling*.

## Context

### The refusal, and what stood behind it

The parser saw `[..]s64` and reported E0124 with the note "`[..]T` arrive in a later wave".
Meanwhile `modules/List/module.jr` maintained a hand-rolled `Int_List` with fields `data`,
`count`, `capacity` — the identical layout, expressed as an ordinary struct in Jairs, with
`push`/`pop`/`grow`/`free_data` written on top. The library type worked and shipped in
ADR-0107; what was missing was the *syntax*, so a user writing `xs: [..]s64` got a parse error
rather than a first-class type.

The plan's decision named the wave: lift the syntax, own the layout in the compiler, keep the
operations in Jairs.

## Decision

### 1. Layout is `{data: *T, count: s64, capacity: s64}` — three words

The same shape `modules/List`'s `Int_List` uses today. Three words on a 64-bit target: 24 bytes,
alignment 8. **Structural**, like `[]T` and `*T`: `[..]s64` interns to one `PoolId` however many
files write it, and the element type is the whole identity. Kept separate from an ordinary
struct declaration of the same shape for the reason `string` is separate from
`struct { data: *u8; count: s64; }` (ADR-0004): a user-written struct of the same shape is a
*different* type and never coerces, so merging identities would let indexing and view semantics
apply to types the caller did not declare as arrays.

**Rejected: two words plus a heap-side descriptor** (`{data, count}` in the value with a
capacity kept elsewhere). Saves 8 bytes per dynamic array but adds a level of indirection to
every capacity read and needs a second allocation on every growth — the trade goes the wrong way
on both counts, and matching `Int_List` keeps the two implementations one behavioural set.

**Rejected: a `Dynamic_Array($T)` polymorphic struct emitted by the compiler and looked up
by name.** Would let the same layout ride on cross-file parameterised structs (ADR-0117)
without adding a `TypeRef` variant. Rejected because a user writing `xs: [..]s64` should not
need to import a `Dynamic_Array` module — the point of native syntax is that a caller reaches
for it without ceremony.

### 2. Three pseudo-fields, all places — `.data`, `.count`, `.capacity`

A dynamic array exposes:

- `.data: *T` — the heap-storage pointer, of the ordinary interned pointer type. Unlike a
  view's data word (which is deliberately *not* exposed, ADR-0044 §4), this one is: a dynamic
  array **owns** its heap block, so a caller who wants to `free` or reallocate must reach it.
- `.count: s64` — the number of used elements. A place, so `xs.count = 3` writes through the
  word exactly as a struct field assignment would.
- `.capacity: s64` — the number of allocated slots. Also a place, so a library grow routine
  can update it after reallocating.

**All three are places, all three are readable and writable.** That is what will let a
library `push` routine populate the fields after allocating.

**Rejected: expose `.count` but hide `.data` and `.capacity` behind intrinsics.** A cleaner
API and a real cost — a caller inspecting `xs.capacity` for a benchmark or debugging would
need a native intrinsic call, which is friction the type does not need. The dynamic-array
value already owns its storage; hiding fields would be a fiction the caller can see through.

**Rejected: give a dynamic array `[N]T`-style `.count` semantics** (a constant folded from a
compile-time-known length). Impossible by construction — `[..]T` has no compile-time length.
Written to name what was considered and refused.

### 3. Operations stay in Jairs — no native `push`, no native `free`

The compiler owns the *type* and the *layout*. It does not own the operations. A library —
initially `modules/List`, later converted or replaced — writes `push`, `pop`, `free_data`,
`grow`, etc., in Jairs. That is what "ops in Jairs" in PLAN §7's table means, and it keeps the
compiler surface minimal: nothing needs to inline a growth policy, and a caller who wants a
different one writes their own.

**Rejected: a native `push` intrinsic.** Growth policy (doubling from 4, ADR-0107) belongs in
a library so a caller can substitute it. Native code would either be one policy for
everybody or a parameterised policy that reads like a hack.

**Rejected: give `[..]T` its own `for` loop shape.** A dynamic array already reduces to a
view via `xs.data` + `xs.count`, and iterating a view (`for xs.data[0 .. xs.count]`) is not
worse for reading than a special-cased `for xs`. When there is a caller for the syntactic
sugar, that's a follow-up wave, not this one.

### 4. This wave delivers **the surface and the layout**, not the operations

`valid/109-dynamic-array-syntax.jr` pins declaration, zero-initialisation of all three fields,
and the fields as places. Growth operations from a library (a `push` that reallocates, a `free`
that releases) are the follow-up wave's work — either by converting `modules/List` to operate
on `[..]s64` or by writing a new module. The library layer sits on top of what this wave
delivered without needing a compiler change to do so.

## Consequences

- **The eight-wave programme is 7 of 8 done.** Wave 7 (`$$T`) and wave 8 (`print(fmt, ..Any)`)
  remain.
- **1010 workspace tests unchanged, 222 → 223 corpus files.** `valid/109` exercises the
  surface. `Item::DynamicArrayType`, `TypeRef::DynamicArray`, three new `Projection` variants,
  and the layout helpers `triple_capacity`/`triple_layout` add up in the changed crates, but
  no new Rust unit tests: the differential/snapshot harnesses cover what a corpus program can
  observe.
- **`Type_Info_Kind.DYNAMIC_ARRAY` is reported** in `jr-db`'s const-eval branch, so a program
  reflecting `type_info([..]s64).kind` gets a discriminable answer rather than a
  plausible-but-wrong `VIEW`. The `Type_Info_Kind` enum in `modules/Basic` does not yet include
  this member — that is a library change, deferred to the wave that also converts
  `modules/List`.
- **`Item::DynamicArrayType` is refused as a compile-time aggregate element** (`jr-db`'s
  `reduce_element`), for the same reason `Item::ViewType` is: its data pointer belongs to the
  compile-time evaluator and would silently change target under relocation.
- Deferred, each owed its own decision or its own wave: a library `push` on `[..]T` (a follow-
  up); a `for xs { it }` shape for a `[..]T` (a follow-up); `type_info` reflection reaching
  the element type (a `Type_Info_Kind.DYNAMIC_ARRAY` member + a payload); conversion of
  `modules/List` to `[..]s64` (a wave that also gets to decide whether to keep `Int_List` as a
  compatibility name).

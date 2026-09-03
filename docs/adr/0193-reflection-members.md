# ADR-0193: An enum's members, a view's stride, and a structural type's spelling

- **Status:** Accepted
- **Date:** 2026-09-03
- **Deciders:** dboll

## Context

ADR-0189 §6 recorded four gaps in what `print` could show and named **one** root cause for all of them:
`Type_Info_Field.ty` and `Type_Info.element` are type *ids*, ADR-0077 §1 makes an id deliberately
opaque, and so nothing can recurse into a member's type. The stated fix was a `*Type_Info` per type via
ADR-0152 §3's static-data table, described as "the highest-leverage library-facing item this project
has".

**Three of the four do not need that**, and finding out which is the whole of this wave. The naive
version of the stated fix also does not work: emitting a nested `Type_Info` per member **diverges** on
`Node :: struct { next: *Node; }`, which is precisely why ADR-0077 chose ids in the first place.

## Decision

### 1. An enum gains a member table, and it needs no types at all

`Type_Info.members: []Type_Info_Member`, where a member is `{ name: string; value: s64; }`. Emitted from
the pool's own `enum_members`, so reflection and a `Colour.BLUE` expression cannot disagree about what
`BLUE` is — the same reasoning that makes the field table read `jr_pool::field_offset` rather than
recomputing offsets.

`Type_Info_Member` is a **second small struct** rather than `Type_Info_Field` reused: a field has an
*offset into a value* and a member has a *value of its own*, so a shared struct would leave one member
meaningless in each use and a reader could not tell which was which.

`print` now shows `BLUE`. A value with no matching member prints as a number rather than `<invalid>`:
an enum's storage can hold one (a cast, a flags combination), and the number is the only true thing to
say about it.

### 2. A view needs one number, and the missing arm beside it was invisible until it arrived

`Type_Info.element_size: s64` — the stride of an array, view or dynamic array element, computed from the
*element type's* own layout.

A fixed array was already printable because `size / count` is exact for one. A **view** cannot use that:
its `size` is its header's, not its elements'. So its elements were unreachable even though its own
header holds `data` and `count` — which is why ADR-0189 §5 recorded a view as unreachable "for a
different reason than a procedure is".

**And `element` was never populated for a view.** `type_info_value`'s `(count, element)` match handled
`ArrayType` and `PointerType`; a view and a dynamic array fell through to `(0, 0)`. That omission had
been invisible for waves because nothing *used* `element` for a view — adding the stride beside it is
what surfaced it, as a formatter that had a stride and no element type printing `[.., ..]`. Two absent
things looked like one until one of them was fixed.

`count` stays 0 for a view, and that is not an oversight: a view's count is a **runtime** property held
in its own header, so a static answer would be a lie. `valid/144` asserts both — 0 for the view, 4 for
the fixed array — so a later change reporting a plausible number fails.

One routine reads a view *and* a dynamic array, because ADR-0136 §1 gives both the same first two words
and a dynamic array's capacity is simply not read. A zero stride is refused rather than trusted: it
would loop forever printing element zero, and it can only mean this compiler emitted no size for a kind
that has elements.

### 3. A structural type's name is composed, and it terminates because a declared type answers by name

`type_spelling` composes `*Point`, `[3]s64`, `[]u8`, `[..]Vertex` from the element's own spelling.
Before it, every structural type reported its name as the lowercased *kind* — a `[3]s64` was called
`array`, which reads like a type nobody wrote. ADR-0075 §3 had deferred exactly this ("a composite would
need its element rendered too").

**The recursion terminates for the reason the naive `*Type_Info` does not.** A cycle needs a *declared*
type in it, and a declared type is answered by its declared name **without looking at its members** — so
`*Node` is two steps and `**Node` is three. A declared name also wins over structure, which is right
beyond termination: `Vector4` is not `[4]float32` to a reader even when it is structurally identical.

Three identical `library_struct_type` lookups were collapsed into one on the way past; a fourth was
about to be written.

### 4. What is still opaque, and why it is the hard one

A **nested aggregate or enum field** still prints `..`. `format_field` compares a field's type id
against each builtin's — thirteen constant comparisons the compiler folds — and a field of any other
type has no answer.

That is the one gap that genuinely needs id → `*Type_Info`, and the shape it needs is now clearer than
ADR-0189 §6 stated: not a nested emission, which diverges, but a **flat table** with each type emitted
once and members holding pointers into it, so a second visit to `Node` finds the existing entry. That
is a wave, and it is recorded rather than half-built.

`valid/144` **asserts the gap** rather than omitting it, so the day it lifts, the file's expectation
changes visibly.

## Consequences

- `print` shows an enum's member name, a view's and dynamic array's elements, and a real type spelling.
- `Type_Info.element` answers for a view and a dynamic array, which it silently did not.
- Reflection can enumerate an enum's members, which no Jairs program could do before.
- `Type_Info` has eleven members; the contract in `jr-sema` and the declaration in `Basic` are checked
  against each other by E0265, so the two appended members cannot drift into each other's bytes.
- Still owed: the flat id → `Type_Info` table, for a nested field.

## Alternatives considered

**A nested `Type_Info` per member, as ADR-0189 §6 described.** Rejected on a fact: it diverges on a
self-referential struct, and `Node :: struct { next: *Node; }` is the first thing anybody writes.

**Dividing `size / count` for a view.** Impossible, not merely wrong: a view's `size` is 16 whatever it
holds.

**Reusing `Type_Info_Field` for enum members.** Rejected in §1 — one of the two members would be
meaningless in each use.

**Deferring all four gaps to the table wave.** Rejected: three of them needed no table, and holding
them hostage to the hardest one is how an item stays owed for six waves.

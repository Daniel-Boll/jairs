# ADR-0238: Raw-pointer indexing and compound offsets share one unchecked place

- **Status:** Accepted
- **Date:** 2026-09-10
- **Deciders:** dboll
- **Amends ADR-0064 §5:** raw-pointer indexing is no longer deferred, and its claim that pointer
  compound assignment already composed is made true in MIR.
- **Scope:** `p[i]`, `p += n` and `p -= n`. Pointer ordering, pointer addition and every other
  pointer compound operator remain absent.

## Context

ADR-0064 made `p + n` and `p - n` element-scaled, but left `p[i]` as a later surface decision. It
also stated that `p += n` would work through existing compound-assignment desugaring. The live code
does not satisfy that second claim: sema insists that the right operand have the pointer type, then
MIR emits an ordinary pointer-valued `Rvalue::Binary` rather than the indexed-address shape used by
pointer offsets.

Both gaps block ordinary counted-string code:

```jr
while s.count > 0 && is_space(s.data[0]) {
  s.data += 1;
  s.count -= 1;
}
```

The back ends and VM already support a `Projection::Index` whose current place type is a pointer.
Views use exactly that route for their `data` word: load the pointer, scale the index by the
pointee stride, and address the resulting element. The missing work is therefore deciding when
source indexing selects that route and making compound assignment reuse it.

## Decision

### 1. `p[i]` is an unchecked, element-scaled place

For `p: *T` and an integer `i`, `p[i]` has type `T` and denotes the same place as `(p + i).*`.
The index is measured in elements, including for aggregates; `*u8` naturally measures bytes.
Negative indices are legal for the same reason `p - n` is legal.

A raw pointer carries no allocation identity or length, so no `BoundsCheck` is emitted. The caller
is responsible for keeping the computed address within the allocation (or its one-past position
when the place is only addressed and not read). Reading or writing an invalid place is undefined
raw-pointer behaviour.

An indexed raw pointer is a place even when the pointer value came from a call or another temporary:
the storage being named is the pointee, not storage for the pointer value itself.

### 2. Existing pointer-to-container auto-dereference keeps precedence

Jairs already accepts `p[i]` when `p` points, through one or more pointer layers, to a fixed array,
vector or view. That spelling keeps its existing bounded-container meaning and checks its index.

Only when peeling the pointer chain does not reach an already-indexable container does the original
outer `*T` use raw-pointer indexing. This preserves existing programs while giving `*u8`, `**Node`
and other ordinary pointer sequences the expected element access.

### 3. `p += n` and `p -= n` use pointer-offset lowering

For a pointer assignment target, only `+=` and `-=` accept an integer right operand. They are
equivalent to assigning `p + n` or `p - n` back to the same place and therefore use ADR-0064's
unchecked, element-scaled address operation.

MIR factors the operation through one helper that spills a pointer value into a pointer-typed place
and applies `Projection::Index`. Binary pointer offsets, raw-pointer indexing and pointer compound
assignment all use that helper. Ordinary numeric compound assignment continues to emit
`Rvalue::Binary`.

`p *= n`, `p +%= n`, bitwise compound forms and shifts remain E0223. Pointer addition (`p + q`) and
integer-minus-pointer remain refused. Although `p - q` is a valid `s64` pointer difference,
`p -= q` remains E0223 because that distance cannot be assigned back into `p`.

### 4. No syntax, formatter, query or engine primitive changes

The parser, HIR, formatter and both editor grammars already preserve index expressions and compound
assignment. VM, Cranelift and LLVM already lower pointer-typed index projections for views. This
wave changes sema and MIR selection, then tests the existing engine route across all three engines.

## Rejected alternatives

### Give raw pointers an implicit bound

Rejected because `*T` stores only an address. Guessing a length from a neighbouring string, view or
dynamic-array field would make identical pointer values behave differently according to provenance
the type does not carry.

### Desugar `p[i]` in HIR to `(p + i).*`

Rejected because indexing is already a place-forming HIR expression used by loads and stores.
Keeping it as an index lets MIR choose the same pointer projection views already use and avoids
manufacturing source expressions, spans and temporary identities.

### Make every pointer-to-array index the outer pointer

Rejected because it would silently change the established `p: *[N]T; p[i]` meaning from one `T`
inside the array to one whole `[N]T` in an array sequence. The existing auto-dereference rule wins
for already-indexable containers.

### Emit ordinary pointer-valued binary MIR for `+=`

Rejected because `Rvalue::Binary` is numeric, uses one operand/result type and carries ordinary
integer overflow rules. Pointer offset needs an integer index, pointer result and target-layout
stride supplied at the place-lowering boundary.

## Consequences

- Counted-string scanners may read `s.data[i]` and advance `s.data += n` directly.
- Pointer indexing works for loads, stores and address-taking without introducing a bounds promise.
- Array, vector and view indexing retain their existing compile-time/runtime checks.
- Dead-store elimination distinguishes an indirect `pointer[index]` store from a direct store to
  the temporary slot holding that pointer. The indirect write and the pointer value feeding it are
  observable; deleting either made `-O1` disagree with `-O0`.
- The String module's `byte_at` implementation becomes the strict range policy around a one-line
  raw-pointer read rather than a workaround for missing syntax.
- Gate 7 is required because MIR changes and the LLVM differential must exercise the shared route.

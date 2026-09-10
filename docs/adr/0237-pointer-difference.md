# ADR-0237: Pointer difference is a signed, element-scaled MIR operation

- **Status:** Accepted
- **Date:** 2026-09-10
- **Deciders:** dboll
- **Lifts ADR-0064 §1/§5's explicit deferral:** `p - q` is now defined when both operands have the
  same pointer type.
- **Scope:** Pointer difference only. Pointer ordering, raw-pointer indexing and subtraction across
  different pointer types remain absent.

## Context

ADR-0064 made pointer offsets element-scaled but deferred pointer difference because the result must
divide the byte distance by the pointee stride, and layout deliberately does not live in `jr-mir`.
That omission now blocks ordinary parsers:

```jr
start: *u8 = content.data;
print("Failed at %\n", content.data - start);
```

Both operands are `*u8`; the desired result is the number of bytes consumed. The same rule must stay
coherent for wider pointees: `(p + 3) - p` for `p: *s64` is `3`, not `24`.

Lowering the expression as the ordinary trapping integer subtraction is incorrect. A valid reverse
difference is negative, so subtracting two unsigned machine addresses through ADR-0002's checked
arithmetic would trap. Lowering byte subtraction and division in MIR is also incorrect because MIR
does not own target layout and must not acquire a pointee size merely for this operation.

## Decision

### 1. `p - q` requires one identical pointer type and yields `s64`

When `p` and `q` both have exactly the same `*T` type, `p - q` has type `s64`. Its value is the signed
distance from `q` to `p`, measured in elements of `T`:

```jr
bytes: *u8;
wide: *s64;

(bytes + 9) - bytes; // 9
(wide + 3) - wide;   // 3, not 24
wide - (wide + 2);   // -2
```

Pointer addition remains invalid. Subtraction between different pointer types is E0223, even if the
two pointees happen to have the same layout. Requiring type identity prevents a byte stride from
being selected arbitrarily and makes an intentional representation change visible as an explicit
cast.

As with ADR-0064's offsets, the operation is unchecked: raw pointers carry no allocation identity or
length. The programmer is responsible for subtracting pointers into the same allocation. One-past
pointers are legal operands because no dereference occurs. The byte distance must be an exact
multiple of `T`'s stride and the element distance must fit `s64`; violating either condition is
undefined raw-pointer behaviour rather than a checked arithmetic trap.

A zero-sized `T` is refused statically with E0223. Every offset of such a pointer has the same
address, so no address subtraction can recover an element count and no nonzero stride exists to
divide by.

### 2. MIR carries `PointerDifference`; each engine supplies the stride

`Rvalue::PointerDifference { lhs, rhs }` is a distinct operation. It carries pointer operands and
defines an `s64`; the pointee type is recovered from the operands' recorded type, so no duplicated
type field can drift.

Each engine:

1. subtracts the two machine addresses without ADR-0002 overflow trapping;
2. interprets the byte difference as signed;
3. divides it by `layout_of(T).stride`, where stride is size rounded up to alignment;
4. produces the resulting `s64`.

The VM bytecode records the resolved stride on its own `PointerDifference` instruction, at the same
lowering boundary where indexed places already become `ScaledIndex { stride }`. Cranelift and LLVM
query the same `jr-pool` layout directly.

The implementation uses machine signed division without a divisibility check. That is observable
only after the program has already left the defined contract above; no truncating result is promised.

### 3. The MIR node is pure but not constant-folded

Pointer difference has no memory effect and cannot trap for well-formed MIR, so DCE may remove an
unused result. Const propagation does not fold it: MIR constants do not carry relocatable pointer
provenance or target layout, and inventing either would widen this feature beyond its need.

Inlining, SSA replacement, slot remapping, verification and use tracking handle both operands
explicitly. The verifier requires:

- both operands have the same pointer type;
- the destination type is `s64`.

## Rejected alternatives

### Return the raw byte distance for every pointer type

Rejected because it contradicts ADR-0064's element-scaled arithmetic. `p + 3` followed by subtraction
must round-trip to `3` regardless of `T`.

### Implement only `*u8 - *u8`

Rejected because it solves one parser while leaving an arbitrary type exception. `*u8` naturally
measures bytes under the general element rule.

### Accept different pointer types

Rejected because the operation needs one stride and neither operand has a principled claim to choose
it. An explicit cast is the visible decision when representation-level subtraction is intended.

### Reuse ordinary `Rvalue::Binary { op: Sub }`

Rejected because ordinary subtraction uses the operand type and ADR-0002's overflow semantics.
Pointer difference has pointer operands, an `s64` result, target-layout scaling and valid negative
answers; hiding those differences in the generic arithmetic path would make wrong lowering easy.

### Put the stride in MIR

Rejected because ADR-0017 keeps target layout out of MIR. The three engines already own the exact
boundary where an element index becomes bytes, and pointer difference belongs at that same boundary.

## Consequences

- `content.data - start` checks when both are `*u8` and yields the byte offset expected by parsers.
- The VM, Cranelift and LLVM implement one explicit operation and are differentially tested for
  forward, reverse, one-past and wider-pointee differences.
- `p + q`, `n - p` and different-type `p - q` remain E0223.
- A pointer to a zero-sized pointee cannot be subtracted because it has no element stride.
- No parser, formatter, tree-sitter or LSP syntax change is needed; this is semantics and execution.
- Gate 7 is required because MIR and both native back ends change.

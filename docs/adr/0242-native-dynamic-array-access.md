# ADR-0242: Native dynamic arrays are bounded indexable sequences

- **Status:** Accepted
- **Date:** 2026-09-10
- **Deciders:** dboll
- **Scope:** Source indexing, address-taking, assignment, and ordinary `for`
  iteration over `[..]T` and pointers to it. Growth, ownership, slicing, and
  iterator protocols remain library concerns.
- **Amends ADR-0136 §4:** native dynamic arrays no longer require access through
  their `.data` pointer.
- **Extends ADR-0238 §2:** pointer-to-dynamic-array auto-dereference keeps the
  bounded container meaning ahead of unchecked raw-pointer indexing.

## Context

ADR-0136 made `[..]T` a compiler-known three-word value:

```text
{ data: *T, count: s64, capacity: s64 }
```

ADR-0140 and ADR-0231 then built generic library operations over that value, but
ordinary source still could not write `xs[i]` or `for value, index: xs`.
Callers had to either use a fallible library operation or escape through
`xs.data[i]`, which changes a bounded container into an unchecked raw pointer.

The MIR already has `DynamicArrayData` and `DynamicArrayCount` projections.
`BoundsCheck` already accepts a run-time length operand for views. No execution
engine primitive is missing; only sema's accepted shapes and MIR's choice of
data/count projections are incomplete.

## Decision

### 1. `[..]T` is an indexable bounded container

`xs[i]` has type `T` and denotes a place. The same place supports reads,
assignments, compound assignments, and address-taking:

```jr
value := xs[i];
xs[i] = value + 1;
pointer := *xs[i];
```

MIR loads the bound from `xs.count`, takes the element source from `xs.data`,
emits the existing `BoundsCheck`, and then applies the existing index
projection. There is no dynamic-array-specific load or store path.

### 2. Bounds use `count`, never `capacity`

Only the used prefix is logically populated. An index equal to `count` traps
even when allocated capacity is larger. This preserves the same contract as
`List.get` and prevents uninitialised spare storage from becoming ordinary
language-visible elements.

The check is omitted in a `#no_abc` procedure exactly as it is for arrays and
views. That attribute changes checking policy, not which word is the semantic
length.

### 3. Ordinary `for` iteration visits the used prefix

`for value, index: xs` starts at zero, ends at the value loaded from
`xs.count`, and reads elements through `xs.data`. Existing reverse and
index-variable loop forms inherit the same bounds because they share
`ForBounds`.

The array header is evaluated once before the loop, matching arrays, views,
strings, and sequence temporaries. Mutation of the header during the loop does
not retarget or resize an already-started iteration.

### 4. Pointer-to-container meaning wins before raw-pointer fallback

For `p: *[..]T`, `p[i]` auto-dereferences `p` and indexes the dynamic array with
its `count` bound. It does not mean raw indexing over adjacent dynamic-array
headers.

This is the existing rule for `*[N]T` and `*[]T`: peel a pointer chain, prefer a
known bounded sequence at the end, and use ADR-0238's unchecked raw-pointer
meaning only when no bounded container is reached.

### 5. No slicing or ownership rule is implied

`xs[]` remains outside this wave. A dynamic array owns capacity while a view
borrows a prefix, so choosing whether slicing borrows `count`, supports
subranges, or participates in mutation deserves its own source contract.
Indexing and iteration neither allocate nor free storage.

## Rejected alternatives

### Continue requiring `xs.data[i]`

Rejected because it discards the container's bound at the exact operation that
needs it. It also makes dynamic arrays less composable than views despite
carrying strictly more length information.

### Bound indexing by `capacity`

Rejected because capacity describes allocation, not initialised elements.
Making spare storage readable would disagree with every library operation and
would expose allocator contents as values.

### Add a dynamic-array MIR operation

Rejected because the existing data/count projections plus the existing
run-time `BoundsCheck` express the operation completely. A new instruction
would duplicate array/view lowering and force three engine implementations to
encode no new semantic fact.

### Give raw-pointer indexing precedence for `*[..]T`

Rejected because it would make `p[i]` address the ith dynamic-array header while
the same spelling over `*[N]T` and `*[]T` addresses the ith element. The
container interpretation is both established and safer.

## Consequences

- Native dynamic arrays can be used directly anywhere ordinary indexed places
  are accepted.
- Iteration, indexing, library `get`, and library `set` agree that `count` is
  the used length.
- Existing bounds-check configuration and `#no_abc` apply without a new policy.
- VM, Cranelift, and LLVM need no new operation; gate 7 is still required
  because MIR lowering changes.
- The first free global diagnostic code remains E0301; the first free parser
  code remains E0137.

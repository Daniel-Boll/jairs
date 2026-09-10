# ADR-0232: `Table(K,V)` is zero-ready, policy-extensible and allocator-owning

- **Status:** Accepted
- **Date:** 2026-09-10
- **Decider:** dboll

## Context

The requested recursive node shape is incomplete while `Table(string, string)` is only a local
placeholder:

```text
Node :: struct {
  name: string;
  children: [..]*Node;
  properties: Table(string, string);
}
```

ADR-0229 and ADR-0230 supply nominal inference and owner-file specialisation, and ADR-0231 proves
generic imported container operations with `[..]*Node`. The remaining decisions are the hash/equality
policy, allocation ownership, lookup and iteration shapes, and how closely to follow a closed-beta
Jai module whose current declaration source is unavailable.

The source audit in `docs/research/jai-table-and-interfaces.md` found contemporary use of
`Table(K,V)`, zero-ready insertion, string keys, custom type-level hash/equality arguments,
`table_find_pointer`, reserve/reset operations, `deinit` and direct `for table` iteration. It also
found API drift: current observed `table_find` order differs from the older tutorial.

Three interface designs converged on a generic table with custom policy and captured allocation.
The selected design keeps Jai-shaped `table_*` names, Jairs' established `(value, found)` result
order and an explicit cursor until user-defined iteration exists.

A focused compiler probe rejected `New(State(K,V))` and `typed(State(K,V), raw)`: an intrinsic type
argument may be a bare bound `K`, but not yet a parameterised nominal expression containing bound
variables. The same probe proved imported generic callback fields/calls, custom struct keys,
`type_info(K)`, `typed(K, raw)` and returning `*V` from generic storage.

## Decision

### 1. `Hash_Table` exports a zero-ready `Table(K,V)`

The public module exports:

```text
Table(K,V)
Table_Iterator(K,V)

table_add
table_set
table_find
table_find_pointer
table_contains
table_remove
table_reserve
table_count
table_capacity
table_clear
table_iterator
table_next
table_set_key_policy
deinit

hash_bytes
hash_string
hash_s64
hash_combine
```

`table_add` inserts or replaces and returns whether storage was available.
`table_set` performs the same upsert and returns a pointer to the stored value,
matching the observed contemporary Jai result shape.
`table_find` returns `(V, bool)`, matching `List.get`, `List.pop` and `Map.get`.
`table_find_pointer` returns null for absence and lets a caller mutate the stored value in place.

Public Jai call sites strongly suggest that `table_add` is intended for known-new keys while
`table_set` is the explicit upsert, but the distributed declaration and duplicate-key contract are
not public. Jairs therefore makes both safe upserts rather than inventing destructive duplicate
behaviour; `table_set` is the operation to use when its stored-value pointer is useful.

A zero-initialised table needs no initializer. `table_reserve` is optional, and first insertion
captures the active allocator lazily.

### 2. Common key types have built-in policy; every other `K` may install one

Default hash/equality is available for:

- `string`, by byte content rather than pointer identity;
- integer and enum values;
- `bool`; and
- pointers, by address identity.

The scalar cases hash and compare their value bytes. Strings hash their bytes and use content
equality. A fixed FNV-1a hash keeps results deterministic on one target; iteration order is not a
cross-target contract.

Structs, arrays, floats, views, dynamic arrays, procedures and unions require
`table_set_key_policy` before insertion. The callback pair receives borrowed pointers plus one
borrowed `*u8` policy-data pointer. Equal keys must always hash equally, key meaning must stay stable
while stored, and callbacks must not mutate keys.

Unsupported default insertion is a source-located assertion rather than silent byte comparison.
Padding, float NaNs and borrowed aggregate storage make arbitrary byte policy dishonest.

### 3. The header is a struct of arrays, not an opaque generic state allocation

`Table(K,V)` holds parallel pointers to keys, values, cached hashes and byte slot states, plus count,
capacity, tombstone count, callbacks, allocator capture and a mutation generation.

This is a deliberate response to the measured intrinsic gap:

- `typed(K, raw)` and `typed(V, raw)` work in an imported clone;
- `typed(Slot(K,V), raw)` does not; and
- separate arrays let an allocation failure roll back without publishing a partial table.

The fields are public only because Jairs has no field privacy. They are implementation-owned and
callers must use the procedures. This loses one-word opacity but removes one state allocation and
one indirection.

### 4. Open addressing owns probing and growth

The module uses:

- power-of-two capacity beginning at eight;
- linear probing;
- cached `u64` hashes;
- empty/live/tombstone byte states;
- growth or rebuilding before occupied-plus-tombstone load reaches 3/4; and
- allocate-copy-free rehashing that publishes the new arrays only after every allocation succeeds.

An update probes before deciding to grow, so replacing an existing value cannot fail because of an
unneeded allocation. Removal leaves a tombstone. Rebuilding reuses cached hashes and does not invoke
user callbacks again.

### 5. Allocation is captured and cleanup is explicit

The first backing allocation captures `context.allocator`, `context.allocator_free` and the current
allocator data. Every later allocation and release runs under that captured triple and copies
mutated allocator data back.

The table owns only its four backing arrays. Keys and values are shallow-copied; strings and pointers
remain borrowed. `table_remove`, `table_clear` and `deinit` do not recursively destroy them.

`table_clear` retains storage, policy and allocator capture. `deinit` frees every backing array,
logically resets the header and is safe twice. Procedure-valued fields retain unspecified stale bits
because procedure values are not nullable in Jairs; explicit flags make those fields inactive until
the next allocator capture or policy installation. A `Table` header allocated by `New(Table(K,V))`
remains a separate allocation the caller must release after `deinit`.

Key and value alignment above the allocator protocol's 16-byte guarantee is rejected before the
first backing allocation.

### 6. Iteration is explicit and mutation-invalidated

`table_iterator` returns a cursor carrying the table pointer, next slot and generation.
`table_next` scans to the next live slot and returns `(K, *V, bool)`. Order is unspecified.

Insertion of a new key, removal, reserve/rehash, clear and deinit invalidate cursors and stored-value
pointers. Replacing an existing value does not structurally mutate the table.

Exact `for key, value : table` syntax remains a language/metaprogramming wave; the library does not
special-case the compiler for one container.

## Rejected alternatives

- **Keep generalising `Map`.** Its operation surface and implementation are concrete `s64 -> s64`;
  changing its nominal layout breaks existing users. It remains the compatibility module.
- **Require initialization for every table.** String and scalar defaults are common, and current Jai
  source demonstrates useful zero-ready tables.
- **Byte-hash every possible `K`.** Struct padding, float equality and borrowed views make equal
  source values disagree or unstable.
- **Wait for `$T/interface`.** Structural data fields do not express free-standing hash/equality
  policy, and the two features are independently useful.
- **Store `State(K,V)` behind one `*u8`.** The exact generic intrinsic composition is not accepted
  today; pretending otherwise would start the module on a compiler blocker.
- **Compiler-owned `for Table`.** One container does not justify a language special case.
- **Stable iteration order.** Open addressing and reserve operations naturally reorder slots, and no
  inspected source promises stability.

## Consequences

- The exact requested `Node.properties: Table(string, string)` is a real standard-library type and
  composes with `New(Node)` and generic `[..]*Node` stacks.
- Callers get zero-ready common keys and arbitrary custom keys without waiting for imported `$N`
  specialisation or structural interfaces.
- The table's layout is larger and conventionally opaque rather than mechanically private.
- Hash-flood resistance is not provided; custom seeded policy is the escape hatch.
- `Map` remains available and source-compatible.
- This is a library-only wave unless its executable probes uncover another compiler gap.

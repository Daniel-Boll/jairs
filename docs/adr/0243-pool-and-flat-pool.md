# ADR-0243: Pool allocators capture their backing context and never relocate live results

- **Status:** Accepted
- **Date:** 2026-09-10
- **Deciders:** dboll
- **Scope:** Standard-library `Pool` and `Flat_Pool` modules, their context
  allocator adapters, allocation/reset/release policy, and composition with
  `New`, dynamic arrays, and generic tables. Individual deallocation and
  allocator-mode structs remain outside the current context protocol.
- **Guide evidence:** the pinned *The Way to Jai* chapter 34 names `Pool`,
  `get`, `set_allocators`, `reset`, `release`, `pool_allocator_proc`,
  `memblock_size`, `bytes_left`, and `overwrite_memory`. `Flat_Pool.reserve`
  and the explicit free adapters are Jairs additions required by its smaller
  allocator protocol.

## Context

Jairs already lets a program replace these three context fields:

```jr
allocator: (s64) -> *u8;
allocator_free: (*u8);
allocator_data: s64;
```

`New(T)`, `String_Builder`, and `Hash_Table` use that protocol, but there is no
general arena a caller can install for a scope. The pinned Jai guide describes a
multi-block `Pool`; parity also needs the common one-slab `Flat_Pool` shape.

The protocol creates one non-obvious ownership rule. Once a pool installs
itself, consulting the current context allocator to obtain another block would
call the pool again and recurse. The pool must instead remember the allocator
triple that existed before installation and replay it while requesting or
freeing backing storage.

The two pool shapes make different promises:

- a multi-block arena can grow without moving an earlier allocation;
- a flat arena cannot grow a live slab without invalidating every pointer it
  returned.

Treating them as one implementation with a mode flag would make the second
promise easy to violate.

## Decision

### 1. Both pools capture one backing allocator triple

The first operation that needs backing storage captures
`context.allocator`, `context.allocator_free`, and `context.allocator_data`.
`set_allocators` also performs that capture before replacing the active
context, so a zero-initialised pool is usable either directly or as an
installed allocator.

Every backing allocation/free runs inside `push_context` with the captured
triple restored. If the backing allocator mutates its data word, the pool copies
the updated value back afterward. This is the same ownership seam used by
`Hash_Table` and `String_Builder`.

Capture happens once per pool value and remains associated after final release,
so the same value may allocate again without accidentally capturing its own
installed callbacks as backing storage.

### 2. The context data word carries the owning pool pointer

The callback has only a size argument, so it recovers its owner from
`context.allocator_data`. Private unions reinterpret `*Pool` or `*Flat_Pool`
as the protocol's `s64` word. The inverse conversion happens only in the
corresponding allocator callback.

Each module provides a no-op free callback. Arena allocations are released in
bulk; pretending that an individual pointer can be reclaimed would make
`New`'s matching `allocator_free` call corrupt the cursor or free a whole
shared block.

### 3. Every allocation has a 16-byte cursor granularity

Requests round up to 16 bytes, the maximum alignment the current context
allocator protocol and `New` promise. A zero-byte request consumes 16 bytes and
may therefore return a stable non-null pointer. A negative size, arithmetic
overflow, or backing allocation failure returns null without publishing a
partial state change.

Neither API invents an alignment parameter the context cannot carry.

### 4. `Pool` is a retained multi-block arena

`Pool` owns a linked list of blocks. A zero `memblock_size` selects a 65,536-byte
normal payload. A request larger than that receives a dedicated block sized for
the request; ordinary allocations use retained normal blocks.

Returned addresses never move. `reset` rewinds every block and retains it for
reuse. When `overwrite_memory` is true, reset stamps the formerly used payload
with a private nonzero byte before exposing it as free again. `bytes_left`
describes the currently selected ordinary block and is maintained as public
guide-compatible state.

`release` frees every block through the captured backing free callback, clears
the list/cursor state, and is idempotent. It is the only operation that releases
backing blocks.

The public surface is:

```jr
get(pool: *Pool, size: s64) -> *u8;
set_allocators(pool: *Pool);
pool_allocator_proc(size: s64) -> *u8;
pool_allocator_free_proc(allocation: *u8);
reset(pool: *Pool);
release(pool: *Pool);
```

### 5. `Flat_Pool` refuses relocation while live

`Flat_Pool` owns one `{data, capacity, used}` slab. The first allocation lazily
reserves at least 65,536 bytes. `get` allocates from the cursor and returns null
when the remaining capacity is insufficient.

`reserve` may allocate and publish a larger slab only while `used == 0`. It
leaves an equal/larger slab alone, frees the replaced empty slab only after the
new allocation succeeds, and leaves the old state intact on failure. While
`used != 0`, a growth request returns false rather than moving live results.

`reset(overwrite_memory = false)` optionally stamps the used prefix and rewinds
the cursor. `fini` releases the slab through the captured backing free callback
and is idempotent.

The public surface is:

```jr
get(pool: *Flat_Pool, size: s64) -> *u8;
reserve(pool: *Flat_Pool, capacity: s64) -> bool;
set_allocators(pool: *Flat_Pool);
flat_pool_allocator_proc(size: s64) -> *u8;
flat_pool_allocator_free_proc(allocation: *u8);
reset(pool: *Flat_Pool, overwrite_memory: bool = false);
fini(pool: *Flat_Pool);
```

### 6. Composition is explicit about which allocator each container uses

Inside a `push_context` where a pool is installed, `New(Node)` and
`Hash_Table.Table(string, string)` allocate through that pool. A native
`[..]*Node` has no allocator field, so a test can obtain its backing data from
`Pool.get` and set `data`, `count`, and `capacity` explicitly.

`modules/List` still uses `malloc/free`; its `push` is not silently described as
pool-backed. Changing `List` to capture an allocator would be a separate
ownership decision affecting every existing caller.

The two modules export colliding names such as `get`, `reset`, and
`set_allocators`; programs using both import them qualified.

## Rejected alternatives

### Read the ambient allocator whenever a block is needed

Rejected because, after installation, the ambient allocator is the pool itself.
The first growth would recurse until stack exhaustion.

### Grow a flat pool by reallocating its slab

Rejected because every returned pointer would dangle. Returning null is an
honest bounded-arena result; silently moving live allocations is a false
stability promise.

### Make `List.push` use whichever allocator is active

Rejected in this wave because existing list storage is released through
`free`. Switching only allocation would mismatch ownership, while changing both
halves needs a captured allocator in every dynamic array or a new API contract.

### Let the free callback reclaim individual allocations

Rejected because a bump arena has no per-piece ownership record and may contain
many live values in one block. Bulk reset/release is the defining policy.

### Expose the overwrite stamp value

Rejected because overwrite is debugging invalidation, not data semantics. Only
the fact that formerly live bytes are changed is public.

## Consequences

- Programs can install a stable arena for a lexical `push_context` and use
  ordinary `New`/table APIs inside it.
- Pool lifetime is explicit: `reset`/`release` and `reset`/`fini`; there is no
  destructor or RAII.
- A normal `Pool` may grow beyond 64 KiB without relocating earlier results;
  `Flat_Pool` has one contiguous allocation and reports exhaustion.
- Zero-initialised values are valid starting states and cleanup is idempotent.
- The compiler, MIR, layout, VM, and native back ends are unchanged; the
  ordinary six gates are sufficient for this all-library wave.
- The module count increases by two. The first free global diagnostic code
  remains E0301; the first free parser code remains E0137.

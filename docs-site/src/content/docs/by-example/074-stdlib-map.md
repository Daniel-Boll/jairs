---
title: Map (hash table)
description: An open-addressed hash table from s64 to s64 on the heap, with linear probing, tombstones, and 3/4-load growth.
sidebar:
  order: 74
---

`Map` is a hash table from `s64` to `s64`, open-addressed on the heap. It is the module that most
exercises what the earlier containers built: a heap array of *structs* (typed allocation), grown by
allocate-copy-free like `List`, with field access through pointer arithmetic. Like `List`, it owns heap
memory and there are no destructors, so a caller **must** call `free_map`.

The types are declared parameterised (`struct($K, $V)`) but, as with `Array` and `List`, the operations
are provided only for the concrete `s64 -> s64` instance, because inference through a parameterised
struct and cross-file parameterised structs are still deferred. So a caller declares `Map(s64, s64)`.

## The types

```jr
/// One slot: a key, its value, and whether it is live.
Slot :: struct($K, $V) {
    key: K;         // meaningful only when used
    value: V;       // meaningful only when used
    used: bool;     // true for a live slot
    deleted: bool;  // true for a tombstone — a probe skips it; an insert may reuse it
}

/// A hash table from s64 to s64.
Map :: struct($K, $V) {
    slots: *Slot(K, V);  // null before the first put
    count: s64;          // live entries
    tombstones: s64;     // removed slots not yet reclaimed by a rehash
    capacity: s64;       // a power of two, so a bucket index is a mask rather than a modulo
}

FIRST_CAPACITY :: 8;
```

Three states — empty, live, tombstone — are held in two bools rather than an enum, because using an
`enum` across a module boundary is a path this module need not open when two flags say it plainly.

## The API

```jr
put :: (m: *Map(s64, s64), key: s64, value: s64) -> bool   // insert or update; false only on failed alloc
get :: (m: *Map(s64, s64), key: s64) -> (s64, bool)         // value + whether present
has :: (m: *Map(s64, s64), key: s64) -> bool
remove :: (m: *Map(s64, s64), key: s64) -> bool             // leaves a tombstone
size :: (m: *Map(s64, s64)) -> s64                          // live entries
free_map :: (m: *Map(s64, s64))                             // MUST be called
```

```jr
#import "Basic";
#import "Map";

main :: () {
    n := 0;

    m: Map(s64, s64);
    m.slots = null;
    m.count = 0;
    m.tombstones = 0;
    m.capacity = 0;

    // put / get / size.
    put(*m, 1, 100);
    put(*m, 2, 200);
    put(*m, 3, 300);
    v, ok := get(*m, 2);
    if ok && v == 200 && size(*m) == 3 {
        n = n + 1;
    }

    // Update leaves the count alone.
    put(*m, 2, 999);
    updated, _ := get(*m, 2);
    if updated == 999 && size(*m) == 3 {
        n = n + 2;
    }

    // Absence.
    _, present := get(*m, 42);
    if !present && !has(*m, 42) {
        n = n + 4;
    }

    // Removal leaves a tombstone; a key later on the same probe path stays findable.
    remove(*m, 1);
    if !has(*m, 1) && size(*m) == 2 && has(*m, 3) {
        n = n + 8;
    }

    // A negative key hashes cleanly.
    put(*m, -7, 777);
    neg, negok := get(*m, -7);
    if negok && neg == 777 {
        n = n + 16;
    }

    free_map(*m);

    // Growth: fifty inserts through several rehashes, every key surviving with its value.
    big: Map(s64, s64);
    big.slots = null;
    big.count = 0;
    big.tombstones = 0;
    big.capacity = 0;
    i := 0;
    while i < 50 {
        put(*big, i, i * 3);
        i = i + 1;
    }
    if size(*big) == 50 {
        n = n + 32;
    }
    survived := true;
    j := 0;
    while j < 50 {
        bv, bok := get(*big, j);
        if !bok {
            survived = false;
        }
        if bv != j * 3 {
            survived = false;
        }
        j = j + 1;
    }
    if survived {
        n = n + 64;
    }

    // A key never inserted is still absent after all that growth.
    _, ghost := get(*big, 999);
    if !ghost {
        n = n + 128;
    }

    free_map(*big);

    exit(n);
}
```

The program above (`Map` reaches `malloc`/`free` directly through its `#import "Basic"`, so no allocator
install is needed) inserts and reads back several keys, updates one in place (the count is
unchanged), confirms absence, removes a key (leaving a tombstone) while a later key on the same probe
path stays findable, inserts a **negative** key so the sign bit goes through the hash cleanly, and forces
several rehashes with fifty inserts — every key surviving with its value. The exit code is **255**, and
every bit depends on the two engines computing the same buckets and probe paths, which is the sharp end
of a hash table under a differential harness.

## Why open addressing, and how it grows

One heap allocation holds all the slots and the probe sequence is plain arithmetic, so **both engines
walk it identically** — which the differential harness needs. Separate chaining would need a per-node
allocation and a pointer chase for no benefit at these sizes.

The table grows when the **probe load** — live entries *plus* tombstones — would exceed 3/4, not when it
is completely full. Linear probing degrades sharply past about 3/4, so this is correctness-adjacent
rather than merely a speed knob: `put` checks `(count + tombstones + 1) * 4 > capacity * 3` before
inserting. `grow` doubles the capacity (allocate-copy-free, not `realloc`, for the same reason `List`
avoids it — `realloc`'s in-place behaviour is something the comptime VM does not model), and **rehashes**
every live entry (a slot's bucket depends on the capacity), which drops tombstones by simply not copying
them.

## Tombstones

`remove` leaves a **tombstone** — `used = false, deleted = true` — rather than clearing the slot, because
clearing would break a probe sequence that ran through it to a key stored later. `find_slot` walks
linearly from the key's bucket: it remembers the *first* tombstone on the path (so an insert reuses it
rather than lengthening the probe) but continues past it, because the key may still be live further along;
an empty (never-used, non-tombstone) slot ends the search. Tombstones are reclaimed by the next `grow`.

The bucket is a Fibonacci-style `u64` mix — `h *% 11400714819323198485` (a **wrapping** multiply, `*%`,
because a hash deliberately discards the high bits that overflow; a plain `*` would trap), then an
xorshift, masked with `capacity - 1` (valid because the capacity is a power of two). The mix is pure
arithmetic with no FFI, so both engines compute the same bucket.

`get`, `has` and the two-value return pattern follow the same reasoning as the other containers: every
`s64` is a legitimate value, so `get` returns `(value, present)` rather than a sentinel. `free_map` is
safe on a map that never grew (`slots` is null) and safe twice, because it resets `slots`.

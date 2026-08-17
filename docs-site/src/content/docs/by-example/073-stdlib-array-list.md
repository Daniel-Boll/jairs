---
title: Array & List
description: A fixed-capacity inline array and a heap-backed growable list — two containers with genuinely different ownership contracts, plus views over a raw pointer.
sidebar:
  order: 73
---

`Array` and `List` are the standard library's two sequence containers. They have genuinely **different
ownership contracts**, which is why they are two modules rather than one: an `Array(s64)`'s storage is
inline, so a caller can forget about it; a `List(s64)` owns heap memory and a caller **must** call
`free_data`, because there are no destructors in Jairs.

Both types are declared as parameterised structs (`struct($T)`), but the *operations* are provided only
for the concrete `s64` instance — `push :: (a: *Array(s64), v: s64)` and so on. That is because
**inference through a parameterised struct is deferred**: a generic `push :: (a: *Array($T), v: T)`
cannot bring `T` into scope, so every routine names a concrete instance. Callers therefore write
`Array(s64)` and `List(s64)`.

## Array — fixed capacity, no cleanup

```jr
/// A count and its storage: the array's used prefix is items[0 .. count).
Array :: struct($T) {
    items: [16]T;   // capacity 16; a type argument cannot be a capacity, so the capacity is baked in
    count: s64;
}

CAPACITY :: 16;

push :: (a: *Array(s64), v: s64) -> bool           // false when full
pop :: (a: *Array(s64)) -> (s64, bool)             // element + whether there was one
get :: (a: *Array(s64), index: s64) -> (s64, bool) // in the USED range [0, count)
set :: (a: *Array(s64), index: s64, v: s64) -> bool // replaces; will not extend
clear :: (a: *Array(s64))
is_empty :: (a: *Array(s64)) -> bool
is_full :: (a: *Array(s64)) -> bool
```

```jr
#import "Basic";
#import "Array";

main :: () {
    n := 0;

    a: Array(s64);
    a.count = 0;

    // Empty to start with, and `pop` says so.
    _, empty_ok := pop(*a);
    if is_empty(*a) && !is_full(*a) && !empty_ok {
        n = n + 1;
    }

    // Two pushes, then read them back in order.
    if push(*a, 10) && push(*a, 20) && a.count == 2 {
        n = n + 2;
    }
    first, first_ok := get(*a, 0);
    second, second_ok := get(*a, 1);
    if first_ok && second_ok && first == 10 && second == 20 {
        n = n + 4;
    }

    // `get` refuses an index in [count, CAPACITY) — the slot exists, the element does not.
    _, past := get(*a, 2);
    _, negative := get(*a, -1);
    if !past && !negative {
        n = n + 8;
    }

    // `set` replaces in range and refuses to extend.
    if set(*a, 0, 11) && !set(*a, 2, 99) {
        n = n + 16;
    }
    replaced, _ := get(*a, 0);
    if replaced == 11 {
        n = n + 32;
    }

    // `pop` returns the last element and shortens the array.
    popped, popped_ok := pop(*a);
    if popped_ok && popped == 20 && a.count == 1 {
        n = n + 64;
    }

    // Fill to capacity, confirm the refusal, then clear and confirm it accepts again.
    filling := true;
    while filling {
        if !push(*a, 1) {
            filling = false;
        }
    }
    if a.count == CAPACITY && is_full(*a) && !push(*a, 1) {
        clear(*a);
        if is_empty(*a) && push(*a, 5) {
            n = n + 128;
        }
    }

    exit(n);
}
```

`push` answers **`false` when full** rather than trapping, because filling a fixed buffer is an ordinary
thing a correct program does and handles — unlike indexing past a compiler-known bound, which is a
mistake. It takes the array by *pointer* because it mutates; a by-value struct parameter would append to
a copy. `pop` and `get` return **two values** (element + flag) rather than a sentinel, because every
`s64` is a legitimate element and no value could mean "empty" without excluding it. `get` and `set` bound
on `count`, not `CAPACITY`: reading an unused slot would return the value the declaration zeroed it to — a
real number indistinguishable from a genuine element. The exit code is **255**.

### Why the capacity is baked in

A parameterised struct takes **type** arguments only, so `Array(s64, 16)` is not spellable and a capacity
cannot come from a caller. Sixteen is enough to fill an array in a test without making the struct large; a
caller wanting another size declares their own struct with the same shape.

## List — heap-backed, genuinely growable

`List` is what `Array` could not be: a growable array on the heap, doubling from a first capacity of 4.

```jr
List :: struct($T) {
    data: *T;      // null until the first push; typed via `typed`, since an allocator returns *u8
    count: s64;
    capacity: s64; // zero exactly when data is null
}

FIRST_CAPACITY :: 4;

push :: (a: *List(s64), v: s64) -> bool            // false only on out-of-memory
pop :: (a: *List(s64)) -> (s64, bool)
get :: (a: *List(s64), index: s64) -> (s64, bool)
set :: (a: *List(s64), index: s64, v: s64) -> bool
clear :: (a: *List(s64))                            // forgets elements, keeps the allocation
free_data :: (a: *List(s64))                        // releases storage — MUST be called
is_empty :: (a: *List(s64)) -> bool
elements :: (a: *List(s64)) -> []s64                // a view over the USED prefix
```

```jr
#import "Basic";
#import "List";

main :: () {
    n := 0;

    l: List(s64);
    l.data = null;
    l.count = 0;
    l.capacity = 0;

    // A fresh list allocates nothing, so it is free to declare and safe to free.
    if is_empty(*l) && l.capacity == 0 && l.data == null {
        n = n + 1;
    }

    // Ten pushes through a 4-element first allocation: growth happens twice.
    pushed := true;
    i := 0;
    while i < 10 {
        if !push(*l, i * 3) {
            pushed = false;
        }
        i = i + 1;
    }
    if pushed && l.count == 10 {
        n = n + 2;
    }
    if l.capacity >= 10 && l.capacity > FIRST_CAPACITY {
        n = n + 4;
    }

    // Every element survived both copies — what a broken copy loop would break.
    survived := true;
    j := 0;
    while j < 10 {
        v, ok := get(*l, j);
        if !ok {
            survived = false;
        }
        if v != j * 3 {
            survived = false;
        }
        j = j + 1;
    }
    if survived {
        n = n + 8;
    }

    // `pop` shortens the list and leaves the capacity alone.
    before := l.capacity;
    last, last_ok := pop(*l);
    if last_ok && last == 27 && l.count == 9 && l.capacity == before {
        n = n + 16;
    }

    // The memory exists between count and capacity; the element does not.
    _, past := get(*l, 9);
    if !past && !set(*l, 9, 1) && set(*l, 0, 99) {
        n = n + 32;
    }
    replaced, _ := get(*l, 0);
    if replaced == 99 {
        n = n + 64;
    }

    // `clear` keeps the buffer, `free_data` releases it — deliberately different routines.
    kept := l.capacity;
    clear(*l);
    if is_empty(*l) && l.capacity == kept && push(*l, 1) {
        free_data(*l);
        // Safe twice, and safe on a list that never grew.
        free_data(*l);
        fresh: List(s64);
        fresh.data = null;
        fresh.count = 0;
        fresh.capacity = 0;
        free_data(*fresh);
        if l.data == null && l.capacity == 0 {
            n = n + 128;
        }
    }

    exit(n);
}
```

**Growth doubles** so `n` pushes cost `O(n)` amortised — a fixed increment would be `O(n²)`, a bug
wearing a policy's clothes. A failed allocation is `false`, not a trap, because running out of memory is
not a *program* error and aborting would take away the caller's chance to recover. `get` bounds on
`count` for a sharper reason than `Array`'s: the slots between `count` and `capacity` hold whatever the
allocator returned — genuinely undefined, not merely zeroed. `clear` and `free_data` are deliberately
different: reusing a buffer a caller has paid for is a real thing to want. `free_data` is safe twice and
safe on a list that never grew. The exit code is **255**.

### The divergence writing List caught

`List` was the first construct whose whole point is memory outliving the call that made it, and writing
it caught the first genuine two-engine divergence the differential harness ever found: this program
exited **247 in the comptime VM and 255 natively**. The VM satisfied `malloc` from its own linear region
whose cursor was the frame bump mark, *restored on return* — so heap memory allocated in a callee was
reclaimed when it returned, and reading it back gave zero (release zeroes for determinism, which made the
symptom a clean wrong answer). The fix grows the heap downward from the top, where no frame release
touches it. Nothing had caught it before because nothing before allocated in a callee and used the memory
in the caller.

## Views from a raw pointer: `view`

`elements(a)` hands a list's used prefix to anything taking a `[]s64` — this is what makes the library
*compose*. It is built on the `view(p, n)` intrinsic, which turns a pointer and a count into a view.

```jr
#import "Basic";
#import "List";
#import "Sort";

Pair :: struct {
    a: s64;
    b: s64;
}

main :: () {
    n := 0;

    // A view over a heap allocation, indexed and counted.
    d := typed(s64, malloc(4 * size_of(s64)));
    (d + 0).* = 10;
    (d + 1).* = 20;
    (d + 2).* = 30;
    (d + 3).* = 40;
    v := view(d, 4);
    if v.count == 4 && v[0] == 10 && v[3] == 40 {
        n = n + 1;
    }

    // A shorter view of the same memory: the count is the view's, not the allocation's.
    short := view(d, 2);
    if short.count == 2 && short[1] == 20 {
        n = n + 2;
    }

    // A view is a window: writing through it changes the memory behind it.
    v[0] = 99;
    if (d + 0).* == 99 {
        n = n + 4;
    }
    free(untyped(d));

    // Not scalar-only: a view over a struct element type.
    ps := typed(Pair, malloc(2 * size_of(Pair)));
    pv := view(ps, 2);
    pv[0].a = 3;
    pv[0].b = 4;
    if pv[0].a + pv[0].b == 7 && pv.count == 2 {
        n = n + 8;
    }
    free(untyped(ps));

    // The point of the feature: a growable list's contents, sorted in place by another module.
    l: List(s64);
    l.data = null;
    l.count = 0;
    l.capacity = 0;

    // A zero-count view over an empty list is well-formed — nothing indexes it.
    if elements(*l).count == 0 {
        n = n + 16;
    }

    push(*l, 5);
    push(*l, 1);
    push(*l, 4);
    push(*l, 2);
    push(*l, 3);
    sort_ints(elements(*l));
    first, _ := get(*l, 0);
    last, _ := get(*l, 4);
    if first == 1 && last == 5 && ints_sorted(elements(*l)) {
        n = n + 32;
    }
    free_data(*l);

    exit(n);
}
```

`sort_ints(elements(*l))` is three modules cooperating on one buffer — `List` produces the storage,
`view` windows it, `Sort` orders it in place. A view is a **window, not a copy**: writing through `v[0]`
changes the memory behind it. The element type comes from the pointer, so nothing is asserted — `view` on
a `*s64` is a `[]s64` and cannot be anything else, which is why it takes no type argument.

Honesty markers worth keeping in mind: the count passed to `view` is **unchecked** (a pointer's
allocation size is not tracked anywhere), so `view` is *visible and searchable* rather than *safe*. And a
view is **invalidated by anything that reallocates** — a `push` that grows moves the storage, and
`free_data` frees it — which is the ordinary consequence of a window plus explicit memory, stated because
nothing enforces it. The exit code is **63**.

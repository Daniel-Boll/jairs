---
title: Array & List
description: A fixed-capacity inline array and a heap-backed growable dynamic array — two containers with genuinely different ownership contracts, plus views over a raw pointer.
sidebar:
  order: 73
---

`Array` and `List` are the standard library's two sequence containers, and they have genuinely
**different ownership contracts**, which is why they are two modules rather than one: an `Array(s64)`'s
storage is inline, so a caller can forget about it; `List` owns heap memory, and a caller **must** call
`free_data`, because there are no destructors in Jairs.

`List` is not what it used to be. There is no `List($T)` struct any more: ADR-0136 gave `[..]T` — a
**compiler-known** dynamic array, three words, `{data: *T, count: s64, capacity: s64}` — native syntax
with all three fields as places, and ADR-0140 converted every `List` routine to operate on the native
`*[..]s64` directly, deleting the hand-rolled struct entirely. A caller declares `xs: [..]s64;`
(zero-initialised: `data` is `null`, `count` and `capacity` are `0`) and grows it through `List`'s
procedures.

Both `Array` and `List`'s routines are provided only for the concrete `s64` element type —
`push :: (a: *Array(s64), v: s64)`, `push :: (a: *[..]s64, v: s64)` — and the reason is not that a
parameterised struct is unusable across a module boundary: it is not (ADR-0117); `Array`'s own struct
still declares `struct($T)`, and a `struct($T)` genuinely does cross a module boundary today. What stays
concrete is the *procedures*: an **imported** polymorphic procedure is refused with `E0268`, so a generic
`push :: (a: *Array($T), v: T)` or `push :: (a: *[..]$T, v: T)` would be uncallable by every importer.
Callers therefore write `Array(s64)` for one and a plain `[..]s64` for the other, and both become
`$T`-generic to their callers once cross-file instantiation lands.

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

`List` is what `Array` could not be: a growable array on the heap, doubling from a first capacity of 4 —
except the growable array itself is the language's own `[..]s64`, and `List` is the library code built
directly on top of it rather than a wrapper type of its own.

```jr
// A `[..]s64` is three places the compiler lays out for you: {data: *s64, count: s64, capacity: s64}.
xs: [..]s64;   // zero-initialised: data is null, count and capacity are 0

FIRST_CAPACITY :: 4;

push :: (a: *[..]s64, v: s64) -> bool   // false only on out-of-memory
pop :: (a: *[..]s64) -> (s64, bool)
get :: (a: *[..]s64, index: s64) -> (s64, bool)
set :: (a: *[..]s64, index: s64, v: s64) -> bool
clear :: (a: *[..]s64)                   // forgets elements, keeps the allocation
free_data :: (a: *[..]s64)               // releases storage — MUST be called
is_empty :: (a: *[..]s64) -> bool
elements :: (a: *[..]s64) -> []s64       // a view over the USED prefix
```

A `[..]s64` cannot be **indexed** directly — `xs[0]` is `E0234`, "only a fixed-size array `[N]T` and a
view `[]T` can be indexed" — so `get`/`set` exist for single elements and `elements` hands the whole used
prefix out as a `[]s64` for anything that wants to iterate it, sort it, or search it. `context.allocator`
is required: `List` calls `malloc` and `free` through it (via `Basic`), and needing them is not the same
as having them installed.

```jr
#import "Basic";
#import "List";
#import "Sort";

main :: () {
    n := 0;

    // A zero-initialised native dynamic array: no allocation, empty, safe to free.
    xs: [..]s64;
    if is_empty(*xs) && xs.count == 0 && xs.capacity == 0 && xs.data == null {
        n = n + 1;
    }

    // Eight pushes through a 4-element first allocation: growth happens once, and the capacity ends
    // above the first. Every element must survive the reallocation's copy.
    pushed := true;
    i := 0;
    while i < 8 {
        if !push(*xs, i * 2) {
            pushed = false;
        }
        i = i + 1;
    }
    if pushed && xs.count == 8 && xs.capacity >= 8 && xs.capacity > FIRST_CAPACITY {
        n = n + 2;
    }

    // Every element readable after growth — what a broken copy loop would break.
    survived := true;
    j := 0;
    while j < 8 {
        v, ok := get(*xs, j);
        if !ok || v != j * 2 {
            survived = false;
        }
        j = j + 1;
    }
    if survived {
        n = n + 4;
    }

    // `pop` shortens the list; `set` refuses an index past `count`.
    last, last_ok := pop(*xs);
    if last_ok && last == 14 && xs.count == 7 && set(*xs, 0, 99) && !set(*xs, 7, 1) {
        n = n + 8;
    }

    // `[..]s64` cannot be indexed directly — `elements` hands its used prefix to Sort as a `[]s64`.
    sort_ints(elements(*xs));
    first, _ := get(*xs, 0);
    top, _ := get(*xs, 6);
    if first == 2 && top == 99 && ints_sorted(elements(*xs)) {
        n = n + 16;
    }

    // The caller owns the storage; nothing else frees it.
    free_data(*xs);

    exit(n);
}
```

**Growth doubles** so `n` pushes cost `O(n)` amortised — a fixed increment would be `O(n²)`, a bug
wearing a policy's clothes. A failed allocation is `false`, not a trap, because running out of memory is
not a *program* error and aborting would take away the caller's chance to recover. `get` bounds on
`count` for a sharper reason than `Array`'s: the slots between `count` and `capacity` hold whatever the
allocator returned — genuinely undefined, not merely zeroed. `clear` and `free_data` are deliberately
different: reusing a buffer a caller has paid for is a real thing to want. `free_data` is safe twice and
safe on a list that never grew. The exit code is **31**.

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

    // The point of the feature: a growable dynamic array's contents, sorted in place by another module.
    l: [..]s64;

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

See also [Book I — The Jairs Language](/language/introduction/).

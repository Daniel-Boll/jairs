---
title: Memoising with a hash map
description: Cache an expensive recursive computation in a Map, and manage the heap it owns.
sidebar:
  order: 3
---

Naïve recursive Fibonacci recomputes the same values exponentially often. This program fixes
that with a [`Map`](/language/the-standard-library/#map) cache: each result is computed once
and remembered. Along the way it shows how a heap-owning data structure is created, used, and
freed in a language with no garbage collector and no destructors.

```jr
#import "Basic";
#import "Map";

// Fibonacci, memoised in a hash map the caller owns. Each result is computed once;
// every later request for the same `n` is a map lookup.
fib :: (m: *Map(s64, s64), n: s64) -> s64 {
    if n < 2 {
        return n;
    }
    cached, hit := get(m, n);
    if hit {
        return cached;
    }
    result := fib(m, n - 1) + fib(m, n - 2);
    put(m, n, result);
    return result;
}

main :: () {
    m: Map(s64, s64);
    m.slots = null;
    m.count = 0;
    m.tombstones = 0;
    m.capacity = 0;

    i := 0;
    while i <= 30 {
        print_int(fib(*m, i));
        print(" ");
        i = i + 1;
    }
    print("\n");

    // The map owns heap memory, and there are no destructors, so we free it.
    free_map(*m);
}
```

Output:

```
0 1 1 2 3 5 8 13 21 34 55 89 144 233 377 610 987 1597 2584 4181 6765 10946 17711 28657 46368 75025 121393 196418 317811 514229 832040
```

## How it works

**Initialising the map.** `Map(s64, s64)` is a `s64 → s64` hash table. A fresh one is set up
by zeroing its four fields — `slots` is the (as-yet unallocated) heap array, and `count`,
`tombstones` and `capacity` describe its occupancy. Setting `slots = null` is what tells the
first `put` to allocate.

**The cache lookup.** `get(m, n)` returns `(value, true)` on a hit and `(_, false)` on a
miss — the same [two-value shape](/language/errors-and-traps/#errors-are-values) as the
bracket checker's `pop`. On a hit we return immediately; on a miss we compute, `put` the
result, and return it. Because the map is threaded through as a pointer, every recursive call
shares the one cache.

**Recursion over a shared pointer.** `fib` takes `*Map(s64, s64)` and passes `m` straight
down (`m` is already a pointer). This is how a growing, heap-backed structure is shared
across a call tree without copying it.

**Freeing what we own.** `Map` allocates on the heap, so it **owns** memory — and Jairs has
no destructors, so nothing frees it for us. We call `free_map(*m)` when done. A type that owns
memory says so in its API by giving you a `free_*` to call; forgetting it is a leak the
compiler does not catch, which is the trade for having no GC. (The map itself grows as needed
— behind `put`, the table doubles and rehashes past 3/4 load.)

## What it demonstrates

- The `Map` module as a cache, with `get` / `put` / `free_map`.
- Sharing a heap structure across recursion via a pointer parameter.
- Explicit ownership: a heap-owning type is freed by the caller, with `defer` or a direct
  call, because there is no garbage collector — see [Memory](/language/memory/).

Next: [a deterministic dice simulation](/in-practice/dice-simulation/).

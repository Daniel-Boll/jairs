---
title: Sort
description: In-place sorting over a view, for any element type, given a comparison — a stable insertion sort, an unstable heap sort, and a stable merge sort.
sidebar:
  order: 72
---

`Sort` orders a view in place, for any element type, given a comparison. It is the standard library's
third module and the first that is *polymorphic* — so the first that depends on the language's generics
rather than merely coexisting with them.

## The API

```jr
/// Sorts `xs` in place by `less`. STABLE (insertion sort), no allocation, O(n²) comparisons worst case.
sort :: (xs: []$T, less: (T, T) -> bool, comparisons: *s64)

/// Sorts a view of s64 ascending. A named convenience so the common case needs no comparison spelled out.
sort_ints :: (xs: []s64)

/// Sorts a view of s64 ascending, reporting how many comparisons it took.
sort_ints_counting :: (xs: []s64, comparisons: *s64)

/// Sorts `xs` in place by `less` in O(n log n) comparisons, UNSTABLY (heapsort). In place, no allocation.
heap_sort :: (xs: []$T, less: (T, T) -> bool, comparisons: *s64)

/// Sorts a view of s64 ascending with heap_sort.
heap_sort_ints :: (xs: []s64)

/// Sorts a view of s64 ascending with heap_sort, reporting the comparison count.
heap_sort_ints_counting :: (xs: []s64, comparisons: *s64)

/// Sorts `xs` in place STABLY in O(n log n) — bottom-up merge sort taking scratch from talloc, falling
/// back to insertion sort when the arena has no room, so the answer never depends on memory pressure.
stable_sort :: (xs: []$T, less: (T, T) -> bool, comparisons: *s64)

/// stable_sort at s64 with less, for a caller in another module.
stable_sort_ints :: (xs: []s64, less: (s64, s64) -> bool, comparisons: *s64)

/// Whether `xs` is ordered by `less`.
is_sorted :: (xs: []$T, less: (T, T) -> bool) -> bool

/// Whether a view of s64 is ordered ascending.
ints_sorted :: (xs: []s64) -> bool

/// Whether `xs` is in non-decreasing order by `less`, for a caller in another module.
ints_sorted_by :: (xs: []s64, less: (s64, s64) -> bool) -> bool

/// Ascending order for s64.
less_int :: (a: s64, b: s64) -> bool
```

`comparisons: *s64` is **mandatory** on every polymorphic entry point rather than optional, so there is
no null check in the comparison loop and no second copy of either algorithm.

```jr
#import "Basic";
#import "Sort";

main :: () {
    n := 0;

    // A reversed array — insertion sort's worst case, and the case that actually exercises the shifting.
    a: [5]s64;
    a[0] = 5;
    a[1] = 4;
    a[2] = 3;
    a[3] = 2;
    a[4] = 1;
    sort_ints(a[]);
    if a[0] == 1 && a[1] == 2 && a[2] == 3 && a[3] == 4 && a[4] == 5 {
        n = n + 1;
    }
    if ints_sorted(a[]) {
        n = n + 2;
    }

    // Duplicates, so stability has something to preserve — and so a lost or repeated element shows up.
    b: [6]s64;
    b[0] = 3;
    b[1] = 1;
    b[2] = 3;
    b[3] = 2;
    b[4] = 1;
    b[5] = 2;
    sort_ints(b[]);
    if b[0] == 1 && b[1] == 1 && b[2] == 2 && b[3] == 2 && b[4] == 3 && b[5] == 3 {
        n = n + 4;
    }

    // Already sorted: it must come out unchanged rather than merely ordered.
    c: [3]s64;
    c[0] = 7;
    c[1] = 8;
    c[2] = 9;
    sort_ints(c[]);
    if c[0] == 7 && c[1] == 8 && c[2] == 9 {
        n = n + 8;
    }

    // One element, which the loop never enters, and which is ordered.
    d: [1]s64;
    d[0] = 42;
    sort_ints(d[]);
    if d[0] == 42 && ints_sorted(d[]) {
        n = n + 16;
    }

    // The check that keeps the rest honest: an unsorted view must be reported unsorted.
    e: [3]s64;
    e[0] = 2;
    e[1] = 1;
    e[2] = 3;
    if !ints_sorted(e[]) {
        n = n + 32;
    }

    exit(n);
}
```

Note `sort_ints(a[])`: the `a[]` slices the fixed array `a` into a `[]s64` view, and a view parameter is
**mutable through the callee**, which is what lets an in-place sort exist at all. The exit code is **63**
— six independent groups, each contributing one bit. The last group, `!ints_sorted(e[])`, is what keeps
the rest honest: without it, a `sort` that did nothing at all would satisfy every assertion that only
reads a sorted array.

## Why the caller supplies the comparison

`sort(xs, less, comparisons)` takes a procedure rather than requiring `<` on the element type, and that is
a language fact rather than a taste. Resolving an *operator* inside a `$T` template against the
instantiated type is a lookup that generic instantiation does not do: `operator <` exists and a `#modify`
predicate can *reject* an instantiation, but nothing can *select* an implementation per instantiated type.
That would be operator-bounded polymorphism, a real feature belonging to whichever wave decides how a
template states its requirements — so a caller must always supply `less`, on every entry point, with no
exception. A comparison parameter is also the only form that serves a scalar **and** a struct with nothing
the language lacks — and it composes with `String.compare`.

Three language facts were probed before a line was written, and all three hold: a view parameter is
mutable, a `$T` parameter infers through a view (`xs: []$T`), and a procedure pointer can be passed and
called.

## Which sort to use

Three algorithms exist, and each earns its place rather than duplicating the others:

- **`sort` / `sort_ints`** — insertion sort. `O(n²)`, stated plainly, but **stable**, allocates nothing,
  and is short enough to read, which mattered for the first sorting routine in a language whose test
  suite compares two independent engines.
- **`heap_sort` / `heap_sort_ints`** — `O(n log n)` comparisons in the *worst* case rather than on
  average, in place, no allocation. **Unstable**: equal elements can change relative order.
  (ADR-0146 §4.)
- **`stable_sort` / `stable_sort_ints`** — `O(n log n)`, and **stable**, like insertion sort. A bottom-up
  merge sort taking its scratch from `talloc`, and falling back to insertion sort when the arena has no
  room, so the answer never depends on memory pressure. (ADR-0155.)

A few designs were tried and are <span class="jairs-status refused">refused</span>, on purpose:

- **No hybrid** (insertion below a threshold, heapsort above), which is what a production sort typically
  does: it would make stability depend on the *input size*, so a program would work on small inputs and
  silently reorder equal keys on large ones. Stability is observable behaviour, not an implementation
  quality, and swapping the algorithm silently would change what an existing program computes.
- **No quicksort**, whose worst case is `O(n²)` on adversarial input and which needs a pivot argument.
- **No caller-supplied scratch buffer** for `stable_sort` — the fastest option, but it makes every call
  site carry a parameter that is always the same expression; a version doing exactly that was written and
  removed. `malloc`/`free` per call was rejected too: it pays an allocator round trip per sort and leaks on
  a trap.

```jr
#import "Basic";
#import "Sort";

main :: () {
    n := 0;

    // heap_sort_ints: unstable, O(n log n) worst case, in place.
    a: [5]s64;
    a[0] = 5;
    a[1] = 3;
    a[2] = 1;
    a[3] = 4;
    a[4] = 2;
    heap_sort_ints(a[]);
    if a[0] == 1 && a[1] == 2 && a[2] == 3 && a[3] == 4 && a[4] == 5 && ints_sorted(a[]) {
        n = n + 1;
    }

    // stable_sort_ints, checked against ints_sorted_by and its own comparison count.
    comparisons := 0;
    b: [4]s64;
    b[0] = 9;
    b[1] = 9;
    b[2] = 1;
    b[3] = 1;
    stable_sort_ints(b[], less_int, *comparisons);
    if b[0] == 1 && b[1] == 1 && b[2] == 9 && b[3] == 9 {
        if ints_sorted_by(b[], less_int) && comparisons > 0 {
            n = n + 2;
        }
    }

    // heap_sort_ints_counting: the counted twin, for a caller that wants the comparison count.
    c: [3]s64;
    c[0] = 30;
    c[1] = 10;
    c[2] = 20;
    heap_sort_ints_counting(c[], *comparisons);
    if c[0] == 10 && c[1] == 20 && c[2] == 30 {
        n = n + 4;
    }

    exit(n);
}
```

The exit code is **7**.

## Why there are `_ints` wrappers

**Cross-file instantiation is deferred** (`E0268`): an *importing* file cannot instantiate `sort`,
`heap_sort`, `stable_sort` or `is_sorted` directly — the call is refused, because instantiating an
imported template is `E0268`. The workaround is a wrapper *in the declaring module*, where the
instantiation can happen. So `sort_ints`, `sort_ints_counting`, `heap_sort_ints`,
`heap_sort_ints_counting`, `stable_sort_ints` and `ints_sorted`/`ints_sorted_by` are not conveniences
today — they are **the only way an importer can use this module**, and they will become conveniences when
cross-file instantiation arrives. `less_int` is exported so a caller can compose it (sorting descending
with a wrapper, or passing it to `is_sorted`/`ints_sorted_by` to check a postcondition).

See also [Book I — The Jairs Language](/language/introduction/).

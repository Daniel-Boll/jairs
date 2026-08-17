---
title: Sort
description: An in-place, stable insertion sort over a view, for any element type, given a comparison procedure.
sidebar:
  order: 72
---

`Sort` orders a view in place, for any element type, given a comparison. It is the standard library's
third module and the first that is *polymorphic* — so the first that depends on the language's generics
rather than merely coexisting with them.

## The API

```jr
/// Sorts `xs` in place, ordering by `less`. Stable; O(n²).
sort :: (xs: []$T, less: (T, T) -> bool)

/// Sorts a view of s64 ascending.
sort_ints :: (xs: []s64)

/// Whether `xs` is ordered by `less`.
is_sorted :: (xs: []$T, less: (T, T) -> bool) -> bool

/// Whether a view of s64 is ordered ascending.
ints_sorted :: (xs: []s64) -> bool

/// Ascending order for s64.
less_int :: (a: s64, b: s64) -> bool
```

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
**mutable through the callee**, which is what lets an in-place sort exist at all.

## Why the caller supplies the comparison

`sort(xs, less)` takes a procedure rather than requiring `<` on the element type, and that is a language
fact rather than a taste. Resolving an *operator* inside a `$T` template against the instantiated type is
a lookup that generic instantiation does not do: `operator <` exists and a `#modify` predicate can
*reject* an instantiation, but nothing can *select* an implementation per instantiated type. That would
be operator-bounded polymorphism, a real feature belonging to whichever wave decides how a template
states its requirements. A comparison parameter is also the only form that serves a scalar **and** a
struct with nothing the language lacks — and it composes with `String.compare`.

Three language facts were probed before a line was written, and all three hold: a view parameter is
mutable, a `$T` parameter infers through a view (`xs: []$T`), and a procedure pointer can be passed and
called.

## Why insertion sort

It is `O(n²)`, stated plainly. The reasons to choose it *here* are not performance:

- **Stable** — equal elements keep their relative order, which quicksort does not give.
- **No extra storage** — a merge sort would allocate, and the allocation convention is a separate
  decision.
- **Short enough to read**, which matters for the first sorting routine in a language whose test suite
  compares two independent engines.

A faster algorithm is a later decision with a benchmark behind it.

## Why there are `_ints` wrappers

**Cross-file instantiation is deferred**: an *importing* file cannot instantiate `sort` or `is_sorted`
directly — the call is refused. The workaround is a wrapper *in the declaring module*, where the
instantiation can happen. So `sort_ints` and `ints_sorted` are not conveniences today — they are the only
way an importer can use this module, and they will become conveniences when cross-file instantiation
arrives. `less_int` is exported so a caller can compose it (sorting descending with a wrapper, or passing
it to `is_sorted` to check a postcondition).

The exit code is **63** — six independent groups each contributing one bit. The last group,
`!ints_sorted(e[])`, is what keeps the rest honest: without it, a `sort` that did nothing at all would
satisfy every assertion that only reads a sorted array.

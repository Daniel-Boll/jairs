---
title: Views ([]T)
description: "[]T views: a pointer-and-length pair that lets one procedure serve every array length, with no implicit conversion and write-through semantics."
sidebar:
  order: 28
---

A view `[]T` is a `{data: *T, count: s64}` pair — the same two words a `string` is (ADR-0044). Where a `[N]T` parameter works for exactly one length, a view lets a single procedure serve arrays of every size. It is created from an array with the `[]` slice operator, carries no implicit conversion, and writes through to the storage it was made from.

```jr
#import "Basic";

/// One procedure, every length -- which is the whole point.
total :: (xs: []s64) -> s64 {
    i := 0;
    t := 0;
    while i < xs.count {
        t = t + xs[i];
        i = i + 1;
    }
    return t;
}

/// A view is a pointer to storage, not a copy of it.
fill :: (xs: []s64, value: s64) {
    i := 0;
    while i < xs.count {
        xs[i] = value;
        i = i + 1;
    }
}

/// `.count` crossing a procedure boundary.
length_of :: (xs: []s64) -> s64 {
    return xs.count;
}

main :: () {
    n := 0;

    four: [4]s64;
    four[0] = 1;
    four[1] = 2;
    four[2] = 4;
    four[3] = 8;

    two: [2]s64;
    two[0] = 16;
    two[1] = 32;

    // The same procedure over two different lengths.
    if total(four[]) == 15 {
        n = n + 1;
    }
    if total(two[]) == 48 {
        n = n + 2;
    }

    // The length is runtime data, so it differs per view.
    if length_of(four[]) == 4 {
        n = n + 4;
    }

    // A view in a local, indexed directly.
    xs := four[];
    if xs.count == 4 {
        n = n + 32;
    }

    // Writing through a view is visible in the array it was made from.
    fill(two[], 7);
    if two[0] == 7 {
        n = n + 64;
    }

    // Assigning through a view held in a local, then reading the *array*.
    xs[0] = 100;
    if four[0] == 100 {
        n = n + 256;
    }

    // A view copies as two words: `ys` and `xs` name the same storage.
    ys := xs;
    ys[3] = 9;
    if four[3] == 9 {
        n = n + 4096;
    }

    // Slicing through a pointer, which auto-derefs exactly as indexing does.
    p := *four;
    if total(p[]) == 115 {
        n = n + 8192;
    }

    if n == 16383 {
        exit(0);
    }
    exit(1);
}
```

The reason views exist is `total`: it sums a `[]s64` regardless of length, so `total(four[])` and `total(two[])` are the same procedure, where under `[N]T` they would have had to be two. A view is a pointer plus a count, so one function body serves every size.

Three rules are worth stating:

- **There is no implicit conversion.** `total(buf)` is an error; you must write `total(buf[])`. The `[]` operator explicitly forms the view from the array. An implicit conversion would take the array's address invisibly — and would be the language's first implicit conversion, which ADR-0044 §2 declined to introduce.
- **`.count` on a view is a load** of the second word, not a compile-time constant. Where a `[N]T`'s `.count` comes from the type, a view's length is runtime data, so it can differ between two calls to the same procedure — which is exactly why `length_of` returns different answers for `four[]` and `two[]`.
- **A view writes through** to the storage it was made from. `fill(two[], 7)` mutates `two` itself; assigning `xs[0] = 100` through a view held in a local is visible when the *array* `four` is read back. That write-through is what makes passing a view worth doing.

Because a view is just two words, copying one (`ys := xs`) copies the pointer and count — so `ys` and `xs` name the *same* storage, and `ys[3] = 9` shows up in `four`. Slicing also auto-dereferences through a pointer: with `p := *four`, `p[]` produces a view of the pointed-to array exactly as indexing through a pointer would. The full corpus file additionally shows a `[]u8` view over a byte buffer, including a zero-initialised element read back through the view.

As with the other computational corpus files, the trailing `exit(0)` fires only when every assertion holds (`n == 16383`), making the whole run observable through the exit status so the two engines can be checked to agree.

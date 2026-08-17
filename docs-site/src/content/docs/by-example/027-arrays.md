---
title: Fixed arrays
description: "[N]T fixed arrays: zero-initialisation, indexing as a place, the .count constant, bounds checks and #no_abc, and a named length constant."
sidebar:
  order: 27
---

A `[N]T` is a fixed-size array of `N` elements of type `T`, laid out inline (ADR-0039). This page covers its basics, the `#no_abc` opt-out of bounds checking, and letting a named constant supply the length.

## Declaration, indexing, and .count

```jr
#import "Basic";

main :: () {
    // No initialiser, so every element is zero.
    buf: [4]u8;

    // Indexing is a place, so it assigns as well as reads.
    buf[0] = 65;
    buf[1] = 66;
    buf[3] = 1;

    // `.count` is the length from the *type* -- a compile-time constant.
    total := 0;
    i := 0;
    while i < buf.count {
        total = total + cast(s64, buf[i]);
        i = i + 1;
    }

    // A computed index, so the bounds check is a real runtime check.
    last := buf[buf.count - 1];

    // An array of a wider element, to exercise a stride that is not 1.
    words: [3]s64;
    words[2] = 7;
    words[0] = words[2] * 2;

    // An array of a struct.
    dots: [2]Point;
    dots[1].x = 5;
    dots[1].y = 6;

    // 65 + 66 + 0 + 1 = 132
    if total == 132 {
        if last == 1 {
            if words[0] == 14 {
                if dots[1].x + dots[1].y == 11 {
                    if dots[0].x == 0 {
                        exit(0);
                    }
                }
            }
        }
    }
    exit(1);
}

Point :: struct {
    x: s64;
    y: s64;
}
```

The key properties:

- **A declaration with no initialiser zero-fills every element** (ADR-0039 §4). This is the *defined* behaviour, not luck — the MIR tracks definedness per slot, and a partial write would otherwise poison the whole array. So `buf[2]`, never written, reads as `0`.
- **Indexing is a place.** `buf[0] = 65` assigns, and `buf[i]` in the loop reads — the same syntax on either side.
- **`.count` is the length from the type**, a compile-time constant rather than a load (ADR-0039 §5). It lets a `while` over the array name its bound once (there is no `for` in this file's era).
- **Bounds checks are real runtime checks** for a computed index like `buf[buf.count - 1]`, not something folded away.
- **Stride follows the element type**: `[3]s64` steps by 8 bytes, `[2]Point` by a struct's layout, and an aggregate array's untouched elements are still zeroed (`dots[0].x == 0`).

## Turning bounds checks off: #no_abc

```jr
#import "Basic";

/// Indexes with the check suppressed. The index comes from a **parameter**.
read_fast :: (buf: [4]s64, i: s64) -> s64 #no_abc {
    return buf[i];
}

/// The same body without the directive, so its check survives.
read_safe :: (buf: [4]s64, i: s64) -> s64 {
    return buf[i];
}

/// A `for` over an array also emits a check per element, at a second site.
sum_fast :: (buf: [4]s64) -> s64 #no_abc {
    total := 0;
    for x: buf {
        total = total + x;
    }
    return total;
}

/// Both attributes on one procedure.
raw_fast :: (buf: [4]s64, i: s64) -> s64 #no_abc #c_call {
    return buf[i];
}

main :: () {
    n := 0;

    xs: [4]s64;
    xs[0] = 1;
    xs[1] = 2;
    xs[2] = 4;
    xs[3] = 8;

    // The unchecked read and the checked one must agree, in range.
    if read_fast(xs, 2) == 4 {
        n = n + 1;
    }
    if read_fast(xs, 2) == read_safe(xs, 2) {
        n = n + 4;
    }
    if sum_fast(xs) == 15 {
        n = n + 8;
    }
    if raw_fast(xs, 3) == 8 {
        n = n + 16;
    }
    // ...
    if n == 127 {
        exit(0);
    }
    exit(1);
}
```

Bounds checking is a build setting (ADR-0003), carried as an explicit MIR operation that a pass strips, with a per-procedure opt-out written `#no_abc`.

Honestly, **this file cannot observe the interesting thing**, and the corpus says so. A stripped bounds check is invisible in any program that stays in range — and a corpus file must run cleanly, so every index here *is* in range. Reading `buf[9]` with checks off would read whatever is at that address, which is undefined behaviour by construction; a test asserting what that produces would be asserting a fact about one machine's stack. So the direct evidence for the pass lives elsewhere (a test counting `BoundsCheck` statements under each setting). What this file *does* prove is the observable half: that `#no_abc` parses, formats, checks, lowers, and runs.

`read_fast` and `read_safe` share a body but differ by the directive, so the difference is a property of the *declaration*, not the build — a file-level reading of `#no_abc` would give them identical MIR. The index comes from a parameter, so nothing in the mid-end can prove it in range and const-fold both checks into agreement. `sum_fast` shows the directive reaching a *second* check-emission site (the `for` loop's per-element check). And `raw_fast` carries `#no_abc #c_call` together — the two are independent and legal in either order, since an ordering rule is one no reader could guess.

## A named length constant

```jr
#import "Basic";

N :: 4;
M :: 2;

/// Sums an `[N]s64` **by name**.
total :: (xs: [N]s64) -> s64 {
    t := 0;
    for x: xs {
        t = t + x;
    }
    return t;
}

main :: () {
    n := 0;

    buf: [N]s64;
    buf[0] = 1;
    buf[1] = 2;
    buf[2] = 3;
    buf[3] = 4;
    if total(buf) == 10 {
        n = n + 1;
    }
    if buf.count == 4 {
        n = n + 2;
    }

    // The same constant naming a **different element type**.
    flags: [N]bool;
    flags[3] = true;

    // Nested, with two different constants.
    grid: [N][M]s64;
    grid[3][1] = 7;
    if grid[3][1] == 7 {
        n = n + 8;
    }

    if n == 15 {
        exit(0);
    }
    exit(1);
}
```

An array length may **name a literal-valued constant** (ADR-0070). `N :: 4` makes `[N]s64` an array of four, indexable to 3 and no further, and the constant works on a parameter type (`total`) as well as a local. The same `N` names the length of a *different* element type (`[N]bool`), so a length is resolved per use rather than baked into one type, and it nests (`[N][M]s64`).

This is an amendment to the earlier rule that refused `[COUNT]u8`, not a reversal of it. The earlier objection was that evaluating a length would invert a crate dependency — true for a length that needs *evaluation* (arithmetic, a `#run`, or a constant naming another constant, all still errors), but too broad for a bare literal constant like `N :: 4`, where the value is already present and only the *name* needs resolving. The length still reaches the layout as a plain integer, so nothing downstream can tell how it was written — which is the evidence it belongs in type resolution rather than later.

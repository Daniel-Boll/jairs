---
title: Pointer arithmetic
description: Element-scaled, unchecked `p + n`, `n + p` and `p - n` — the operation that lets a pointer walk an array or a bump arena.
sidebar:
  order: 44
---

A pointer that cannot move is not much of a pointer. This page adds `p + n`, `n + p`, and `p - n`:
addition and subtraction that advance a pointer by whole elements, unchecked. It is what lets a
program walk an array cell by cell, and what lets a bump allocator carve a region into slices.

```jr
#import "Basic";

/// Fills `n` s64 cells by advancing a pointer one element at a time,
/// then reads two back through fresh offsets.
walk_s64 :: () -> s64 {
    a: [4]s64;
    base := *a[0];

    // Write through advancing element offsets: 10, 20, 30, 40.
    i := 0;
    while i < 4 {
        p := base + i;
        p.* = (i + 1) * 10;
        i = i + 1;
    }

    total := 0;

    // `p + n`: the third cell is 30.
    third := base + 2;
    if third.* == 30 {
        total = total + 1;
    }
    // `n + p`: addition commutes, so `2 + base` is the same cell.
    also_third := 2 + base;
    if also_third.* == 30 {
        total = total + 2;
    }
    // `p - n`: from the fourth cell, back two, is the second — 20.
    back := (base + 3) - 2;
    if back.* == 20 {
        total = total + 4;
    }
    return total;
}

/// A bump allocator over a *u8 region: hand out s64-sized cells by
/// advancing a byte pointer by 8.
bump :: () -> s64 {
    region := malloc(64);
    if region == null {
        return 0;
    }

    // Two cells, 8 bytes apart.
    cell0 := region;
    cell1 := region + 8;
    cell0.* = 111;
    cell1.* = 222;

    // Read back through independently-computed offsets.
    r0 := region + 0;
    r1 := region + 8;
    got := 0;
    if r0.* == 111 {
        got = got + 8;
    }
    if r1.* == 222 {
        got = got + 16;
    }

    free(region);
    return got;
}

main :: () {
    n := walk_s64() + bump();
    if n == 31 {
        exit(0);
    }
    exit(1);
}
```

## Element-scaled, not byte-scaled

This is the fork that matters. On a `*s64`, `base + 1` advances *eight* bytes, not one — so
`(base + 1).*` reads the second `s64`, not the second byte. `walk_s64` fills a `[4]s64` by advancing
`base` one element at a time, then reads the third cell through a fresh `base + 2` and finds `30`. A
byte-scaled implementation would compute the wrong address here and read garbage. The read and the
write are computed independently, so a matching value proves `p + i` is a stable address rather than
an accident.

## Commuting and subtracting

Addition commutes: `2 + base` reaches the same element as `base + 2`, and both read `30`.
Subtraction moves back by elements too — `(base + 3) - 2` lands on the second cell and reads `20`.
The three offset forms together confirm direction and stride.

## The bump-allocator case

`bump` walks a `*u8` region from `malloc`. On a byte pointer element and byte scaling coincide, so
advancing by `8` steps over one `s64`-sized cell. Writing `111` and `222` into two cells eight bytes
apart and reading them back through independently-recomputed offsets (`region + 0`, `region + 8`)
shows the addresses are stable. This is the motivating case — exactly what a temporary-storage arena
does.

## Unchecked

Pointer arithmetic is unchecked. A raw pointer carries no length, so nothing traps on an offset,
in-bounds or not — the program is responsible for staying inside its allocation. Both functions here
stay in bounds deliberately; the language does not police it.

## Teeth

`walk_s64` returns `7` (1 + 2 + 4) and `bump` returns `24` (8 + 16), summing to `31`. A wrong
stride, a wrong direction, or a byte-versus-element confusion would produce a different total and a
different exit status. The engines compute the scaled addresses from one shared stride value, so
they agree despite allocating the region from different sources.

See also [Book I — The Jairs Language](/language/introduction/).

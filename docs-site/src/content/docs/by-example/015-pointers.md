---
title: Pointers
description: Prefix `*` to take an address, postfix `.*` to dereference, pointer-to-pointer, and field access that auto-dereferences.
sidebar:
  order: 15
---

Jairs deliberately splits pointer syntax so that taking an address and dereferencing never look
alike. Prefix `*` takes an address; postfix `.*` dereferences. `*T` is the pointer type.

```jr
Point :: struct {
    x: s64;
    y: s64;
}

main :: () {
    value := 42;

    // Prefix `*` takes an address. `*T` is the pointer type.
    p: *s64 = *value;

    // Postfix `.*` dereferences. Postfix avoids C's ambiguity between
    // dereference and multiplication.
    copied := p.*;
    p.* = 43;

    // Pointers to aggregates: field access auto-dereferences.
    origin: Point;
    pp := *origin;
    pp.x = 1;

    // Pointer to pointer.
    ppp: **s64 = *p;
    round_trip := ppp.*.*;
}
```

## Address-of and dereference

Prefix `*` is the address-of operator: `*value` produces a pointer, and the type of that
pointer is written `*s64`. To read through a pointer you use the **postfix** `.*` form:
`copied := p.*` loads the pointee, and `p.* = 43` stores through it. Putting the dereference on
the right, as a postfix, is what avoids C's visual clash between `*p` (dereference) and `a * b`
(multiplication) — in Jairs the two never collide.

## Auto-dereferencing field access

When a pointer targets an aggregate, field access reaches through it automatically. `pp` is a
`*Point`, yet `pp.x = 1` writes the field directly — you don't have to spell out `pp.*.x`. The
compiler dereferences for you.

## Pointer to pointer

Pointers compose: `**s64` is a pointer to a pointer to an `s64`. `ppp := *p` takes the address
of the pointer `p`, and `round_trip := ppp.*.*` dereferences twice to get back to the original
value.

## Field access through nested structs

The companion example shows field paths without pointers, walking into nested structs:

```jr
Point :: struct {
    x: s64;
    y: s64;
}

Line :: struct {
    from: Point;
    to: Point;
}

main :: () {
    line: Line;
    line.from.x = 1;
    line.from.y = 2;
    line.to.x = 3;
    line.to.y = 4;

    dx := line.to.x - line.from.x;
}
```

A `Line` holds two `Point`s, so a path like `line.to.x` chains field accesses to reach the
`x` of the `to` point. The final line reads two such paths and subtracts them.

See also [Book I — The Jairs Language](/language/introduction/).

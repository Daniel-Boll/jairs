---
title: Operator overloading
description: An operator can be given a meaning for user-defined types, resolved by exact match on both operands.
sidebar:
  order: 34
---

Jairs lets you define an operator's meaning for your own types (ADR-0048). An overload is written
`operator OP :: (…)` — the ordinary `name :: value` constant form, with an operator where the
name goes. The name interns as a synthetic symbol (`operator+`) that no user can write, and
because of that it lands in the same flat per-file name map every other constant does, so
importing, shadowing, and ambiguity reporting all work with no new mechanism.

```jr
#import "Basic";

Vec2 :: struct {
    x: s64;
    y: s64;
}

Pair :: struct {
    lo: s64;
    hi: s64;
}

operator + :: (a: Vec2, b: Vec2) -> s64 {
    return a.x + b.x + a.y + b.y;
}

operator * :: (a: Vec2, b: s64) -> s64 {
    return (a.x + a.y) * b;
}

operator * :: (a: s64, b: Vec2) -> s64 {
    return a * (b.x + b.y);
}

operator + :: (a: Pair, b: Pair) -> s64 {
    return a.lo + b.lo + a.hi + b.hi;
}

operator == :: (a: Vec2, b: Vec2) -> bool {
    return a.x == b.x && a.y == b.y;
}

operator != :: (a: Vec2, b: Vec2) -> bool {
    return a.x != b.x || a.y != b.y;
}

operator < :: (a: Vec2, b: Vec2) -> bool {
    return (a.x + a.y) < (b.x + b.y);
}
```

Note: every overload here returns a **scalar**, deliberately. (Returning a struct from an
overload was blocked when this file was written — the [aggregate-return](/by-example/032-aggregate-returns/)
feature later unblocked it — and this file keeps its scalars so it keeps the coverage it had.)

## Resolution is an exact match on both operands

There is no conversion, no promotion, and no ranking. Resolution keys on *both* operand types, so
`Vec2 * s64` and `s64 * Vec2` are two separate declarations, and writing only one means only one
order works:

```jr
    if p * 10 == 30 {
        n = n + 2;
    }
    if 10 * p == 30 {
        n = n + 4;
    }
```

That is the cost of not becoming C++. Because `Pair + Pair` is a different declaration from
`Vec2 + Vec2`, a `Pair` reaches a different overload of the same operator — which is the whole
point of keying on the operand pair.

## Comparison operators too

`==`, `!=`, and `<` may be overloaded (returning `bool`), so the whole comparison family is
covered, not just equality:

```jr
    if p == same {
        n = n + 16;
    }
    if p != q {
        n = n + 32;
    }
    if p < q {
        n = n + 64;
    }
```

## A builtin meaning always wins

`s64 + s64` never consults the overload table. This falls out of the orphan rule — no overload
can exist for two builtin types — so ordinary integer arithmetic is untouched:

```jr
    a := 7;
    b := 5;
    if a + b == 12 {
        n = n + 128;
    }
```

If the builtin path *did* wrongly consult the table, the exact-match lookup would find nothing and
fall through, so the observable sign that it behaves is simply that the answer stays right.

## Composition and procedure boundaries

An overload lowers to an **ordinary direct call** — no new MIR node — so it composes with builtin
operators and can be inlined like any small procedure. It resolves inside any body, not only in
`main`:

```jr
    if (p + q) * 2 == 66 {        // overload result feeding a builtin operator
        n = n + 1024;
    }
    if total(p, q) == 33 {        // overload resolved inside another procedure
        n = n + 2048;
    }
```

## Observable result

```jr
    if n == 4095 {
        exit(0);
    }
    exit(1);
```

Every assertion folds into the bitmask `n`, and the exit status makes the whole computation
observable so the two engines can be asserted to agree byte-for-byte.

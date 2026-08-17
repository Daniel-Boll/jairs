---
title: Returning structs
description: A procedure returns a struct (or a view) by value, through the caller-allocated calling convention.
sidebar:
  order: 32
---

A procedure may return an aggregate — a struct, or a view — by value (ADR-0051). This was the
back-end change that removed the last place where the two engines disagreed about what
*compiles*: `jr run` had always managed it, since a VM value can hold bytes, but `jr build` used
to refuse, because returning a pointer into the callee's own frame would dangle. The
caller-allocated return convention resolves that.

```jr
#import "Basic";

Vec2 :: struct {
    x: s64;
    y: s64;
}

Big :: struct {
    a: s64;
    b: s64;
    c: s64;
    d: s64;
    e: s64;
    f: s64;
    g: s64;
    h: s64;
}

mk :: (a: s64, b: s64) -> Vec2 {
    r: Vec2;
    r.x = a;
    r.y = b;
    return r;
}
```

`mk` builds a local `Vec2`, fills its fields, and returns it. The returned aggregate is a *value*
loaded out of a slot, not an address — an earlier attempt to return `Rvalue::Address` was refused
by the verifier ("taking an address must produce a pointer"), which is that check earning its
keep.

## Two sizes, two code paths

`Vec2` is 16 bytes; `Big` is 64. They matter separately because they take different paths through
the native back end: a small struct's copy unrolls into loads and stores, while a large one calls
`memcpy`. Emitting that call surfaced a latent bug in *every* libcall — the namer produced
Cranelift's internal `Memcpy` rather than the C `memcpy` — found only because this was the first
libcall ever emitted.

```jr
big :: (n: s64) -> Big {
    r: Big;
    r.a = n;
    r.b = n + 1;
    // ...
    r.h = n + 7;
    return r;
}
```

Every assertion reads a field *back*, not just the struct's size — a convention that returned the
right number of bytes and the wrong values would pass any test that only checked the call
completed. The check on `Big`'s last field (`b.h == 107`) is what makes a copy that got the
*length* wrong visible rather than plausible.

## The overload that was unblocked

The natural first example of an operator overload is `Vec2 + Vec2 -> Vec2`, and this feature is
what finally let it compile:

```jr
operator + :: (p: Vec2, q: Vec2) -> Vec2 {
    r: Vec2;
    r.x = p.x + q.x;
    r.y = p.y + q.y;
    return r;
}
```

## Both halves of the convention

An aggregate *result* feeding an aggregate *parameter* makes the two halves of the convention
meet in one call. The parameter side already worked; the return side is what this added:

```jr
double :: (v: Vec2) -> Vec2 {
    return v + v;
}
```

A struct can also be returned through two call levels with no intermediate variable — `forward`
returns `mk(a, b)` directly — and a result whose fields come from arithmetic that *could* trap
(`keep`) is read afterwards, making observable the rule that a trap must not leave a variable
half-written.

## A returned view

A view `[]T` is a `{data, count}` aggregate, so it travels the same return path a struct does:

```jr
tail :: (buf: []s64) -> []s64 {
    return buf;
}
```

At the call site the returned view is read *through*, not just for its `.count`:

```jr
    arr: [4]s64;
    arr[0] = 7;
    arr[3] = 9;
    w := tail(arr[]);
    if w.count == 4 {
        n = n + 4096;
    }
    if w[0] == 7 {
        n = n + 8192;
    }
    if w[3] == 9 {
        n = n + 16384;
    }
```

Reading through the view catches a `data` word that copied correctly but pointed at the wrong
storage — a bug a `.count` check alone would miss.

## Observable result

```jr
    if n == 32767 {
        exit(0);
    }
    exit(1);
```

Because the VM needed no change for this feature at all, the harness comparing the two engines is
the *only* thing checking that the new convention agrees with the one that already worked — which
is exactly why the result is encoded in the exit status.

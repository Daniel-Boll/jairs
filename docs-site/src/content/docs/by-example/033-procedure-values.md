---
title: Procedures as values
description: A procedure can be stored in a value, passed as a parameter, and called through a pointer.
sidebar:
  order: 33
---

A procedure in Jairs is a value: it can be bound to a local, passed as a parameter, returned, and
called through the resulting pointer (ADR-0059). This is what an allocator needs — a procedure
pointer plus data — and until it existed neither engine could call through a pointer.

```jr
#import "Basic";

add :: (a: s64, b: s64) -> s64 {
    return a + b;
}

sub :: (a: s64, b: s64) -> s64 {
    return a - b;
}

apply :: (fn: (s64, s64) -> s64, a: s64, b: s64) -> s64 {
    return fn(a, b);
}

pick :: (want_add: bool) -> (s64, s64) -> s64 {
    if want_add {
        return add;
    }
    return sub;
}
```

`add` and `sub` encode their operands' positions (one adds, one subtracts) so that calling the
*wrong* one is a different answer rather than a coincidentally equal one.

## A procedure as a value

Naming a procedure without calling it yields its value, which a local can hold and then call:

```jr
    f := add;
    if f(20, 22) == 42 {
        n = n + 1;
    }
```

A proc-pointer local flows through a slot like any scalar, so it can be reassigned:

```jr
    h := add;
    h = sub;
    if h(43, 1) == 42 {
        n = n + 32;
    }
```

## The procedure-pointer type

The one genuinely new piece of surface is the procedure-pointer *type*, written `(T, T) -> T`.
`apply` takes one and calls it:

```jr
apply :: (fn: (s64, s64) -> s64, a: s64, b: s64) -> s64 {
    return fn(a, b);
}
```

That is the shape a higher-order procedure and an allocator both need. The parameter is one
machine word, so this exercises the ABI as well as the feature:

```jr
    if apply(f, 20, 22) == 42 {   // add through the parameter
    g := sub;
    if apply(g, 50, 8) == 42 {    // a DIFFERENT procedure — apply is not secretly calling add
    if apply(add, 21, 21) == 42 { // a procedure passed straight in, never bound to a local
```

## Identity is observable

`pick` returns one of two procedures depending on a condition. The point is that *which*
procedure a pointer names is observable, not just that it calls one — a representation that only
recorded "some procedure" would call the wrong target:

```jr
    chosen := pick(true);
    if chosen(40, 2) == 42 {
        n = n + 8;
    }
    other := pick(false);
    if other(44, 2) == 42 {
        n = n + 16;
    }
```

The two engines encode a procedure pointer differently by design — the VM encodes an internal
`ProcRef`, the native back end a real code address — but only *calling through* it is observable,
and the differential harness compares that. A wrong target or a lost identity would be a different
number.

## Observable result

```jr
    if n == 127 {
        exit(0);
    }
    exit(1);
```

The exit status encodes which of the seven calls returned 42, so `jr run` and `jr build` can be
asserted to agree. (Taking a *foreign* procedure as a value is refused in the type-errors corpus,
since a file that must check cleanly cannot hold one.)

---
title: $T polymorphic procedures
description: Procedures whose parameter type is a variable a call infers — declared as a template, instantiated per concrete type.
sidebar:
  order: 57
---

A `$T` parameter (ADR-0081) makes a procedure polymorphic: its parameter type is a *variable* that
a call infers from the argument. A `$T` procedure is a **template** — the compiler appends a
concrete clone per type it is called with, and by the time either engine runs it there is nothing
polymorphic left.

## Declaring a template

Declaration and use were built in two waves. The surface came first: a `$T` procedure parses,
formats, lowers as a template, and **type-checks clean when declared** — calling one was refused
until the instantiation wave.

```jr
#import "Basic";

/// The canonical example: one `$T`, inferred from `x`, returned as a bare `T`.
id :: (x: $T) -> T {
    return x;
}

/// One `$T` across two parameters and the return — a single variable, three uses.
first :: (a: $T, b: T) -> T {
    return a;
}

/// `$T` behind a pointer, so the collector recurses through `*`.
deref :: (p: *$T) -> T {
    return p.*;
}

/// `$T` as a view's element, so the collector recurses through `[]`.
count_view :: (items: []$T) -> s64 {
    return items.count;
}

/// Two parameters of the same `$T`, summed.
pair_sum :: (a: $T, b: T) -> T {
    return a + b;
}

main :: () {
    n := 5;
    exit(n);
}
```

The variable is written `$T` at its **binding** occurrence and bare `T` at every **use**. So
`first :: (a: $T, b: T) -> T` is *one* variable used three times, not three independent variables.
`$T` may appear nested inside a pointer (`*$T`) or a view (`[]$T`), and it may appear in the return
type. A `$T` procedure lowers to **no MIR** at all — it is skipped exactly as a `#foreign` body is
— because there is nothing concrete to lower until a call binds `T`.

## Instantiating a template

A call infers `$T` from its argument, and the compiler appends a **concrete procedure** — a clone
of the template with its variable bound — that both engines lower and run like any other. That is
what lets the differential harness check a polymorphic program at all: `id(42)` and `id(p)` are
ordinary procedures to it.

```jr
#import "Basic";

id :: (x: $T) -> T {
    return x;
}

/// Two parameters of one `$T`, summed — instantiated at `s64` here.
add :: (a: $T, b: T) -> T {
    return a + b;
}

/// Two parameters of one `$T`, returning the first. Instantiated at a struct.
first :: (a: $T, b: T) -> T {
    return a;
}

Point :: struct {
    x: s64;
    y: s64;
}

main :: () {
    n := 0;

    // `$T` = s64, inferred from the argument, returned.
    if id(7) == 7 { n = n + 1; }

    // Two calls at the same type — one appended instantiation serves both.
    if add(40, 2) == 42 { n = n + 2; }
    if add(1, 1) == 2 { n = n + 4; }

    // `$T` = Point, carried through `id` and read back: the aggregate survives instantiation.
    p: Point;
    p.x = 8;
    p.y = 8;
    q := id(p);
    if q.x + q.y == 16 { n = n + 8; }

    // `$T` = Point through `first`.
    a: Point;
    a.x = 16;
    a.y = 0;
    b: Point;
    b.x = 99;
    b.y = 99;
    r := first(a, b);
    if r.x == 16 { n = n + 16; }

    // `id` at s64 again — the same instantiation, from a different call site.
    if id(32) == 32 { n = n + 32; }

    // Every assertion: 63.
    if n == 63 {
        exit(0);
    }
    exit(1);
}
```

### De-duplication is structural

Two calls with the same bound type share **one** instantiation (ADR-0005). `add(40, 2)` and
`add(1, 1)` both bind `add` at `s64`, and the compiler appends a single procedure for both: the
key is the tuple of bound interned type ids, so identity is by *what the type is*, not by where
the call was written. `id(7)` and `id(32)` likewise share one `s64` instantiation reached from two
sites.

Binding `$T` to a **struct** (`id(p)`, `first(a, b)`) is the case where a body correct for a
scalar could be wrong for an aggregate, which is why per-instantiation checking is load-bearing —
each clone is checked against its concrete type.

## Several type variables

One procedure may carry several variables (ADR-0083): `pair :: (a: $A, b: $B)`. Each is inferred
from the first argument whose parameter is *directly* `$Var`, and the structural key is the
**tuple** of all bound types — so `pick(s64, bool)` and `pick(s64, s64)` are distinct
instantiations.

```jr
#import "Basic";

/// Two variables, returns the first.
pick_first :: (a: $A, b: $B) -> A {
    return a;
}

/// Two variables, returns the second.
pick_second :: (a: $A, b: $B) -> B {
    return b;
}

/// `$A` used twice and a distinct `$B`: one variable repeated, one independent.
combine :: (a: $A, b: A, tag: $B) -> A {
    return a + b;
}

id :: (x: $T) -> T {
    return x;
}

Point :: struct {
    x: s64;
}

main :: () {
    n := 0;

    p: Point;
    p.x = 1;
    r := pick_first(p, 99);       // A = Point, B = s64
    if r.x == 1 { n = n + 1; }

    q: Point;
    q.x = 2;
    s := pick_second(40, q);      // A = s64, B = Point
    if s.x == 2 { n = n + 2; }

    if combine(40, 2, true) == 42 { n = n + 4; }   // A = s64 (twice), B = bool
    if combine(10, 6, 0) == 16 { n = n + 8; }      // A = s64, B = s64 — a distinct tuple

    if id(16) == 16 { n = n + 16; }

    t: Point;
    t.x = 32;
    u := pick_first(t, 7);        // same tuple as the first call — reuses its instantiation
    if u.x == 32 { n = n + 32; }

    if n == 63 {
        exit(0);
    }
    exit(1);
}
```

`combine(40, 2, true)` and `combine(10, 6, 0)` differ only in `$B` (`bool` versus `s64`), yet they
are two distinct instantiations because the key is the whole tuple of bound types. `combine`'s
`$A` is used across two parameters — one variable repeated — while `$B` is independent.

## Inference through a pointer or view

The variable need not appear *directly* as a parameter's type (ADR-0084). A pointer or view
parameter — the common polymorphic shape, like a `sort` over `[]$T` or a `swap` over `*$T` —
infers by a one-layer structural match: `*$T` against `*U` peels both pointers and binds `T = U`;
`[]$T` against `[]U` peels both views.

```jr
#import "Basic";

/// Infers `T` through a pointer and reads the pointee.
deref :: (p: *$T) -> T {
    return p.*;
}

/// Infers `T` through a view.
sum_view :: (items: []$T) -> s64 {
    return items.count;
}

/// `*$T` in two parameters — one variable, two positions.
first_of :: (a: *$T, b: *$T) -> T {
    return a.*;
}

Point :: struct {
    x: s64;
    y: s64;
}

main :: () {
    n := 0;

    v := 42;
    if deref(*v) == 42 { n = n + 1; }        // *$T ↔ *s64

    buf: [3]s64;
    if sum_view(buf[]) == 3 { n = n + 2; }   // []$T ↔ []s64

    p: Point;
    p.x = 4;
    p.y = 4;
    q := deref(*p);                          // *$T peeling to a struct pointee
    if q.x + q.y == 8 { n = n + 4; }

    a := 16;
    b := 99;
    if first_of(*a, *b) == 16 { n = n + 8; } // *$T in two positions, one variable

    small: [2]u8;
    if sum_view(small[]) == 2 { n = n + 16; } // a distinct element type

    w := 32;
    if deref(*w) == 32 { n = n + 32; }        // reuses the s64 instantiation

    if n == 63 {
        exit(0);
    }
    exit(1);
}
```

This is **one-directional** — a structural match, not a full unifier: it peels one layer and binds
the variable, with no substitution back (ADR-0084 §3), which is all a single `$T` in a single
structural position needs. `first_of(*a, *b)` has `*$T` in two positions of one variable: the
second is a *use*, checked against the binding the first produced.

## Observing the result

Every one of these files uses the `n` accumulator — each passing assertion adds a distinct power
of two, summing to `63`. The `exit` value encodes which held, so `jr run` and `jr build` can be
asserted to agree byte-for-byte. That agreement is a genuine check here: the native engine
materialises a distinct function per instantiation (per distinct *tuple* of bound types), which
the VM does not, so agreement is not a shared shortcut.

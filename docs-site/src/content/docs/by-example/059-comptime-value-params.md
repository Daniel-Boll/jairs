---
title: $N comptime-value parameters
description: Procedures polymorphic over a compile-time value — the value-side mirror of $T, baked into each instantiation.
sidebar:
  order: 59
---

`$N` (ADR-0087) is a parameter polymorphic over a compile-time-known *value* — the value-side
mirror of `$T`. Where `$T` varies a type, `$N` varies a value; each call bakes its argument into a
concrete instantiation.

## Declaring a template

Like `$T`, declaration and use were split into two waves; the surface came first. A `$N` procedure
parses, formats, lowers as a template, and **its body type-checks when declared**. Calling one was
refused until the instantiation wave.

```jr
#import "Basic";

/// The canonical example: one `$N`, used as an ordinary `s64` in the body.
sized :: ($N: s64) -> s64 {
    return N + 1;
}

/// A `$N` beside an ordinary value parameter — a template may mix comptime and runtime parameters.
scaled :: ($N: s64, factor: s64) -> s64 {
    return N * factor;
}

/// Two comptime-value parameters in one signature.
area :: ($N: s64, $M: s64) -> s64 {
    return N * M;
}

main :: () {
    n := 7;
    exit(n);
}
```

The load-bearing difference from `$T` is that **more is checkable at template time**. A `$T`
parameter's *type* is unknown until a call, so its body cannot be checked; but a `$N: s64`
parameter's type is fully known — only its *value* varies. So `N` is a genuine `s64` in the body,
and `sized`'s `N + 1` and `scaled`'s `N * factor` are ordinary `s64` arithmetic, checked here,
where a `$T` body could not be. Like any template, a `$N` procedure lowers to **no MIR** until a
call fixes the value.

## Instantiating a template

A call evaluates each `$N` argument to a **constant** at compile time — via the same acyclic
pre-pass `#insert` uses, so const-eval and the checker are not made mutually recursive — and
appends a concrete procedure with that value baked in. The instantiation's parameter list has the
`$N` parameters **dropped** (they have no runtime existence), and each reference to `N` in the
body becomes a literal.

```jr
#import "Basic";

make :: ($N: s64) -> s64 {
    return N + 1;
}

scaled :: ($N: s64, factor: s64) -> s64 {
    return N * factor;
}

main :: () {
    n := 0;

    // `$N = 5`, baked. The instantiation returns 6.
    if make(5) == 6 { n = n + 1; }

    // A second call at the same value — one instantiation serves both.
    if make(5) == 6 { n = n + 2; }

    // A distinct value — a second instantiation.
    if make(7) == 8 { n = n + 4; }

    // Mixed comptime and runtime parameters. `$N` is baked to 3; `factor` is passed as 4.
    if scaled(3, 4) == 12 { n = n + 8; }

    // A comptime call whose result is used in a bigger expression.
    a := make(5);
    b := scaled(3, 4);
    if a + b == 18 { n = n + 16; }

    // Every assertion: 31.
    if n == 31 {
        exit(32);
    }
    exit(1);
}
```

De-duplication is **structural**, keyed on the tuple of interned value ids: `make(5)` and a second
`make(5)` share one instantiation, while `make(5)` and `make(7)` are two. In `scaled(3, 4)` the
comptime `$N` is baked to `3` and the runtime `factor` is passed as `4` at the call — a template
may mix the two.

Here the accumulator reaching `31` triggers `exit(32)`, so the differential's exit-code assertion
checks a distinct value. The native back end materialises a distinct function per instantiation,
which the VM does not, so `jr run` and `jr build` agreeing byte-for-byte is a real check.

## The case `$N` exists for: `[N]T`

The reason to parameterise over a value is an array whose length comes from that value
(ADR-0089). `buf: [N]s64` inside `make :: ($N: s64)` needs `N`'s value at the point the array type
resolves — which is precisely why it had to wait for instantiation.

```jr
#import "Basic";

/// Fills a `[N]s64` with 1..N and sums it, so the array's real size is load-bearing twice.
fill_and_sum :: ($N: s64) -> s64 {
    buf: [N]s64;
    i := 0;
    while i < N {
        buf[i] = i + 1;
        i = i + 1;
    }
    total := 0;
    j := 0;
    while j < buf.count {
        total = total + buf[j];
        j = j + 1;
    }
    return total;
}

main :: () {
    // N=4 → 1+2+3+4 = 10; N=3 → 1+2+3 = 6. Two distinct instantiations, two distinct array types.
    exit(fill_and_sum(4) + fill_and_sum(3));
}
```

Each instantiation bakes its own value, so `fill_and_sum(4)` gets a genuine `[4]s64` and
`fill_and_sum(3)` a genuine `[3]s64` — **two different types, two different layouts, from one
declaration**. Within the body `N` is used both as the array length and as an ordinary `s64` (the
`while i < N` bound), and `buf.count` reads the *real* length from the type. The value reaches the
type checker through the HIR, interned by the const-eval pre-pass, so the checker never runs
const-eval itself and depends on neither the database's evaluator nor the VM.

### The template body is typed against a placeholder

A template has no value for `N`, so its body is typed against a placeholder length `[0]s64` and
its length-dependent checks are **withheld** (ADR-0089 §2) — otherwise `buf[0]` there would be a
false "index out of range". The template is never lowered, and each instantiation is checked
against its *real* length, which is where a genuinely bad index is caught.

The program exits `16` (`10` from `N=4` plus `6` from `N=3`). A shared or wrong length would give
a different total, so this asserts the *value* and not merely engine agreement — and the native
back end lays out each instantiation's array independently of the VM, so the agreement is real.

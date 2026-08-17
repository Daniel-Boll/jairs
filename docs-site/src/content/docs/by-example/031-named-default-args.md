---
title: Named & default arguments
description: Arguments may be passed by parameter name and in any order, and parameters may declare literal defaults.
sidebar:
  order: 31
---

A call may pass its arguments by parameter name instead of by position, and a parameter may
declare a default value used when the caller omits it (ADR-0053). A named argument matches a
*parameter name*, so sema rewrites the argument list into positional order before anything
downstream sees it — the lowering never learns what a name was.

```jr
#import "Basic";

draw :: (x: s64, y: s64, colour: s64 = 7, scale: s64 = 1) -> s64 {
    return x * 1000 + y * 100 + colour * 10 + scale;
}

only_named :: (a: s64 = 5, b: s64) -> s64 {
    return a * 10 + b;
}

kinds :: (flag: bool = true, ratio: float64 = 0.5, small: u8 = 9) -> s64 {
    n := 0;
    if flag {
        n = n + 1;
    }
    if ratio == 0.5 {
        n = n + 2;
    }
    if small == 9 {
        n = n + 4;
    }
    return n;
}

add :: (a: s64, b: s64) -> s64 {
    return a + b;
}

edge :: (n: s8 = -128) -> s64 {
    if n == -128 {
        return 1;
    }
    return 0;
}
```

Every procedure here encodes its arguments' positions into one number — `x * 1000 + y * 100 + …`
— so passing an argument to the wrong parameter yields a *different* answer rather than a
plausible one. Testing with all-equal arguments would prove nothing.

## Defaults

A default is written `name: T = literal`. A call may omit a defaulted argument, and the declared
value is filled in at the call site:

```jr
if draw(1, 2) == 1271 {          // colour 7, scale 1 both defaulted
if draw(1, 2, 3) == 1231 {       // colour supplied, scale defaulted
if draw(1, 2, 3, 4) == 1234 {    // nothing defaulted
```

A default **must be a literal**. This is a layering constraint, not a preference: const-eval runs
downstream of signature resolution, so allowing a default to name a constant would make a
signature depend on a value whose own type depends on signatures.

A default **need not be trailing**. `only_named` puts its default first, which means the
procedure is callable only by name:

```jr
if only_named(b = 3) == 53 {       // a defaults to 5
if only_named(a = 1, b = 3) == 13 {
```

Requiring defaults to come last would be a simpler rule that forbids this signature for nothing.

Defaults may be of any literal kind — `bool`, `float64`, `u8` — and even negative, because the
range check is against the type's full range: `s8`'s minimum `-128` is representable, where a
magnitude-based check would wrongly refuse it.

## Named arguments

An argument may name its parameter, and named arguments may appear in any order:

```jr
if draw(1, 2, scale = 9) == 1279 {          // named, skipping a defaulted colour
if draw(x = 1, y = 2) == 1271 {             // all named, in declaration order
if draw(y = 2, x = 1) == 1271 {             // named OUT of order — reordering must happen
if draw(1, y = 2, colour = 3) == 1231 {     // positional first, then named
if draw(scale = 4, colour = 3, y = 2, x = 1) == 1234 {   // fully reversed
```

The out-of-order case is the one that proves reordering actually happens: a pass that ignored
names would compute `2 * 1000 + 1 * 100 + …` here rather than 1271. Positional arguments must
come before named ones.

## The untouched common path

`add(3, 4)` passes no names and omits no defaults, so it gets no entry in the filled-argument map
at all and lowers exactly as it did before this feature existed — proof the common path is
untouched rather than merely believed to be.

## Observable result

```jr
    if n == 16383 {
        exit(0);
    }
    exit(1);
```

`main` accumulates a bitmask of passing assertions and exits with it. Encoding the result in the
exit status makes it observable to the differential harness, so `jr run` and `jr build` can be
asserted to agree. (The refusals — an unknown parameter name, a duplicate, a missing required
argument — live in the invalid corpus, because a file that must check cleanly cannot hold one.)

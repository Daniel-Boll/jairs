---
title: Array literals
description: "`T.[a, b, c]` — a fixed array literal whose named element type answers every question `[1, 2, 3]` left open, and its two refusals."
sidebar:
  order: 38
---

`T.[a, b, c]` is a fixed array literal, naming the element type before the elements (ADR-0194). Reading three real Jai repositories for ADR-0185 counted the bare `[1, 2, 3]` spelling 39 times, the single most-used construct Jairs lacked — ADR-0039 §6 had deferred it over three open questions, and naming the element type answers all three at once: the length *is* the element count, the elements are ordinary expressions rather than required constants, and each is checked against the named type through the same expectation mechanism every other typed position already uses.

## The base case

```jr
Point :: struct {
    x: s64;
    y: s64;
}

takes_three :: (a: [3]s64) -> s64 {
    return a[0] + a[1] + a[2];
}

main :: () {
    total := 0;

    // The base case: an initialiser, whose type is `[3]s64` because three elements were written.
    a := s64.[10, 20, 30];
    total = total + a[0] + a[1] + a[2];
    total = total + a.count;
```

`s64.[10, 20, 30]` gives `a` the type `[3]s64` — the length comes from how many elements were written, not from a separate annotation.

## The narrow-width case

```jr
// Each element is checked against `u8`, so these are `u8` literals — under ADR-0016 §1 an
// untyped integer literal would otherwise land on `s64`, and `b` would be the wrong array
// entirely.
b := u8.[1, 2, 3, 4];
total = total + cast(s64, b[3]) + b.count;
```

This is what makes the named type more than decoration: without it, an untyped integer literal defaults to `s64` (ADR-0016 §1), and `b` would be a `[4]s64` masquerading as bytes. The named type is the expectation every element is checked against.

## Elements need not be constant

```jr
// Elements need not be constant. This is the half `[1, 2, 3]`'s deferral was unsure about, and
// a named type makes it a non-question: an element is an expression like any other.
n := 5;
c := s64.[n, n * 2, n * 3];
total = total + c[2];
```

An element is an ordinary expression, so a runtime value composes with the literal exactly as it would with any other typed position.

## As a call argument and a for sequence

```jr
// As a **call argument**, passed by value like any other array.
total = total + takes_three(s64.[1, 2, 3]);

// As a **`for` sequence**. This needed MIR to spill a sequence that is a value rather than a
// place — an array literal has no place, by the same rule that refuses `s64.[1, 2][0] = 5`.
for v: s64.[7, 8, 9] {
    total = total + v;
}
```

An array literal has no place — it can be read but not assigned into, which is why `s64.[1, 2][0] = 5` is refused the same way. Iterating it directly needed the mid-end to spill a value-only sequence rather than reach for an address that does not exist.

## What it cannot do

An empty literal is <span class="jairs-status refused">refused</span> with E0295 rather than accepted as a `[0]T`: `s64.[]` would be indexable to nowhere, `size_of` zero, and a `for` over it a guaranteed no-op — every operation on it is either an error or a no-op, so it is refused where it is written rather than left to surprise whoever measures it later.

An element that does not fit its named type is refused too, at the *element*, not at the array: `u8.[1, 2, 300]` fails on `300` specifically, because the named type is the expectation for every element, and the compiler reports the one that overflows rather than the construct as a whole.

Naming the length is a separate question from naming the elements, and it has its own refusal: an array's *declared* length (`[N]T`, not a literal) may name a literal-valued constant, but not one that needs evaluation. `LITERAL :: 4; ok: [LITERAL]u8;` is legal — the value is already a literal, so there is nothing to compute — while `SUM :: 2 + 2; a: [SUM]u8;` is <span class="jairs-status refused">refused</span> with E0233, because const-eval runs downstream of where a type annotation is resolved, and sema cannot invert that phase order to compute one (ADR-0039 §3a, ADR-0070 §2).

See also [Book I — The Jairs Language](/language/introduction/).

---
title: Arithmetic & assignment
description: Trapping arithmetic, the explicit wrapping operators, and the compound assignment forms of both.
sidebar:
  order: 12
---

Jairs takes a firm position on integer overflow: ordinary arithmetic **traps** rather than
wrapping silently. When you genuinely want modular arithmetic, you ask for it explicitly with a
`%`-suffixed operator.

## Trapping arithmetic

```jr
// All arithmetic in Jairs traps on overflow (ADR-0002). There is no
// silent wraparound and no undefined behaviour.
main :: () {
    a := 6;
    b := 7;

    sum := a + b;
    diff := a - b;
    prod := a * b;
    quot := a / b;
    rem := a % b;

    // Unary negation.
    neg := -a;

    // Precedence: `*` binds tighter than `+`.
    mixed := a + b * 2;

    // Parentheses override precedence.
    grouped := (a + b) * 2;
}
```

The five binary operators `+ - * / %` behave as expected, plus unary negation `-a`. The design
decision recorded in ADR-0002 is that all of these **trap on overflow** — there is no silent
wraparound and no undefined behaviour. Precedence is conventional: `*` binds tighter than `+`,
so `a + b * 2` multiplies first, and parentheses override that in `(a + b) * 2`.

## Explicit wrapping operators

```jr
// Because plain arithmetic traps, Jairs provides explicit wrapping
// operators. Hash functions, PRNGs and checksums need them; without these
// the standard library could not be written in Jairs at all.
mix :: (state: s64, input: s64) -> s64 {
    h := state;
    h = h +% input;
    h = h *% 6364136223846793005;
    h = h -% 1;
    return h;
}
```

Since plain arithmetic traps, Jairs supplies a parallel set of operators that wrap: `+%`, `-%`,
and `*%`. These are not an afterthought — hash functions, pseudo-random number generators, and
checksums all depend on modular arithmetic, and without the wrapping operators the standard
library could not be written in Jairs at all. The `mix` procedure above is exactly the kind of
hashing step that needs them.

## Compound assignment

```jr
main :: () {
    a := 0;

    a = 1;

    // Compound assignment. These trap on overflow exactly like their
    // binary forms.
    a += 2;
    a -= 3;
    a *= 4;
    a /= 5;
    a %= 6;

    // Wrapping compound forms.
    a +%= 7;
    a -%= 8;
    a *%= 9;
}
```

Both families have compound forms. The trapping compounds `+= -= *= /= %=` trap on overflow
exactly like the binary operators they abbreviate, and the wrapping compounds `+%=`, `-%=`, and
`*%=` wrap exactly like theirs. Plain `a = 1` is ordinary assignment.

See also [Book I — The Jairs Language](/language/introduction/).

---
title: Comparison & logic
description: The six comparison operators, the short-circuiting `&&` and `||`, and logical negation with `!`.
sidebar:
  order: 13
---

Comparisons produce booleans, and the logical operators combine them. `&&` and `||`
short-circuit, and `!` negates.

```jr
main :: () {
    a := 1;
    b := 2;

    lt := a < b;
    le := a <= b;
    gt := a > b;
    ge := a >= b;
    eq := a == b;
    ne := a != b;

    // `&&` and `||` short-circuit; `!` negates.
    both := lt && gt;
    either := lt || gt;
    negated := !lt;
}
```

The six comparison operators are the usual set: `<`, `<=`, `>`, `>=`, `==`, and `!=`. Each
yields a `bool`, which is why the results can be stored in variables (`lt`, `eq`, and so on)
and then combined.

The logical operators work on those booleans:

- `&&` (logical and) and `||` (logical or) **short-circuit** — the right operand is only
  evaluated if the left does not already decide the result.
- `!` negates a boolean, so `negated := !lt` is `true` exactly when `lt` is `false`.

See also [Book I — The Jairs Language](/language/introduction/).

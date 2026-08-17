---
title: Operators and overloading
description: Arithmetic, the non-C precedence rules, and how to overload an operator for your own type.
sidebar:
  order: 9
---

This chapter collects Jairs' operators in one place, calls out the handful of rules that
differ from C, and shows how to give an operator a meaning for your own type.

## Arithmetic

```
+  -  *  /  %      trapping
+% -% *%           wrapping (modular)
-x                 unary negation (traps on the most-negative value)
```

The trapping operators are the default because that is [the design
value](/language/introduction/#the-design-values): an overflowing `+` produces a result the
program did not ask for, so it stops. When you *want* modular arithmetic — hashing, PRNGs,
checksums — you reach for `+%`, `-%`, `*%`, which discard the overflowing bits on purpose:

```jr
h := h *% 31 +% byte;      // a hash: overflow is the mixing, not a bug
```

`/` and `%` on integers choose signed or unsigned operations from the operand type. Division
by zero traps.

## Comparison and logic

```
==  !=  <  <=  >  >=        comparison, producing a bool
&&  ||                       logical, short-circuiting
!                            logical negation
```

## Bitwise — and where precedence differs from C

```
&  |  ^  ~  <<  >>
```

Two precedence facts are **not** C's, and both make correct code read correctly:

- **Bitwise binds tighter than comparison.** `flags & MASK == 0` means `(flags & MASK) == 0`.
  C reads it as `flags & (MASK == 0)` — something Ritchie called a mistake kept only for
  backward compatibility, and which Go, Rust and Zig all changed. Under C's ordering Jairs
  would actually *refuse* the line, because `flags & bool` is a type error here.
- **Shifts sit between `+` and `*`.** `a + b << c` is `a + (b << c)`. C puts shifts below
  `+`.

Other bitwise rules:

- **`>>` is arithmetic for a signed type, logical for an unsigned one** — decided by the
  operand type, exactly as `/` picks signed vs unsigned division. There is no `>>>`; a program
  wanting the bits without the sign casts to the unsigned type first.
- **An out-of-range shift count traps.** `x << 8` on an `s8`, or a negative count, is a trap
  rather than a silently masked no-op. The shift's *result* is not checked — `1 << 7` in an
  `s8` is −128, which is exactly the bits requested.
- **Bitwise operators are integers only.** `1.5 & 2.5` is refused: a float's bits are a sign,
  exponent and mantissa, and ANDing them is meaningless. To inspect a float's bits you use a
  `union`, since `cast` converts values, not representations.

## Assignment

Plain `=` and the compound forms:

```
=  +=  -=  *=  /=  %=  +%=  -%=  *%=  &=  |=  ^=  <<=  >>=
```

A compound operator traps or wraps exactly like its binary form.

## Overloading an operator for your own type

You can give an operator a meaning for your own types with the `operator` declaration — which
is just the `name :: value` constant form with an operator where the name goes:

```jr
Vec2 :: struct { x: s64; y: s64; }

operator + :: (a: Vec2, b: Vec2) -> Vec2 {
    r: Vec2;
    r.x = a.x + b.x;
    r.y = a.y + b.y;
    return r;
}

operator == :: (a: Vec2, b: Vec2) -> bool {
    return a.x == b.x && a.y == b.y;
}
```

Now `p + q` and `p == q` work for `Vec2` values. Three rules govern resolution:

- **Exact match on both operands.** There is no conversion, promotion, or ranking. `Vec2 *
  s64` and `s64 * Vec2` are two *separate* declarations; writing only one means only that
  order works. This is the cost of not becoming C++.
- **A builtin meaning always wins.** `s64 + s64` never consults the overload table, so you
  cannot redefine arithmetic on the primitive types.
- **At least one operand must be local** to your file — the "orphan rule" that keeps two
  files from defining conflicting meanings for the same pair of imported types.

An overload lowers to an ordinary direct call, so it is inlinable like any small procedure,
and it composes: `(p + q) * 2` mixes an overload with a builtin operator freely. Operator
overloads also **cross module boundaries** — this is what lets `Math`'s `Vector3 + Vector3`
work in your program even though the overload lives in the `Math` module.

Still <span class="jairs-status absent">absent</span>: overloading unary operators, `[]`,
`()`, and the compound-assignment operators; and using an overload inside a `#run`.

Next: [The type system](/language/the-type-system/) — conversions, `cast`, `xx`, and how
Jairs decides two types are the same.

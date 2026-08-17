---
title: Values and types
description: The integers, floats, booleans, strings and pointers every Jairs program is built from.
sidebar:
  order: 2
---

Every Jairs program is built from a small set of primitive types. This chapter introduces
them and — just as importantly — the rules Jairs applies when values of different types meet.
The theme to carry forward is that **Jairs does not convert silently**. Where C would quietly
widen, promote, or reinterpret, Jairs asks you to say what you mean.

## Integers

Jairs has a full tower of fixed-width integers:

| Signed | Unsigned |
| --- | --- |
| `s8`, `s16`, `s32`, `s64` | `u8`, `u16`, `u32`, `u64` |

`s64` is the workhorse. Integer literals can be written in decimal, hex (`0x`), binary
(`0b`) or octal (`0o`), and `_` may be used as a digit separator:

```jr
a := 1_000_000;
b := 0xFF;
c := 0b1010;
d := 0o755;
```

A literal with no other context is an `s64`. Where the surrounding code fixes a type — an
annotation, a parameter, an assignment target — the literal takes that type instead:

```jr
g: u8 = 255;        // the literal 255 is a u8 here
```

If a literal does not fit the type the context asks for, that is a compile error (`E0204`),
not a wrap: `x: u8 = 300;` is refused. This is the one case where the width is checked at
compile time.

### Overflow traps

The defining rule of Jairs arithmetic: **integer overflow always traps.** An `s8` that
reaches 128 by addition does not wrap to −128 and does not invoke undefined behaviour — the
program stops, at a known source location, with a backtrace. This is true in debug and
release alike, and it is true in the compile-time VM as well as native code.

When you genuinely want modular arithmetic — hashes, checksums, pseudo-random generators —
you ask for it explicitly with the wrapping operators `+%`, `-%`, `*%`. See
[Operators and overloading](/language/operators-and-overloading/) for the full story.

## Floats

`float32` and `float64` are plain IEEE-754. Float literals are written `1.5`, `1e9`,
`1.5e-3`, `1_000.5`:

```jr
pi := 3.14159;
big := 1e9;
```

Floats behave differently from integers in a way that follows directly from the standard:
**floats do not trap.** `1.0 / 0.0` is `inf`, `0.0 / 0.0` is `NaN`, and an overflowing
multiply saturates to infinity. Integer overflow traps because an overflowing `+` produces a
result the program did not ask for; IEEE-754 *defines* infinity as the answer, so there is
nothing to refuse.

Two consequences surprise people:

- **`==` is not reflexive for floats**, because `NaN == NaN` is false. There is no `is_nan`
  intrinsic yet, so the idiomatic check is `x != x`.
- **A float literal that does not fit `float32` is not an error.** `x: float32 = 1e300;` is
  `inf` — there is no float overflow to refuse, unlike the integer case above.

## Booleans

`bool` is `true` or `false`. The logical operators `&&` and `||` short-circuit; `!` negates.
Comparisons (`==`, `!=`, `<`, `<=`, `>`, `>=`) produce a `bool`.

## Strings

A `string` is a `{data: *u8, count: s64}` pair — a pointer and a length. It is **not**
NUL-terminated. This is deliberate: it is exactly the shape the operating system's `write`
wants, so printing a string needs no conversion and no temporary buffer.

```jr
MESSAGE :: "hello from Jairs\n";
```

String literals support the escapes `\n \r \t \0 \\ \" \uXXXX`.

Two things about strings catch newcomers:

- **`a == b` on two strings is refused** (`E0278`). Because a string is a pointer and a
  count, `==` has two equally plausible meanings — same storage or same contents — and Jairs
  will not pick one for you. Comparing *contents* is `String.equal(a, b)` from the standard
  library; see [The standard library](/language/the-standard-library/).
- **`s.data[i]` does not compile.** A `*u8` is not indexable. Reading one byte is
  `String.byte_at(s, i)`, or by hand `(s.data + i).*` with a cast.

## Pointers

`*T` is a pointer to a `T`. Jairs uses **prefix `*` to take an address** and **postfix `.*`
to dereference** — the opposite convention from C, chosen so that a chain of dereferences
reads left to right:

```jr
sum := 41;
ptr := *sum;        // ptr : *s64, the address of sum
value := ptr.*;     // value : s64, read back through the pointer
```

Field access auto-dereferences: if `p` is a `*Point`, `p.x` reads the field without an
explicit `.*`. Pointer *arithmetic* (`p + n`, `p - n`, element-scaled) exists and is covered
in [Memory](/language/memory/).

`null` is the null pointer. A memory source (`malloc`) and the allocator protocol that
produces typed pointers are also in [Memory](/language/memory/).

## No implicit conversions

This is the rule that ties the chapter together. There is **no implicit conversion between
any two types** — not between integer widths, and emphatically not between integers and
floats:

```jr
// some_s64 + some_float64   // type error: no int/float mixing
// 1 + 1.5                   // type error for the same reason
```

To cross between numeric types you use `cast`, which is explicit and visible:

```jr
n := cast(s64, some_float64);   // saturating float -> int
b := cast(u8, n % 10 + 48);     // narrowing; takes the low bits
```

`cast` **truncates and does not trap** on a narrowing integer conversion — a narrowing cast
*is* the program asking for the low bits. A float-to-int cast **saturates**: `cast(s8,
1000.0)` is 127 and `cast(s8, nan)` is 0, so every float has an answer in every integer type.

The one apparent exception is not a conversion at all: an untyped *literal* takes its
context's type, so `1.5 + f32_value` works (the `1.5` is simply a `float32` here) while
`1 + f64_value` does not (the `1` is an integer literal, and integers do not mix with
floats).

The full rules — including `xx`, the "autocast" that infers the target type from context but
is no more powerful than `cast` — are in [The type system](/language/the-type-system/). The
next chapter, [Declarations and constants](/language/declarations-and-constants/), shows how
you actually bind these values to names.

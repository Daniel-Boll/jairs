---
title: The type system
description: cast, xx, type identity, and the rules Jairs applies when types meet.
sidebar:
  order: 10
---

Jairs is statically and **nominally** typed, with no implicit conversions. This chapter
gathers the conversion rules introduced piecemeal earlier and adds the two tools you use to
cross type boundaries deliberately: `cast` and `xx`.

## Nominal identity

Two aggregates are the same type only if they are the *same declaration*. Two structs with
identical fields are different types; an enum is not its underlying integer; a `Point` defined
in one module is not a `Point` defined in another. Identity is by declaration site, not by
shape.

This has a practical upshot for polymorphism and reflection: the compiler assigns every type a
stable **id** (its pool identity), and that id is what `type_info(T).id` reports and what
`Any` compares — see [Reflection](/language/reflection/).

## No implicit conversions — a recap

There is no automatic conversion between any two types. Not between integer widths, and not
between integers and floats:

```jr
// x: s32 = some_s64;      // refused — narrowing is not implicit
// y := some_s64 + 1.5;    // refused — no int/float mixing
```

The single apparent exception is an **untyped literal**, which takes its context's type. So
`1.5 + f32` works (the literal is a `float32` here) and `1 + f64` does not (an integer literal
does not become a float).

## cast

`cast(T, x)` converts `x` to type `T`, explicitly and visibly:

```jr
n := cast(s64, some_float64);    // float -> int, saturating
b := cast(u8, value % 10 + 48);  // narrowing int, takes the low bits
```

The rules, all of which you have seen:

- **Narrowing an integer truncates and does not trap** — a narrowing cast *is* a request for
  the low bits. (Narrowing a *literal* that does not fit is still a compile error, `E0204`.)
- **A float-to-int cast saturates.** `cast(s8, 1000.0)` is 127; `cast(s8, nan)` is 0. Every
  float has an answer in every integer type, so there is no trap to add.
- **`cast` converts values, not representations.** There is no `cast(*s64, some_ptr)` between
  unrelated pointer types, and no way to read a float's bits with `cast` — that would be a
  reinterpretation, which Jairs confines to `union` and to the `Any` boundary. This refusal is
  deliberate: a general pointer cast would make a wrong pointee type a silent wrong read.

## xx — autocast

`xx` is the "autocast": it casts to whatever type the *context* expects, so you do not repeat
the target type when it is obvious:

```jr
take_u8 :: (b: u8) { … }

main :: () {
    n := 65;
    take_u8(xx n);        // context says u8, so xx casts to u8
}
```

`xx` is **no more powerful than `cast`** — it performs exactly the conversions `cast` allows,
just with the target inferred rather than written. In particular `xx` will not do a pointer
reinterpretation either. Its most common companion is the bare enum member `.RED`, which is
the same "ask the context for the type" idea applied to enum members.

## Where reinterpretation is allowed

Two places, and only two, let you look at a value's bits as another type:

- A **`union`**, which overlays its fields — reading a different field than you wrote
  reinterprets the storage.
- The **`Any`** boundary, where `any_of`/`any_as` erase and recover a value's type with a
  checked read.

Everywhere else, changing a value's type changes its value (`cast`) or is refused. That
single rule — reinterpretation is confined and named — is why "a wrong pointee type is a
silent wrong read" is not a failure mode Jairs has.

## Typed allocation

There is one more corner where types and raw memory meet. An allocator returns `*u8` (raw
bytes), and you cannot `cast` that to a `*T`. Instead Jairs provides `typed(T, p)` to view a
`*u8` as a `*T` at a *named boundary*, and `untyped(p)` to go back:

```jr
d := typed(s64, malloc(n * size_of(s64)));   // *u8 -> *s64, visibly
// … use d as an ordinary *s64 …
free(untyped(d));                              // *s64 -> *u8
```

`typed` is not *safer* than a cast — `typed(s64, p)` on four bytes is still wrong — but it is
**visible**: the target type is a type argument you can search for, the same shape that makes
`Any`'s erasure happen only at a marked boundary. It takes a `*u8` specifically, so `*T ->
*U` cannot be reached by another spelling. This is what makes the heap-backed `List` and `Map`
modules expressible; see [Memory](/language/memory/) and [The standard
library](/language/the-standard-library/).

Next: [Memory](/language/memory/) — the context, allocators, temporary storage, and pointer
arithmetic.

---
title: Typed constants and type_of
description: "`name : T : value` makes a constant's annotation the expectation every use is checked against, and `type_of(x)` is the inverse of every other type argument."
sidebar:
  order: 39
---

Two small, unrelated-looking features that both close a gap the standard library hit first. A typed constant, `name : T : value`, gives a constant a declared type rather than one inferred from its initialiser (ADR-0190). `type_of(x)` takes a value and gives back its type — the inverse of `size_of(T)`, which takes a type and gives back a number (ADR-0192).

## A typed constant is the expectation, not a comment

```jr
// A `GLenum`-shaped constant: the case the whole feature exists for.
FLAG : u32 : 256;

// Every width, so a wrong one shows. `size_of` reads the *constant's* type, not the literal's.
TINY : u8 : 200;
SMALL : s16 : -300;
WIDE : u64 : 18446744073709551615;

// Not only integers: a float constant and a string constant carry their annotation too.
THICKNESS : float32 : 1.5;
DRIVER : string : "gl";
```

Before this, a Jairs constant took its type from its initialiser, and an untyped integer literal lands on `s64` (ADR-0016 §1) — so every constant crossing a C boundary needed a `cast` at each use. `modules/GL` alone carried twenty such casts for twenty-one constants that are all `GLenum`. The annotation is the expectation for the value, checked where the constant is *written*: `TOO_BIG : u8 : 300;` is <span class="jairs-status refused">refused</span> with E0204 (the literal does not fit) and `WRONG_KIND : s64 : "text";` with E0214 (the wrong kind entirely) — both at the declaration, not wherever the constant is later used.

## The width is real, not just accepted

```jr
takes_u32 :: (v: u32) -> s64 {
    return cast(s64, v);
}

takes_u8 :: (v: u8) -> s64 {
    return cast(s64, v);
}

main :: () {
    total := 0;

    // Each of these calls is the assertion: the parameter type is exact, so passing an `s64` would be
    // E0214 and this file would not build.
    total = total + takes_u32(FLAG);
    total = total + takes_u8(TINY);
```

`FLAG` is a real `u32` and `TINY` a real `u8` — calling `takes_u8(TINY)` type-checks specifically because `TINY`'s declared width matches the parameter, not because `200` happens to fit in whatever width was convenient. An annotated constant is still a constant everywhere a constant is required, too: `COUNT : s64 : 4;` still names an array length, `buf: [COUNT]u8;`, exactly as an untyped one would.

## type_of(x): the value's type, as a type

```jr
// A polymorphic procedure is where `type_of` earns its keep in real code: the parameter's type has no
// spelling at the call site, so nothing else can name it.
size_of_argument :: (v: $T) -> s64 {
    return size_of(type_of(v));
}
```

`type_of` is one arm of `described_type`, the function every type-taking intrinsic asks for its argument, so `size_of`, `type_info` and `any_as` all gained it at once. Inside a polymorphic procedure, `type_of(v)` names `$T` with no other spelling available at the call site — which is exactly where it earns its keep.

```jr
n: s32 = 5;

// **The identity, not just the size.** `type_of(n)` must be *the* `s32`, not something the same size
// — which is what makes it usable as a type rather than only as a measurement.
if type_info(type_of(n)).id == type_info(s32).id {
    total = total + 1000;
}
// And it must not collide with a different type of the same size: `s32` and `u32` are both four
// bytes, so an implementation keyed on width alone passes everything above and fails here.
if type_info(type_of(n)).id != type_info(u32).id {
    total = total + 4000;
}
```

`type_of(n)` yields the *same interned type* the name `s32` would have given — an implementation that returned a merely equivalent type would pass every size check and fail this identity comparison, since `s32` and `u32` are both four bytes but must not compare equal.

See also [Book I — The Jairs Language](/language/introduction/).

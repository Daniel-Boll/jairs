---
title: Polymorphism
description: Writing one procedure or struct that works for many types — $T, polymorphic structs, and $N.
sidebar:
  order: 16
---

Polymorphism in Jairs is **monomorphisation**: you write a procedure or struct with a type
*variable*, and the compiler produces a concrete copy for each set of types you actually use
it with. Nothing polymorphic survives to the back end — which is precisely what lets the
differential harness check a polymorphic program, since both engines run ordinary concrete
procedures.

## $T — polymorphic procedures

A parameter type written `$T` is a variable the call infers:

```jr
id :: (x: $T) -> T {           // one type variable, inferred from x
    return x;
}

main :: () {
    a := id(42);               // T = s64
    b := id(true);             // T = bool — a second instantiation
}
```

`id(42)` and `id(true)` become **two concrete procedures**, one per distinct tuple of bound
types, deduplicated across call sites. The body is **checked per instantiation**, so a
template that adds its arguments is fine for `s64` and a diagnostic — not a miscompile — for a
type with no `+`.

A single `$T` can appear across several positions (that is *one* variable, reused), and it can
be inferred **through** a pointer or a view:

```jr
first :: (a: $T, b: T) -> T { return a; }     // one T, three uses
deref :: (p: *$T) -> T { return p.*; }         // T inferred through *
count :: (items: []$T) -> s64 { return items.count; }   // through []
```

Several independent variables work too: `pair :: (a: $A, b: $B)`. A template may even **call
another template** — expansion iterates to a fixed point so a clone body's own polymorphic
calls are resolved.

Two-way unification, explicit type arguments, and — importantly — a **cross-file**
instantiation are <span class="jairs-status absent">absent</span>. Calling a `$T` procedure
declared in *another* module is refused; the workaround is a concrete wrapper the module
instantiates itself, which is exactly why the standard library exposes `sort_ints` alongside
the generic `sort`.

## Polymorphic structs

A struct can take a type argument, making it a **type constructor**:

```jr
Box :: struct($T) {
    value: T;
}

main :: () {
    bi: Box(s64);      // a distinct type whose `value` is s64
    bi.value = 7;

    bb: Box(bool);     // a DIFFERENT type whose `value` is bool
    bb.value = true;
}
```

`Box` alone is not a type; `Box(s64)` applies it to `s64`. `Box(s64)` and `Box(bool)` are
**distinct types** with distinct field types and layouts, told apart in the compiler the way
`[2]s64` and `[3]s64` are. Instances nest: `Box(Box(s64))` works.

A parameterised struct now **crosses a module boundary** — an importer resolves its fields
from the declaring module's definition, and the type's identity stays the declaring module's,
so a `Box(s64)` means the same type in two importers. What is still
<span class="jairs-status absent">absent</span> is inferring a struct's argument through a
`$T` parameter (`(b: Box($T))`), `using` on a parameterised struct, and a directly recursive
`List($T)`.

## $N — comptime-value parameters

The value-side mirror of `$T`: a parameter written `$N` is polymorphic over a **compile-time
value**, and the value is *baked into* each instantiation's body:

```jr
make :: ($N: s64) -> s64 {
    return N * 10;         // N is a literal in each instantiation
}

main :: () {
    a := make(5);          // an instantiation with N = 5 baked in
    b := make(7);          // a distinct instantiation with N = 7
    c := make(5);          // dedupes with the first
}
```

The argument is evaluated to a compile-time constant (a non-constant argument is refused), the
`$N` parameters drop out of the instantiation's parameter list, and each mention of `N` becomes
a literal. You can mix comptime and runtime parameters — `scaled :: ($N: s64, factor: s64)`
passes only `factor` at the call.

The payoff this feature exists for is **`[N]T` sized by a `$N` parameter**:

```jr
buffer :: ($N: s64) {
    buf: [N]s64;           // each instantiation gets its own array type
    // …
}
```

Two calls at 4 and 3 produce a `[4]s64` and a `[3]s64` from one declaration — genuinely
different array types, with different layouts, from a single source.

## The shape of it all

Everything here reduces to concrete code before it runs. `$T` picks types by structural
matching; `$N` bakes values; a polymorphic struct keys its layout on the argument. The
compiler multiplies the source out and both engines execute the ordinary result — which is why
polymorphism, for all its surface, adds nothing the back end has to understand.

Next: [Metaprogramming](/language/metaprogramming/), where a program generates code for itself.

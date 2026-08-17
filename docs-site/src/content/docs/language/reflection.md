---
title: Reflection
description: Asking the compiler about types at compile time — type_info and Any.
sidebar:
  order: 15
---

Reflection is the ability of a program to ask about its own types. Jairs has two pieces:
`type_info(T)`, which describes a type, and `Any`, which carries a value together with its
type so a routine can accept "a value of some type" and check what it got.

## type_info

`type_info(T)` returns a `Type_Info` describing the type `T`:

```jr
info := type_info(Point);
k := info.kind;        // STRUCT
name := info.name;     // "Point"
sz := info.size;       // its size in bytes
```

The fields are:

| Field | Meaning |
| --- | --- |
| `id` | the type's stable, canonical identity (its pool id) |
| `kind` | which shape it is — `INTEGER`, `FLOAT`, `STRUCT`, `POINTER`, `ARRAY`, `ENUM`, … |
| `name` | its source name (or a builtin's spelling, `"s64"`) |
| `size` | runtime size in bytes |
| `alignment` | runtime alignment in bytes |
| `count` | a struct's field count, or an array's length; 0 otherwise |
| `element` | an array's element or a pointer's pointee, as a type id; 0 otherwise |

The numbers come from the *same* layout computation every real layout decision uses, so
reflection cannot disagree with the layout it describes.

### Type_Info is declared in Jairs

`Type_Info` is not a magic compiler type — it is a `struct` declared in `modules/Basic`, in
Jairs. It has to be, because a program that reflects must be able to *write* `info:
Type_Info`, and no compiler-internal type is spellable. The compiler validates the struct's
fields on lookup, so editing it produces a clear diagnostic rather than a silent wrong read.

What is still <span class="jairs-status absent">absent</span> is the **variable-length**
detail — a struct's full field list, a procedure's signature — because those need a decision
about who owns the memory the list lives in. The fixed-size facts (`count`, `element`) are
present; following an `element` id back to a `Type_Info` is not.

## Any

An `Any` is a value paired with a pointer to its `Type_Info`:

```jr
Any :: struct {
    type: *Type_Info;
    data: *u8;
}
```

You build one with `any_of` and read it back with `any_as`, which **traps** unless the type
matches:

```jr
takes :: (a: Any) {
    // recover it as an s64 — traps if `a` doesn't actually hold an s64
    n := any_as(a, s64);
    use(n);
}

main :: () {
    x := 42;
    takes(any_of(*x));      // erase x's type into an Any
}
```

The checked read is the whole point. `any_as` compares the `id` from [The type
system](/language/the-type-system/#nominal-identity) — a stable identity that two calls to
`type_info(T)` share and two distinct types never do — so it is a sound check, not a name
comparison (a local `Point` and an imported one share a spelling but not an identity).

### Why the erasure is safe

Erasing `*Point` to the `*u8` inside an `Any` loses no bits — a pointer's layout doesn't
depend on what it points at — so the conversion emits no code; it is a statement in the type
system only. That is exactly why this erasure is *allowed* here while a general `cast(*u8, p)`
is refused: nothing is being reinterpreted. A wrong read is impossible because `any_as` checks
the type before handing the value back.

Currently `any_of(*x)` takes a **pointer** to the value. A bare value coercing to `Any`
implicitly needs a materialised temporary (a literal has no address), and an `Any` inside a
compile-time constant, are both <span class="jairs-status absent">absent</span>.

Next: [Polymorphism](/language/polymorphism/), where types become parameters.

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
| `kind` | which shape it is — `INTEGER`, `FLOAT`, `STRUCT`, `POINTER`, `ARRAY`, `ENUM`, `DYNAMIC_ARRAY`, … |
| `name` | its source name (or a builtin's spelling, `"s64"`); a *structural* type's name is a full recursive spelling — `[3]s64`, `[]s64`, `*Point`, `**Point` |
| `size` | runtime size in bytes |
| `alignment` | runtime alignment in bytes |
| `count` | a struct's field count, or an array's length; 0 otherwise |
| `element` | an array's element or a pointer's pointee, as a type id; 0 otherwise |
| `element_size` | the size of one element of a view or array, in bytes |
| `fields` | a struct's fields — `[]Type_Info_Field`, each a name, a type id, and a byte offset |
| `signed` | whether an integer kind is signed |
| `members` | an enum's members — `[]Type_Info_Member`, each a name and a value |

The numbers come from the *same* layout computation every real layout decision uses, so
reflection cannot disagree with the layout it describes.

### Type_Info is declared in Jairs

`Type_Info` is not a magic compiler type — it is a `struct` declared in `modules/Basic`, in
Jairs. It has to be, because a program that reflects must be able to *write* `info:
Type_Info`, and no compiler-internal type is spellable. The compiler validates the struct's
fields on lookup, so editing it produces a clear diagnostic rather than a silent wrong read.

The variable-length detail is here now — `fields` for a struct, `members` for an enum — because
the compiler already owns a static-data table it can hand out as a `[]T` view, so the memory
question that used to hold this back is answered by that table's own lifetime rather than a
per-call allocation. What is still <span class="jairs-status absent">absent</span> is going the
other way: given an `element` id, there is no function that hands you back the `Type_Info` it
names — you have that array's or pointer's `Type_Info` already, or you don't have it at all.

### type_of, and a pointer as a type argument

`type_of(x)` gives you a type from a **value** — an expression — rather than from a name, which
matters when the name has no spelling of its own, such as a polymorphic parameter's type inside
its own body:

```jr
x := 42;
size_of(type_of(x));        // 8 — the size of x's type, s64
type_info(type_of(x));      // the Type_Info for s64
```

And `type_info` itself accepts a **pointer** type as its argument, not only a bare name:
`type_info(*Point)` describes the pointer, whose `element` is `Point`'s id.

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

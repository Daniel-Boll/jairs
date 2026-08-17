---
title: Polymorphic structs
description: A struct parameterised over a type — a type constructor whose applications are distinct types with distinct layouts.
sidebar:
  order: 58
---

`Box :: struct($T) { value: T; }` (ADR-0085) is a struct parameterised over a type — the *type*
side of polymorphism, after `$T` procedures did the *value* side. `Box` alone is not a type; it is
a **type constructor**, and `Box(s64)` applies it to a type argument.

## Applying a type constructor

```jr
#import "Basic";

/// The canonical parameterised struct: one field of the type variable.
Box :: struct($T) {
    value: T;
}

/// Two fields of one type variable, to exercise layout of a substituted aggregate.
Pair :: struct($T) {
    first: T;
    second: T;
}

main :: () {
    n := 0;

    // `Box(s64)` — the field is `s64`.
    bi: Box(s64);
    bi.value = 7;
    if bi.value == 7 {
        n = n + 1;
    }

    // `Box(bool)` — the *same* declaration, a distinct type with a `bool` field.
    bb: Box(bool);
    bb.value = true;
    if bb.value {
        n = n + 2;
    }

    // `Pair(s64)` — both fields substitute.
    p: Pair(s64);
    p.first = 4;
    p.second = 8;
    if p.first + p.second == 12 {
        n = n + 4;
    }

    // `Box(Box(s64))` — an instance as another instance's type argument.
    bb2: Box(Box(s64));
    bb2.value.value = 8;
    if bb2.value.value == 8 {
        n = n + 8;
    }

    // Every assertion: 15.
    if n == 15 {
        exit(0);
    }
    exit(1);
}
```

Because a type is a compile-time value (ADR-0071), the `s64` in `Box(s64)` is that value being
passed as the argument. `Box(s64)` and `Box(bool)` share one declaration but are **distinct
types** with distinct field types and distinct layouts — told apart by the type argument recorded
in the pool key, the same way `[2]s64` and `[3]s64` are. Their fields come from *substituting* the
argument into the declaration's field types, so `Box(s64).value` is `s64` and `Box(bool).value` is
`bool`.

`Box(bool)` next to `Box(s64)` is the assertion that earns its keep: a compiler that keyed fields
on the declaration alone would give one of them the other's field type. `Pair(s64)` substitutes
*both* fields, exercising the layout of a substituted aggregate. And `Box(Box(s64))` uses one
instance as another's argument, so resolving the argument is itself an instantiation — nested one
level.

## The same declaration used across files

A `struct($T)` declared in a module was, at first, unusable by any importer: the field lookup
searched the *importing* file's own declarations, so `Box(s64)` from another module was refused
(ADR-0117). This is the reason the standard library's `Array`, `List` and `Map` were each a
concrete `Int_*` type before this landed — three library waves were blocked on it.

```jr
#import "Basic";
#import "Generic_Types";

/// Mutates an imported parameterised struct through a pointer.
fill :: (b: *Box(s64), v: s64) {
    b.value = v;
}

main :: () {
    n := 0;

    b: Box(s64);
    b.value = 7;
    if b.value == 7 {
        n = n + 1;
    }

    // A second instantiation of the same imported declaration, with a different field type.
    c: Box(bool);
    c.value = true;
    if c.value {
        n = n + 2;
    }

    // Two arguments.
    p: Pair(s64, bool);
    p.first = 5;
    p.second = false;
    if p.first == 5 && !p.second {
        n = n + 4;
    }

    // Through a pointer, into a procedure: a real type at the ABI level.
    fill(*b, 11);
    if b.value == 11 {
        n = n + 8;
    }

    exit(n);
}
```

### Why it was not a one-line lookup change

A parameterised struct's fields are resolved *per instance, under the caller's type arguments* —
and the declaring file cannot do that: it does not know what an importer will supply, and it
records its body with the variables bound to an error type because nothing concrete exists there
yet. So the **importer** resolves the fields, which means the field type-reference tree has to
cross the module boundary. Since a type reference is an index into the *declaring* file's arena,
the check phase needed that file's imported HIR — which the database already held. Passing it
through is what closed the gap.

**Identity is the declaring file's**: a nominal type's identity is its declaration site, so
`Box(s64)` is the *same* type in two importers rather than one type each — which is what lets a
value of it pass between them. `Pair(s64, bool)` shows that a parameterised struct genuinely takes
a *list* of arguments, not just a first one, and `fill(*b, 11)` shows an imported instance is a
real type at the ABI level — passed by pointer into a procedure and mutated, not merely a
checkable annotation.

## Observing the result

Both files use the `n` accumulator: the local example sums to `15`, the cross-file one likewise to
`15`, each passing assertion adding a distinct bit. The native back end computes each instance's
layout from its substituted fields independently of the VM, so the byte-for-byte agreement between
`jr run` and `jr build` on the exit code is a real check, not a shared shortcut.

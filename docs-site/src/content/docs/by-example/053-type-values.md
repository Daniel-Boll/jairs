---
title: Types as values
description: A type is a compile-time value, so a type can be bound to another name and used everywhere the original is.
sidebar:
  order: 53
---

A type is a compile-time value (ADR-0071). `Point :: struct { … }` has always been a *constant
whose value is a type*; what follows from saying so is the one thing that was missing — binding
that value to another name.

```jr
#import "Basic";

/// The aliased struct. Two fields rather than one, so a transposed alias would put a value in
/// the wrong place instead of merely working.
Point :: struct {
    x: s64;
    y: s64;
}

/// The aliased enum, with three members so a bare `.GREEN` has somewhere to be wrong.
Colour :: enum {
    RED;
    GREEN;
    BLUE;
}

/// The alias itself. This declaration is the whole feature.
Pair :: Point;

/// And for the enum, so the member-access path is covered as well as the layout one.
Shade :: Colour;

/// The alias as a field type, one level of nesting deep.
Holder :: struct {
    p: Pair;
}

/// The alias as a parameter type.
takes :: (q: Pair) -> s64 {
    return q.x + q.y;
}

main :: () {
    n := 0;

    // A local declared through the alias, with both fields written.
    p: Pair;
    p.x = 3;
    p.y = 4;
    if takes(p) == 7 {
        n = n + 1;
    }

    // An enum alias's member *is* the aliased enum's member — the same interned constant.
    c := Shade.GREEN;
    if c == Colour.GREEN {
        n = n + 2;
    }

    // Through a field whose type is the alias.
    h: Holder;
    h.p.x = 9;
    if h.p.x == 9 {
        n = n + 4;
    }

    // As an array element type, where a wrong stride would write into the neighbouring element.
    arr: [2]Pair;
    arr[1].y = 5;
    if arr[1].y == 5 {
        n = n + 8;
    }

    // Through a pointer, so `*Pair` and `*Point` are one type.
    ptr := *p;
    if ptr.x == 3 {
        n = n + 16;
    }

    // Every assertion: 31.
    if n == 31 {
        exit(0);
    }
    exit(1);
}
```

## `Pair :: Point` is a type alias

Because a type is just a value, `Pair :: Point;` binds `Pair` to the same value `Point` names.
`Pair` is then usable everywhere `Point` is: as a local's type, a parameter type (`takes`), a
field type (`Holder.p`), an array element type (`[2]Pair`), and a pointee (`*Pair`). Each of the
assertions exercises one of those positions, so a transposed or wrongly-strided alias would show
up as a wrong value rather than silently working — which is why `Point` has two fields rather than
one.

The compiler builds the alias by reading the type's value from where it already lived: the
signature phase records the resolved type of every nominal declaration, and const-eval runs
downstream of that phase, so the alias is interned from a value that already exists rather than by
re-deriving it.

## An alias does not create a new type

The subtle case is the enum. `Shade :: Colour;` must make `Shade.GREEN` the *same* interned
constant as `Colour.GREEN`, not a second one — because a struct or enum's identity is its
declaration site (ADR-0015 §1). The assertion `c == Colour.GREEN`, where `c` was obtained through
`Shade.GREEN`, is what proves the alias did not mint a second nominal identity. If it had, that
comparison would be a type error across two distinct types.

## What is refused, and what is absent

- A **chain** of aliases (`B :: Pair;`) stays refused: one level is a lookup, but a chain needs a
  fixpoint and a cycle check, deferred for the same reason an array-length chain is.
- `Type` is **not spellable** as an annotation — you cannot write `t: Type;`. That is a deliberate
  decision, documented in the type-error corpus, not an omission.

As always, the `n` accumulator makes the outcome observable: it sums to `31` when all five
assertions hold, so `jr run` and `jr build` can be asserted to agree on the exit code.

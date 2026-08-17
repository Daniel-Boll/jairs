---
title: Structs
description: Declaring structs, nesting them, the empty struct, and chained field access as a place.
sidebar:
  order: 22
---

A `struct` is a named aggregate of typed fields. This page uses two corpus files: one that declares a few struct shapes, and one that reaches into nested fields to read and write them.

## Declaring structs

```jr
Point :: struct {
    x: s64;
    y: s64;
}

// An empty struct is legal and occupies zero bytes.
Marker :: struct {
}

Entity :: struct {
    position: Point;
    health: s64;
    alive: bool;
}
```

A struct is declared with `Name :: struct { ... }`, each field written as `name: Type;`. Three things to note:

- **The empty struct is legal** and occupies zero bytes. It is useful as a marker type.
- **Structs nest by value.** `Entity` embeds a `Point` as its `position` field — the `Point`'s bytes live inline inside the `Entity`, not behind a pointer.
- **Fields can be any type**, including other structs (`position`), integers (`health`), and `bool` (`alive`).

## Field access is a place

```jr
Point :: struct {
    x: s64;
    y: s64;
}

Line :: struct {
    from: Point;
    to: Point;
}

main :: () {
    line: Line;
    line.from.x = 1;
    line.from.y = 2;
    line.to.x = 3;
    line.to.y = 4;

    dx := line.to.x - line.from.x;
}
```

Declaring `line: Line` with no initialiser gives a zeroed aggregate. Field access chains with `.`, and because a field access denotes a **place** (a location), it works on the left of an assignment as well as on the right: `line.from.x = 1` writes through two levels of nesting to the innermost field. On the right, `line.to.x - line.from.x` reads those same places back to compute `dx`. The same `line.to.x` expression is a place when assigned to and a value when read — one syntax, both roles.

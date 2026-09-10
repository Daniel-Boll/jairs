---
title: Structs
description: Declaring, constructing and nesting structs, plus chained field access as a place.
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

## Struct literals

```jr
point := Point.{y = 9, x = 4};   // typed, named, order-independent
other := Point.{4, 9};           // typed, positional, declaration order
origin: Point = .{};             // inferred from the annotation, all zero
partial := Point.{y = 9};        // omitted x is zero

entity: Entity = .{
    alive = true,
    position = .{x = 4, y = 9},  // nested literal inherits Point
    health = 100,
};
```

`T.{...}` names the type. `.{...}` takes a concrete type from an annotation, return type,
assignment target, parameter, or containing field. Entries are all named or all positional;
mixing them is an error. Named entries may be reordered, but their expressions still run once in
source order. Non-empty multiline literals receive a final comma by default; projects can set
`[fmt] struct_literal_trailing_comma = false`.

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

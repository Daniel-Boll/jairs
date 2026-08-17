---
title: using
description: Promote a struct's or parameter's fields into the enclosing scope, so p.x can be written as x.
sidebar:
  order: 35
---

`using` promotes the fields of a struct — as a struct member, a parameter, or a local — into the
enclosing name scope, so `p.x` can be written plainly as `x` (ADR-0050). This is the first
genuinely hard resolution problem in the language: every other name resolves to *one* thing,
while a promoted name resolves to a *path* — `x` meaning `p.x`.

```jr
#import "Basic";

Point :: struct {
    x: s64;
    y: s64;
}

Entity :: struct {
    using base: Point;
    hp: s64;
}

Actor :: struct {
    using body: Entity;
    speed: s64;
}
```

## Field promotion on a parameter

A `using` parameter makes the struct's fields nameable without the `p.` prefix:

```jr
len2 :: (using p: Point) -> s64 {
    return x * x + y * y;
}
```

A promoted name is a **place**, so it can be assigned through — otherwise every `using` parameter
would be silently read-only. Since a parameter is a copy, the write is observable only in the
return value:

```jr
bump :: (using p: Point) -> s64 {
    x = x + 10;
    return x;
}
```

Promotion also works through a pointer, auto-dereferenced exactly as `p.x` already is, so `*Point`
and `Point` agree about what a field access means:

```jr
len2_ptr :: (using p: *Point) -> s64 {
    return x + y;
}
```

## A real local shadows a promoted field

If a real local has the same name as a promoted field, the local wins — silently:

```jr
shadowed :: (using p: Point) -> s64 {
    x := 99;
    return x;   // returns 99, never p.x
}
```

The rejected alternative — promotion winning — would mean that adding a field to `Point` silently
changes what a local name means in every procedure that `using`s it. That is action at a distance,
and this procedure proves the language does not do it.

## Overlapping `using`s

Two `using`s may name the same field as long as it is only ever used **qualified**:

```jr
qualified_only :: (using a: Point, using b: Point) -> s64 {
    return a.x + b.y;
}
```

Overlapping providers are harmless until the ambiguous name is actually referenced — a rule that
refused the declaration outright would reject this procedure for nothing.

## Embedding, and transitivity

A struct that embeds another with `using` promotes its fields. The base stays a real field at a
real offset, so it is still nameable the long way:

```jr
    e: Entity;
    e.x = 5;         // reached through the embedded base, and written through it
    e.y = 6;
    e.hp = 9;
    // e.base.x is the SAME storage, reached qualified
```

Promotion is **transitive** through two levels of embedding — `Actor` embeds `Entity`, which
embeds `Point`:

```jr
    a: Actor;
    a.x = 7;         // reaches a.body.base.x
    a.hp = 8;
    a.speed = 11;
```

A one-level implementation passes every single-level test and fails here, which is why the two
levels are exercised.

## A `using` local

A `using` local promotes its fields from its declaration onward, and only within its block — the
same order-sensitivity an ordinary local has:

```jr
    using u: Point;
    x = 20;
    y = 22;
    if x == 20 {
        n = n + 16384;
    }
    if u.y == 22 {   // same storage, qualified
        n = n + 32768;
    }
```

## Observable result

```jr
    if n == 65535 {
        exit(0);
    }
    exit(1);
```

Adding `Res::Promoted` as a resolution outcome forced every exhaustive match over resolutions to
be updated — which is how the consumers that had to learn about promotion were *found* rather than
remembered. As with the rest of the corpus, the exit status encodes which assertions passed so the
two engines can be checked to agree.

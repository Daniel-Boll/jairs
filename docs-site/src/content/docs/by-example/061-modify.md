---
title: "#modify predicates"
description: A compile-time predicate that runs when a call binds a template's type variables, and may reject that instantiation.
sidebar:
  order: 61
---

`#modify { … }` is a compile-time predicate over an *instantiation* (ADR-0093). It runs when a call
binds a polymorphic procedure's type variables, and returning `false` **rejects that call**. So a
template can say "only for an `s64`" or "only for a struct with at least two fields" in code rather than
in a comment. The predicate is written *beside* the thing it guards, which is why it is an attribute on
the procedure rather than a separate declaration.

```jr
#import "Basic";

/// A predicate over the bound type's identity: this template is meant for `s64` only.
is_s64 :: (x: $T) -> T #modify {
    return type_info(T).id == type_info(s64).id;
} {
    return x;
}

/// A predicate reading a reflected field count, so a guard can be structural rather than nominal.
needs_fields :: (x: $T) -> s64 #modify {
    return type_info(T).count > 1;
} {
    return type_info(T).count;
}

/// `#modify` beside another attribute — the loop takes them in any order.
guarded :: (x: $T) -> T #no_abc #modify {
    return true;
} {
    return x;
}

main :: () {
    n := 5;
    exit(n);
}
```

## Reading the shape

Each declaration has **two** brace blocks: the `#modify { … }` predicate first, then the procedure
body. The predicate answers a `bool` about the bound type variable `T`, using the reflection intrinsic
`type_info(T)`:

- `is_s64` compares identities — `type_info(T).id == type_info(s64).id` — the shape a stable type `id`
  makes sound. This template accepts only `s64`.
- `needs_fields` reads a **reflected field count** (`type_info(T).count > 1`), so a guard can be
  *structural* rather than nominal: it accepts any type with more than one field.
- `guarded` shows `#modify` beside `#no_abc`; the attribute loop takes them in any order.

## Why a predicate needed reflection first

A predicate must be able to ask something about the bound type, and `type_info(T)` *inside* a `$T` body
was refused until a prior decision made it reachable — a `$T` procedure could not reflect on its own
parameter at all. That gap was found by designing this feature, which is why the reflection-in-templates
work landed before `#modify` despite being nobody's plan.

## The refusal that shipped first

As with `#expand`, the surface landed *with* a refusal: before it, the predicate was **parsed and
silently ignored**, so a `#modify` that should reject a call accepted it. That is worse than a rejection,
because nothing tells the author their guard did not run. So a call to a `#modify` procedure was refused
until the evaluation step landed; once the predicate began running, that refusal was retired. An
instantiation the predicate rejects now produces a diagnostic naming the call, not a silent acceptance.

The program above declares these templates but does not *call* them, so `main` does ordinary work and
exits 5 — the point is that declaring one type-checks cleanly and its block survives formatting.

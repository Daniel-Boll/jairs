---
title: Structs, unions and variants
description: The three ways Jairs groups fields — and what each one costs.
sidebar:
  order: 6
---

Jairs has three aggregate forms. They look similar and differ in exactly one dimension:
whether the fields share storage, and whether the language tracks which one is valid.

## struct

A `struct` lays its fields out one after another, each at its own offset. This is the
everyday aggregate.

```jr
Point :: struct {
    x: s64;
    y: s64;
}

main :: () {
    p: Point;          // all fields zeroed
    p.x = 4;
    p.y = 9;
    d := p.x + p.y;
}
```

Structs are **nominal** — two structs with identical fields are still different types — and
currently **one level** (no nested struct *definitions*, though a field may of course have a
struct type). Access is `p.x`, and it auto-dereferences through pointers: if `q` is a
`*Point`, `q.x` works without writing `q.*.x`.

A struct declared without an initialiser is zeroed field by field. This matters: it means a
freshly declared `Point` reads back `{0, 0}` in both engines, rather than stack garbage.

## union

A `union` puts **every field at offset 0** — they share storage. Writing one field and
reading another **reinterprets** the bits. There is no tag and no checking: it is your
responsibility to know which field is valid.

```jr
U :: union {
    i: s64;
    bits: s64;
}

main :: () {
    u: U;
    u.i = 5;
    same := u.bits;    // reads the same 8 bytes back — no trap
}
```

A union is smaller than the equivalent struct (one field's worth of storage, not the sum) and
that is the whole point of it. The classic use is reading a value's representation — for
example `union { f: float64; bits: u64; }` to inspect a float's bits, which is the only way
to do that since `cast` converts values, not representations.

## variant

A `variant` is a **tagged** union: it stores which case was last written, and reading a
*different* case **traps** instead of reinterpreting.

```jr
V :: variant {
    i: s64;
    f: s64;
}

main :: () {
    a: V;
    a.i = 7;           // sets the tag to `i`
    x := a.i;          // reads back 7
    // y := a.f;        // would TRAP — `f` is not the live case

    a.f = 9;           // moves the tag to `f`
}
```

You ask which case is live with `switch`, which compares the tag:

```jr
which :: (v: V) -> s64 {
    switch v {
        case .i;   return 1;
        case .f;   return 2;
    }
    return 0;
}
```

Like an enum `switch`, this is exhaustive over the cases — omitting one is a compile error.

## Choosing between them

| Form | Storage | Cross-field read |
| --- | --- | --- |
| `struct` | fields side by side | each field independent |
| `union` | all fields overlaid | reinterprets, silently |
| `variant` | overlaid + a tag | traps unless it's the live case |

The difference between `union` and `variant` is a deliberate cost you choose: a `variant` is
bigger (it carries the tag) and safe; a `union` is smaller and hands you the bits with no
questions asked. Jairs offers both rather than making one the default, so that the safety and
the size are each an explicit decision.

Some capabilities are still <span class="jairs-status absent">absent</span>: a recursive
variant, a variant in a `#foreign` signature, and eliding the tag check inside an arm that
has already matched.

Next: [Enums and flags](/language/enums-and-flags/), the two ways to name a set of discrete
values.

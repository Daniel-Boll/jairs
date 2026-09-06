---
title: Variadic parameters and print
description: "`..T` and `..Any` variadic parameters, the packing sugar at a call site, and print's % placeholders built on top of both."
sidebar:
  order: 18
---

A variadic parameter lets a procedure take a number of trailing arguments it does not fix in advance. ADR-0138 shipped the declaration shape, `args: ..T`; ADR-0139 added the packing sugar that turns `sum(1, 2, 3)` into a view built at the call site; ADR-0141 widened the element type to `Any`, which is what a formatting routine needs — and `print` (ADR-0189) is built on exactly that.

## The declaration shape: ..T

```jr
sum_view :: (xs: []s64) -> s64 {
    total := 0;
    for x: xs {
        total = total + x;
    }
    return total;
}

// The variadic parameter's type is `[]s64` in HIR, wrapped from the written `..s64` at
// lowering. Inside the body, `args` is an ordinary view.
sum :: (args: ..s64) -> s64 {
    return sum_view(args);
}
```

`args: ..s64` is a parameter with element type `s64`. Inside the body, `args` is nothing special — it is a `[]s64`, so it composes with anything a view already composes with, `sum_view` here. A caller may also pass an explicit view: `sum(buf[])` works because the parameter's real type is a view (ADR-0138).

## Packing sugar at the call site

```jr
// Pure variadic — no fixed parameters.
sum :: (args: ..s64) -> s64 {
    total := 0;
    for x: args {
        total = total + x;
    }
    return total;
}

// Fixed prefix + variadic — the mixed shape `printf(fmt, ..)` uses.
max_of :: (init: s64, args: ..s64) -> s64 {
    best := init;
    for x: args {
        if x > best {
            best = x;
        }
    }
    return best;
}
```

`sum(1, 2, 3)` packs its three trailing arguments into a fresh stack `[3]s64` and passes a view over it (ADR-0139) — `sum()` packs an empty view, and two calls at different call sites get their own storage, so neither observes the other's array. A fixed prefix and a variadic parameter combine, `max_of(init, ..args)`, which is the `printf(fmt, ..)` shape. Sema resolves the ambiguity between the pack and an explicit pass-through at the exactly-one-trailing case: one trailing argument of the view type is the pass-through (`sum(buf[])`), the element type is the pack (`sum(buf[0], buf[1])` or `sum(1, 2, 3)`).

## Mixed types with ..Any

```jr
sum_any :: (args: ..Any) -> s64 {
    total := 0;
    i := 0;
    while i < args.count {
        if args[i].type.id == type_info(s64).id {
            total = total + any_as(args[i], s64);
        }
        if args[i].type.id == type_info(Point).id {
            p := any_as(args[i], Point);
            total = total + p.x + p.y;
        }
        if args[i].type.id == type_info(bool).id {
            if any_as(args[i], bool) {
                total = total + 1;
            }
        }
        i = i + 1;
    }
    return total;
}
```

An `s64`, a `Point` and a `bool` all pack into one call, `sum_any(e, pt, flag)`, because ADR-0139's per-argument check *is* the value-to-`Any` coercion when the element type is `Any` — it fires at every argument position, describing and materialising each one. Inside the body, each element is discriminated at run time by `args[i].type.id` against `type_info(T).id`, and read back with the matching `any_as`. This is one call, mixed argument types, each recovered by its own type — the shape a formatting routine needs (ADR-0141).

## print and % placeholders

```jr
print :: (fmt: string, args: ..Any) -> s64 {
    out_reset(STDOUT);
    format_into_out(fmt, args);
    return out_flush();
}
```

`print` is an ordinary `..Any` variadic, so `print("% % % %\n", true, false, "text", 1.5)` packs its four arguments exactly as `sum_any` above did, and a private `format_any` discriminates each by its `Type_Info` before deciding how to render it (ADR-0189). Neither too few arguments nor too many is an error: too few renders the missing placeholder as `%!(MISSING)`, and too many appends the leftovers as `%!(EXTRA a, b)` — a `print` that refuses to print is worse than one that shows what it was given.

## What a bare value cannot do

`Any` coercion happens at a call boundary — an ordinary argument or a variadic pack — because that is where the language can materialise a fresh slot and take its address for `Any.data` to point at. It does not happen at a plain declaration: `x: Any = 42;` is <span class="jairs-status refused">refused</span> with E0214, *"mismatched types: expected `Any`, found an integer literal"*, because there is no call there to build a slot around. The same bare literal passed as a call argument, `sum_any(42)`, works — the difference is not the *value*, it is whether the expression sits where the coercion is built to fire.

See also [Book I — The Jairs Language](/language/introduction/).

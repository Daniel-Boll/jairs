---
title: Enums
description: Nominal enums, auto-numbering with the continue-from-here rule, and the bare-member and xx conveniences.
sidebar:
  order: 23
---

An `enum` is a nominal type whose members are namespaced under the type name (ADR-0041). This page covers the numbering rules and then the two context-driven conveniences — `xx` and bare `.MEMBER` — that let the surrounding code supply what the source omits.

## Numbering, and the continue-from-here rule

```jr
#import "Basic";

/// Auto-numbered: 0, 1, 2.
Colour :: enum {
    RED;
    GREEN;
    BLUE;
}

/// Explicit values, and the continue-from-here rule.
Status :: enum {
    OK :: 200;
    // `NEXT` is 405, one past `MISSING` -- not 2, which is its index.
    MISSING :: 404;
    NEXT;
    // Two names for one value.
    ALSO_OK :: 200;
}

main :: () {
    n := 0;

    if Colour.RED == Colour.RED {
        n = n + 1;
    }
    if Colour.RED != Colour.GREEN {
        n = n + 2;
    }

    // `cast(s64, c)` is how the number is obtained.
    if cast(s64, Colour.RED) == 0 {
        n = n + 4;
    }
    if cast(s64, Status.NEXT) == 405 {
        n = n + 64;
    }
    // ...
    if n == 2047 {
        exit(0);
    }
    exit(1);
}

is_warm :: (c: Colour) -> bool {
    return c == Colour.RED;
}
```

The numbering rules are Jai's:

- Members are **auto-numbered from 0** in declaration order (`RED`, `GREEN`, `BLUE` are 0, 1, 2).
- An **explicit value** is allowed: `OK :: 200`.
- Later members **continue from the previous value**, not from their position. `NEXT` follows `MISSING :: 404`, so `NEXT` is `405` — *not* `2`, which would be its index. This is the rule that is easy to get wrong by resetting to the member's index.
- Two names may share one value (`ALSO_OK :: 200`), which C and Jai both allow.

Because an enum is **nominal**, `==` and `!=` work between two values of the same enum type, but ordering and arithmetic deliberately do not (those attempts live in `type-errors/`). To get the underlying number you must ask explicitly with `cast(s64, ...)` — the enum does not silently become an `s64`. And `is_warm` shows an enum crossing a procedure boundary, which is what makes it a real type rather than a constant folded at its use site.

## Context supplies the type: xx and bare .MEMBER

```jr
#import "Basic";

Colour :: enum {
    RED;
    GREEN;
    BLUE;
}

Perm :: enum_flags {
    READ;
    WRITE;
}

main :: () {
    n := 0;

    // `xx`: the target comes from the context (an annotation here).
    big := 300;
    small: u8 = xx big;

    // An enum to its backing integer.
    value: s64 = xx Colour.BLUE;

    // bare `.MEMBER`: the enum comes from the context.
    c: Colour = .GREEN;
    if c == .GREEN {
        n = n + 256;
    }
    if is_red(.RED) {
        n = n + 1024;
    }

    // A flags enum, and a *combination* of bare members.
    both: Perm = .READ | .WRITE;

    // Assignment to an existing variable: the target's type is the context.
    c = .BLUE;
    // ...
}

is_red :: (c: Colour) -> bool {
    return c == .RED;
}
```

Two spellings, one idea: both `xx` and bare `.MEMBER` let the *context* supply what the source leaves out (ADR-0046).

- **`xx`** is sugar for a `cast` whose target type was written elsewhere in the statement — an annotation (`small: u8 = xx big`), a parameter type at a call site, or the other side of a comparison. It is never a looser conversion: `xx big` where `big` is `300` truncates to `44` in a `u8`, exactly as `cast(u8, 300)` would. Its precedence is that of a prefix `-`, so `xx tiny + 1` is `(xx tiny) + 1`.
- **Bare `.GREEN`** names an enum member without repeating the type; the enum comes from the context. Written for an annotation (`c: Colour = .GREEN`), on the other side of a comparison (`c == .GREEN`), as a call argument (`is_red(.RED)`), or on assignment (`c = .BLUE`).

Neither invents a fallback when there is no context. An `xx` with nothing to convert *to*, or a `.RED` with no enum to resolve *in*, is an error (E0242, E0244) rather than a guess — a defaulting `xx` would convert to a type nobody wrote, and a searching `.RED` would silently resolve differently the day an unrelated enum grew a member of the same name. Both diagnostics point at the explicit form. And the two compose: one call site can take an `xx` argument and a bare member in different arguments, proving the context reaches arguments once rather than twice.

---
title: Typed allocation & FFI floats
description: "size_of / typed / untyped give heap storage a type without a general pointer cast, and a #foreign procedure may take and return a float."
sidebar:
  order: 77
---

These are the two language features the standard library named as blockers and got. **Typed allocation**
(`size_of`, `typed`, `untyped`) is what lets `List` and `Map` hold heap storage of a real type; **floats
across the FFI boundary** is what lets `Math` wrap libm's transcendentals. Both were built by probing a
refusal and finding the narrowest thing that lifts it.

## Typed allocation

`malloc` returns `*u8`, and `cast(*s64, p)` is refused — a general pointer cast makes a wrong pointee
type a *silent wrong read*. That refusal is right and stays. What was missing was a way to get a
**typed** pointer to fresh memory that is *not* a general cast:

```jr
size_of(T)        // the runtime size of a type, in bytes — folded, so usable inside arithmetic
typed(T, p)       // reinterpret a *u8 as a *T at a named, greppable boundary
untyped(p)        // the reverse: a *T back to a *u8, so free (which takes *u8) accepts it
```

```jr
#import "Basic";

Point :: struct {
    x: s64;
    y: s64;
}

main :: () {
    n := 0;

    // `size_of` on a scalar and on a struct: eight bytes and sixteen.
    if size_of(s64) == 8 && size_of(u8) == 1 {
        n = n + 1;
    }
    if size_of(Point) == 16 {
        n = n + 2;
    }

    // A heap block of three s64s — `size_of` folded *inside* the arithmetic, which is every allocation's use.
    raw := malloc(3 * size_of(s64));
    if raw == null {
        exit(200);
    }
    d := typed(s64, raw);

    // Written through pointer arithmetic, which is the dynamic array's storage working.
    (d + 0).* = 10;
    (d + 1).* = 20;
    (d + 2).* = 30;
    if (d + 0).* == 10 && (d + 2).* == 30 {
        n = n + 4;
    }
    total := (d + 0).* + (d + 1).* + (d + 2).*;
    if total == 60 {
        n = n + 8;
    }

    // `untyped` gives the block back to `free`, which takes a *u8.
    free(untyped(d));
    n = n + 16;

    // A struct through a typed pointer, so retyping is not scalar-only.
    raw2 := malloc(size_of(Point));
    if raw2 == null {
        exit(201);
    }
    p := typed(Point, raw2);
    p.x = 3;
    p.y = 4;
    if p.x + p.y == 7 {
        n = n + 32;
    }
    free(untyped(p));

    exit(n);
}
```

**`typed` is not safer than a cast — it is *visible*.** `typed(s64, p)` on a four-byte allocation is
still wrong. What it adds is that the target type is a type *argument* at a named boundary a reader can
grep for, the same way an erasing conversion is permitted only at an `Any` boundary. Relaxing `cast` for
`*u8` → `*T` would permit the same wrong read with none of that, so `typed` requires a **`*u8`
specifically** — allowing `*T` → `*U` would be the general cast reached by another spelling. `untyped`
exists because a facility that can allocate and not free leaks by construction: `free` takes a `*u8`, so
releasing needs the reverse conversion, and it too is an intrinsic rather than a `cast` relaxation, so
both directions are searchable and neither widens `cast`.

The library allocates and only the *retyping* is an intrinsic: `Basic.malloc` keeps doing what it does,
and the language contributes exactly the one thing a library cannot express. `size_of` folds from the
same layout reflection that `type_info(T).size` reports, so the two cannot disagree — and it arrives with
a caller, since `n * size_of(T)` is the use every allocation has. The exit code is **63**.

## Floats across the FFI boundary

A `#foreign` procedure may take and return a float. This was refused until a specific ABI fact was
handled: a float is passed in a **floating-point register** (`xmm0` on x86-64, `d0` on arm64), not an
integer one, so passing its bits as a `u64` — how integers and pointers had crossed — would call the
callee on a float register that was never written, a plausible-looking wrong number, silently.

```jr
#import "Basic";

// The library is declared here, not taken from Basic, because the sema corpus harness type-checks a file
// without loading Basic — a `libc` from there would be an unknown library under that harness.
libc :: #system_library "c";

sqrt :: (x: float64) -> float64 #foreign libc "sqrt";
sqrtf :: (x: float32) -> float32 #foreign libc "sqrtf";
pow :: (base: float64, exponent: float64) -> float64 #foreign libc "pow";

main :: () {
    n := 0;

    r := sqrt(16.0);
    if r == 4.0 {
        n = n + 1;
    }

    p := pow(2.0, 10.0);   // two float arguments — both float registers used
    if p == 1024.0 {
        n = n + 2;
    }

    f := sqrtf(9.0);       // a float32 in and out — the narrowing path
    if f == 3.0 {
        n = n + 4;
    }

    // A returned float is a usable value: feed it back in. sqrt(sqrt(16.0)) = sqrt(4.0) = 2.0.
    nested := sqrt(sqrt(16.0));
    if nested == 2.0 {
        n = n + 8;
    }

    exit(n);
}
```

The comptime VM's libffi path now describes a float argument and return as `f32`/`f64` (so libffi places
it in the correct register), and native code gives the procedure's signature an `F32`/`F64` parameter (so
the SysV ABI places it in the float register). A **`float32` narrows at the boundary** — libffi's `float`
is 32-bit and a Jairs `float32` lives in the low 32 bits of its word, so the argument is decoded with the
*parameter's* width, which is why `sqrtf` is exercised alongside `sqrt`.

The comparisons are exact `==` rather than tolerant because libm is correctly rounded — `sqrt(16.0)` is
exactly `4.0` — and both engines call the *same* libm, so they agree. A returned float is a usable value,
not just a comparable one, which is why `sqrt(sqrt(16.0))` feeds one result back into the next argument.
The exit code is **15**.

This example ships the *capability*, with the `#foreign` declarations local to the file — the honest
scope. Adding `sqrt` and friends to `Basic`, and lifting `Math`'s transcendentals to libm wraps, are
separate additive changes this unblocks (and which the `Math` page shows landed).

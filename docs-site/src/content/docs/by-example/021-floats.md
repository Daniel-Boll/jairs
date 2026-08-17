---
title: Floating point
description: float32 and float64, IEEE-754's two surprises, saturating casts, and float constants.
sidebar:
  order: 21
---

Jairs has `float32` and `float64` (ADR-0040). Floating point is where the two engines could most easily disagree in silence — the VM evaluates in software while the native back end emits hardware instructions — so the corpus pins down exactly the cases IEEE-754 makes non-obvious.

## Literals, arithmetic, and the two surprises

```jr
#import "Basic";

main :: () {
    // Every literal form.
    plain := 1.5;
    exponent := 1e9;
    signed_exponent := 1.5e-3;
    separated := 1_000.5;

    // Context typing: the literal takes `float32` from the annotation.
    narrow: float32 = 1.5;

    // No fit check: `1e300` in a `float32` is `inf`, because IEEE-754 saturates.
    too_big: float32 = 1e300;

    // Arithmetic, and division by zero producing values rather than traps.
    sum := plain + plain;
    infinity := 1.0 / 0.0;
    neg_infinity := -1.0 / 0.0;
    nan := 0.0 / 0.0;

    // Negation is total: `-0.0` is a real value.
    negative_zero := -0.0;

    n := 0;

    // This one must NOT fire: `NaN == NaN` is false despite identical bits.
    if nan == nan {
        n = n + 1;
    }
    // This one must: `NaN != NaN` is true.
    if nan != nan {
        n = n + 2;
    }
    // And this one: `0.0 == -0.0` is true despite *different* bits.
    if negative_zero == 0.0 {
        n = n + 4;
    }
    // ...
}
```

**Literals** come in every form the lexer produces: plain (`1.5`), exponent (`1e9`), signed exponent (`1.5e-3`), and digit-separated (`1_000.5`). Note that `1e9` has no fractional part — a form the tree-sitter grammar once rejected while the compiler accepted it, a divergence that stayed invisible until this file existed to exercise it.

**Context typing** (ADR-0040 §5): `narrow: float32 = 1.5` gives the literal its type from the annotation rather than defaulting to `float64` and then mismatching.

**No fit check.** Unlike an integer literal, `1e300` assigned into a `float32` becomes `inf` — IEEE-754 saturates, and ADR-0040 §1 makes that a value rather than a failure. In the same spirit, `1.0 / 0.0` yields infinity, `-1.0 / 0.0` negative infinity, and `0.0 / 0.0` a NaN; none of them trap. Negation is total, so `-0.0` is a real value (where negating the most-negative *integer* would trap).

The three comparisons above are the ones a naive bit-compare gets wrong, in opposite directions:

- `NaN == NaN` is **false** despite identical bits, so the first `if` must *not* fire.
- `NaN != NaN` is **true**, because `!=` is the negation of `==` rather than its own ordered predicate.
- `0.0 == -0.0` is **true** despite *different* bits.

A float that reached a raw bit-compare fallback would answer all three the wrong way — a plausible wrong answer rather than an error, which is exactly why they are pinned.

## Casts saturate and truncate toward zero

```jr
    widened := cast(float64, narrow);
    narrowed := cast(float32, plain);
    from_integer := cast(float64, 7);
    truncated := cast(s64, -1.9);
    saturated := cast(s8, 1000.0);
    nan_to_int := cast(s64, nan);
```

All four cast directions are covered. Float-to-integer conversion **truncates toward zero** (so `-1.9` becomes `-1`, not `-2`) and **saturates** rather than wrapping (so `1000.0` into an `s8` is `127`, where a wrap would give `-24`). A NaN converts to `0`. Note also that there is *no* implicit conversion between the two float widths (ADR-0040 §6): comparing a `float32` against a `float64` needs an explicit widening `cast` — stricter than C, and the same strictness the integer widths carry.

The final `if n == 262142` gathers every assertion but the first (which must stay false). `exit` takes the low byte, so this program is checked by exit status `254` — still distinct from every prefix of the assertions failing, which is what lets the differential harness confirm the two engines agree.

## Float constants

```jr
#import "Basic";

HALF :: 0.5;
QUARTER :: 0.25;
NEGATIVE :: -1.5;
WHOLE :: 2.0;

narrow_local :: () -> float32 {
    n: float32 = 0.75;
    return n;
}

main :: () {
    n := 0;

    if HALF == 0.5 {
        n = n + 1;
    }
    if HALF * 4.0 == 2.0 {
        n = n + 2;
    }
    if NEGATIVE * 2.0 == -3.0 {
        n = n + 8;
    }
    // A float constant whose value happens to be integral.
    if WHOLE == 2.0 {
        n = n + 16;
    }
    if narrow_local() > 0.5 {
        n = n + 64;
    }

    if n == 127 {
        exit(0);
    }
    exit(1);
}
```

A `::` constant holding a float once crashed the native back end (ADR-0056). The reason is worth knowing: the code that copies a compile-time result out of the VM mapped *every* scalar to an integer — but a float *is* a scalar in the VM, its bits interpreted by its type. So a float constant interned as an integer value, and the native back end emitted an integer-load instruction into a float register. The VM read it back correctly (it too takes interpretation from the type), which is exactly why nothing caught it: the two engines disagreed about what even *compiles*, a class of gap the differential harness structurally cannot see, since a program that does not build produces no output.

The file probes the constant several ways — bare comparison, arithmetic (so a wrong bit pattern is a wrong answer), a negative constant, and one whose value happens to be integral (`WHOLE :: 2.0`, the case most likely to *look* right while carrying integer bits).

Note the honesty marker in the corpus: a `float32` *constant* has no way to spell its width. `NARROW :: 0.75` infers `float64`, and `NARROW : float32 = 0.75` is a variable, not a constant. So the narrow interning path is exercised through a **local** in `narrow_local` instead, and a typed `float32` constant is recorded as owed.

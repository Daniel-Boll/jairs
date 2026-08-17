---
title: The numeric tower & cast
description: Every integer width Jairs has, explicit casts between them, and negative literals that reach a signed type's minimum.
sidebar:
  order: 20
---

Jairs has a full tower of sized integer types — signed `s8`–`s64` and unsigned `u8`–`u64` — and conversions between them are always explicit through `cast`. This page draws on three corpus programs: the tower's boundaries, `cast` in every direction, and the folding of a leading minus into a literal.

## Every width, at its boundary

```jr
main :: () {
    a: s8 = -128;
    b: s16 = -32768;
    c: s32 = -2147483648;
    d: s64 = -9223372036854775808;

    e: u8 = 255;
    f: u16 = 65535;
    g: u32 = 4294967295;
    h: u64 = 18446744073709551615;
}
```

Each declaration sits at its type's **boundary** — the minimum for a signed type, the maximum for an unsigned one. That is deliberate: a width that silently resolved to the wrong type would fail the literal-fit check here rather than passing quietly. The numeric tower (ADR-0037) was a *naming* change more than a representational one — the underlying pool already stored a width and signedness, and both back ends already read it that way.

The signed minimums are the interesting half. They were unwritable at first: `-128` used to lower as a negation applied to the *magnitude* `128`, which was then tested against `s8`'s maximum of `127` and rejected — the diagnostic even printed "the range of `s8` is -128 to 127" while refusing the very minimum it named. ADR-0038 fixed this by folding the sign into the literal (see below).

## cast in every direction

```jr
#import "Basic";

main :: () {
    // Narrowing a runtime value truncates: 300 & 0xFF is 44.
    big: s64 = 300;
    small := cast(u8, big);

    // Widening is exact, and sign extension follows the *source* type: a
    // negative `s8` widens to a negative `s64`, not to 255.
    neg: s8 = -1;
    widened := cast(s64, neg);

    // A same-width cast changes signedness only.
    same := cast(u64, big);

    if small == 44 {
        if widened == -1 {
            if same == 300 {
                exit(0);
            }
        }
    }
    exit(1);
}
```

`cast(T, x)` covers widening, narrowing, and a change of signedness (ADR-0037 §2):

- **Narrowing a runtime value truncates.** `300` cast to a `u8` keeps only the low byte, `44`. (Narrowing a *literal* is instead a compile error, and that case lives in the `type-errors/` corpus, not here.)
- **Widening is exact, and sign extension follows the source type.** A `-1` held in an `s8` widens to a negative `s64`, not to `255` — the source's signedness decides how the extra bits are filled.
- **A same-width cast changes only signedness.** The back end refuses reduce/extend instructions on equal widths, so this is the pass-through case.

The trailing `exit` makes the result observable. Only two corpus programs print anything, so a computation has to be exposed through the process exit status — otherwise the differential harness would be comparing silence against silence when it checks that `jr run` and `jr build` agree byte-for-byte. Here `exit(0)` fires only when all three checks pass.

## Negative literals

```jr
#import "Basic";

main :: () {
    // The minimum of a two's-complement type is not the negation of anything the
    // type can hold, which is the whole reason the fold exists.
    small: s8 = -128;
    widened := cast(s64, small);

    // An ordinary negative literal, and a negation of a *value* — which still
    // lowers to `Unary(Neg, ..)` and still traps on overflow (ADR-0002).
    ordinary := -5;
    negated := -ordinary;

    if widened == -128 {
        if ordinary == -5 {
            if negated == 5 {
                exit(0);
            }
        }
    }
    exit(1);
}
```

A leading `-` on a literal is folded into the literal during lowering (ADR-0038): `-128` becomes one constant, not a negation of `128`. That is precisely what makes a signed minimum expressible, since the minimum of a two's-complement type is not the negation of any value the type can hold.

The distinction matters. Folding applies only to a literal. Negating a *value* — as in `-ordinary` — still lowers to a real negation operation and still traps on overflow (ADR-0002). Where the tower file above only *declares* these constants, this one *executes* them, so a back end that interned the wrong bits for a negative constant fails here even while the tower file would still pass.

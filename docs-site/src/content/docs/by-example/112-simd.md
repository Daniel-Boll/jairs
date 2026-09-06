---
title: "#simd"
description: "A vector type at the width the hardware actually has — one machine register, wrapping arithmetic on integers, and a refusal for any width no register can hold."
sidebar:
  order: 112
---

`#simd [N]T` is one machine register: `#simd [4]s32` is four lanes of `s32` in sixteen bytes, and `a +% b`
adds every lane in a single instruction in Cranelift and in LLVM (ADR-0148). The comptime VM has no
vector register, so it lowers the same operation to a **loop** — the first time in this project two
engines execute a different *number* of operations for the same MIR — and the three-way differential
exists to prove all three still agree byte for byte despite that.

## One register's worth of lanes

```jr
#import "Basic";

LANES :: 4;

// **A vector crosses a call boundary in a register**, not through a hidden pointer: `Repr::Vector` is
// deliberately not an aggregate, so `returns_via_sret` says no and sixteen bytes travel in `v0`
// (ADR-0148 §1). A test that only did arithmetic in one body would not notice if that were wrong.
doubled :: (v: #simd [LANES]s32) -> #simd [LANES]s32 {
    return v +% v;
}

main :: () {
    total := 0;

    a: #simd [LANES]s32;
    a[0] = 1;
    a[1] = 2;
    a[2] = 3;
    a[3] = 4;

    b: #simd [LANES]s32;
    b[0] = 10;
    b[1] = 20;
    b[2] = 30;
    b[3] = 40;

    // The wrapping spelling, which is the language's decision rather than a limitation (§6): no
    // target has a per-lane overflow flag, so a trapping vector add would need a compare and a
    // branch for every lane — and letting `+` wrap *only* on a vector would give one spelling two
    // meanings. `+ - *` on an integer vector are E0285, and name these.
    sum := a +% b;
    total = total + cast(s64, sum[0]) + cast(s64, sum[1]) + cast(s64, sum[2]) + cast(s64, sum[3]);

    // `*%` and `-%`: the product is `[10, 40, 90, 160]` and `less` is `[9, 39, 89, 159]`. Only lane
    // 0 is read, on purpose — a back end that wrote a correct result to the *wrong lane* would
    // change this answer, while summing every lane would hide exactly that mistake.
    one: #simd [LANES]s32;
    one[0] = 1;
    one[1] = 1;
    one[2] = 1;
    one[3] = 1;
    product := a *% b;
    less := product -% one;
    total = total + cast(s64, less[0]) - 1;

    // **Wrap-around at the boundary, asserted rather than assumed.** `S32_MAX +% 1` is `S32_MIN`, and
    // `S32_MIN + S32_MAX + 1` is 0 — so a saturating engine would give 1 here and a trapping one
    // would not finish.
    edge: #simd [LANES]s32;
    edge[0] = 2147483647;
    edge[1] = 0;
    edge[2] = 0;
    edge[3] = 0;
    wrapped := edge +% one;
    total = total + cast(s64, wrapped[0]) + 2147483647 + 1;

    // Float lanes get `+ - * /`, exactly as a scalar float does: nothing traps, so there is no
    // wrapping question and no reason to require a second spelling (§6).
    f: #simd [2]float64;
    f[0] = 1.5;
    f[1] = 2.5;
    g: #simd [2]float64;
    g[0] = 4.0;
    g[1] = 10.0;
    fsum := f + g;
    fmul := f * g;
    fdiv := g / f;
    fsub := g - f;
    total = total + cast(s64, fsum[0] + fmul[1] + fdiv[1] - fsub[1] + fsub[0] - 1.0);

    // The call, whose answer is 2+4+6+8 = 20.
    d := doubled(a);
    total = total + cast(s64, d[0] + d[1] + d[2] + d[3]);

    // `.count` folds to a constant from the type, exactly as an array's does — nothing is loaded,
    // because the lane count lives in the type.
    total = total + a.count;

    exit(total);
}
```

Integers get the **wrapping** operators `+% -% *%` and floats get the ordinary `+ - * /` — a deliberate
split, not an oversight. No hardware vector unit carries a per-lane overflow flag, so a trapping vector
add would need a compare and a branch inserted after every lane, which defeats the entire reason to
reach for a vector in the first place; and letting plain `+` wrap only when its operands happen to be a
vector would give one spelling two meanings depending on the type either side of it. `/` and `%` on an
integer vector are refused outright rather than scalarised into a loop, for the same reason: a construct
chosen for speed that silently becomes a loop under the hood is the performance-domain twin of a silent
miscompile.

`a.count` folds to `4` from the type itself, exactly the way an array's `.count` does — nothing is
loaded at run time, because the lane count is part of the type rather than data carried alongside it.
The exit status is a checksum of every half: integer wrapping add (110), integer `*%`/`-%` (8), the
wrap-around boundary case (0), float arithmetic (28, truncated), the call (20) and the lane count (4),
for **170**.

## Why the width must fit a real register

```jr
// EXPECT: E0285 — a `#simd` width no machine register has (ADR-0148 §2)
main :: () {
    wide: #simd [4]s64;
}
```

`[4]s64` is thirty-two bytes, and a vector *operation* on this project's supported hardware compiles at
exactly sixteen — an `iadd` on a 32-byte vector fails Cranelift's backend with `Unexpected SSA-value
type`, which is what probing found before this feature was designed at all. The type *constructor* would
happily build one; only an operation on it refuses, with E0285, naming the widths that do fit — `[2]s64`
among them — rather than stating the rule and leaving a reader to guess. This is a machine fact stated in
the language on purpose: a portable-looking `#simd [8]s64` would have to be split into several registers
or scalarised into a loop, and a directive silently ignored is worse than one rejected, with unusual
force for a construct chosen specifically for speed. The same code also covers an element that is not a
numeric scalar — `#simd [2]string`, a pointer, or a `bool` — none of which a lane can hold arithmetic
for.

**`size_of(#simd [4]s32)` is deliberately absent**, and it is not a gap this feature opened: a
*structural* type argument does not parse at all in this position — `type_info([4]s64)` does not either
— which has stood since long before `#simd` existed. A named alias would sidestep it, and this language
has no type aliases yet either. So a program that needs the width asserts it where E0285 already proves
it, in a `type-errors` corpus file, rather than at `size_of`.

## Why a game reaches for this

Four lanes of `s32` or two of `float64` is the shape a batch of particle positions, a tile grid's row, or
four bounding-box comparisons at once wants, and `+% -% *%`/`+ - * /` on the whole register cost one
instruction rather than a loop over four scalars. `Math`'s `Vector2`/`Vector3`/`Vector4` are plain
`float64` structs with no SIMD packing at all (see [Maths for games](/games/math-for-games/)) — a game
that wants a genuinely vectorised transform pipeline reaches for `#simd [N]float64` directly rather than
through `Math`'s types.

See also [Book I — The Jairs Language](/language/introduction/).

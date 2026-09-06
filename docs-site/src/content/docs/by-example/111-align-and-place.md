---
title: "#align and #place"
description: "Per-field layout control — a minimum alignment with #align, and an explicit byte offset with #place, both folded through the same layout computation every other struct uses."
sidebar:
  order: 111
---

`#align` raises one field's minimum alignment above what its type would naturally get; `#place`
overrides its byte offset entirely, letting several fields share the same bytes on purpose (ADR-0144).
Neither is a new code path: both fold through `jr-pool`'s one shared layout computation, so a struct
using either is measured by the same routine that measures every other struct — which is what lets three
independently written engines agree on the resulting offsets.

## Raising an alignment, and overlaying fields on purpose

```jr
#import "Basic";

ALIGNMENT :: 16;

Aligned :: struct {
    tag: u8;
    value: s64 #align ALIGNMENT;
}

// Three fields over the same eight bytes. This is what a `union` cannot say: `header` does *not*
// overlap, and only the last three do (ADR-0144 §4).
Overlay :: struct {
    header: s64;
    lo: u8 #place 8;
    hi: u8 #place 9;
    both: s64 #place 8;
}

// A deliberately unaligned field: 3 is not a multiple of 8. Nothing refuses it, and all three
// engines read it back correctly because each computes its own addresses — the LLVM back end
// claims `align 1` on every access for exactly this reason.
Odd :: struct {
    pad: u8;
    value: s64 #place 3;
}

main :: () {
    a: Aligned;
    a.tag = 1;
    a.value = 2;

    o: Overlay;
    // Clearing through the wide field first, so the two bytes read below are the ones written
    // after it rather than whatever the zeroing left.
    o.both = 0;
    o.lo = 3;
    o.hi = 4;

    d: Odd;
    d.value = 7;
    d.pad = 1;

    total := cast(s64, o.lo) + cast(s64, o.hi) * 10;
    total = total + size_of(Aligned) + size_of(Overlay) + size_of(Odd);
    total = total + d.value;
    exit(total);
}
```

`Aligned.value` would naturally land at offset 8 on an `s64`'s own alignment anyway, so `#align 16`'s
effect here is on the *struct's* alignment: the whole type must now start on a 16-byte boundary, which
rounds its size up from 24 to 32. `Overlay` is what a `union` in this language cannot express: `header`
occupies bytes 0–7 on its own, while `lo`, `hi` and `both` are three different *views* of bytes 8–15 —
placed there explicitly rather than inferred, so the overlap is visible in the declaration rather than
implied by a union tag nobody wrote. `Odd.value #place 3` sits at byte 3, which is not a multiple of 8 —
nothing in this language refuses that, because each of the three engines computes the address for
itself rather than assuming natural alignment, so a misaligned access is slow on some hardware rather
than wrong on any of it.

The exit status is a checksum of offsets and sizes, so a wrong offset changes the *number* rather than
only the shape: `size_of(Aligned)` is 32, `size_of(Overlay)` is 16, `size_of(Odd)` is 16, `lo + hi * 10`
is `3 + 40 = 43` (proof that the overlay genuinely overlays), and `d.value` reads back `7` through a
deliberately unaligned field. `32 + 16 + 16 + 43 + 7 = 114`.

## What each attribute refuses, and why those refusals stop where they do

```jr
// EXPECT: E0282 — an `#align` that is not a usable alignment (ADR-0144 §3)
Misaligned :: struct {
    value: s64 #align 12;
}
```

`#align` refuses a value that is not a power of two — `12` looks like a size, and a size is exactly what
an alignment is not — and also refuses zero (every address is 0-aligned, so the request means nothing)
and anything above 4096, one page, past which a stack slot cannot promise the alignment it was asked
for. **What is deliberately not refused** is a value *below* the type's own alignment: `#align 1` on an
`s64` is already satisfied, because a field's natural alignment is not always knowable while signatures
are being resolved — a field whose type is a struct resolved later has no layout yet — and a rule
enforced only sometimes would be worse than one stated exactly.

```jr
// EXPECT: E0283 — a `#place` offset that is not usable (ADR-0144 §4)
Negative :: struct {
    header: s64;
    value: s64 #place -8;
}
```

`#place` refuses a negative offset — the smallest byte from the start of an aggregate is `0`, and a
negative value names no byte — under its own code, E0283, separate from `#align`'s E0282: the two
attributes have different rules, and a reader filtering by code wants to know which one they got wrong.
**What is deliberately not refused**: two fields placed at the *same* offset, which is `Overlay`'s whole
point above, and an offset that does not satisfy the field's own alignment, which every engine handles
by computing its own address rather than assuming one.

Both attributes share one further refusal, for the same reason an array length does: an operand that
needs *evaluation* — `#align 8 * 2`, or a `#run` — is refused, because a struct's fields are laid out
with its declaration, before the compile-time evaluator has run. A literal-valued named *constant* like
`ALIGNMENT` above is accepted through the same helper that resolves an array's length, so the boundary is
evaluation, not literalness.

## Why a game reaches for these

`#align` is what makes a buffer usable as a GPU uniform block or a SIMD load target when the hardware
demands a stricter boundary than the element type alone would give — a `Vector3` inside a struct destined
for `#simd` traffic, for instance. `#place` is the tool for a packed wire format or a save-file layout
that has to match bytes some other program already wrote, where the struct's fields must land at exact
offsets a format specification names rather than whatever this compiler would have chosen on its own.

See also [Book I — The Jairs Language](/language/introduction/).

---
title: Random
description: A deterministic xorshift64 pseudo-random generator whose state the caller owns, reproducible bit-for-bit between the two engines.
sidebar:
  order: 76
---

`Random` is a deterministic pseudo-random generator whose state the caller owns. It is built on `u64`
arithmetic that agrees **bit-for-bit** between the two engines — which is the whole point rather than a
background guarantee: a generator whose sequence differed between the comptime VM and native code would
fail the differential harness on its first call.

It is **not** cryptographically secure, and says so: a caller who needs unpredictability needs a
different tool.

## The API

```jr
/// A generator's state: one u64 word. Non-zero after `seed`.
Random :: struct { state: u64; }

/// The default seed, and the replacement for a zero one (the golden ratio's fractional bits).
GOLDEN :: #run golden_seed();

/// Seeds `rng`, replacing a zero seed with GOLDEN.
seed :: (rng: *Random, value: u64)

/// The next 64 random bits, advancing the generator.
next :: (rng: *Random) -> u64

/// A random s64 in [low, high), or `low` when the range is empty.
below :: (rng: *Random, low: s64, high: s64) -> s64

/// A random bool, each with probability one half.
coin :: (rng: *Random) -> bool
```

```jr
#import "Basic";
#import "Random";

main :: () {
    n := 0;

    r: Random;
    seed(*r, 12345);
    a := next(*r);
    b := next(*r);
    if a != b {
        n = n + 1;
    }

    // Reproducibility: the same seed, the same first value.
    r2: Random;
    seed(*r2, 12345);
    if next(*r2) == a {
        n = n + 2;
    }

    // `below` stays in [10, 20).
    r3: Random;
    seed(*r3, 999);
    v := below(*r3, 10, 20);
    if v >= 10 && v < 20 {
        n = n + 4;
    }

    // A second draw is also in range — the generator keeps working, not just its first call.
    w := below(*r3, 10, 20);
    if w >= 10 && w < 20 {
        n = n + 8;
    }

    // An empty range returns `low`.
    if below(*r3, 5, 5) == 5 {
        n = n + 16;
    }

    // A zero seed still advances, because it is replaced by GOLDEN.
    r4: Random;
    seed(*r4, 0);
    if next(*r4) != 0 {
        n = n + 32;
    }

    // Different seeds diverge: the state is per-generator, not shared.
    ra: Random;
    rb: Random;
    seed(*ra, 1);
    seed(*rb, 2);
    if next(*ra) != next(*rb) {
        n = n + 64;
    }

    // `coin` returns something both true and false across a short run.
    rc: Random;
    seed(*rc, 424242);
    trues := 0;
    i := 0;
    while i < 8 {
        if coin(*rc) {
            trues = trues + 1;
        }
        i = i + 1;
    }
    if trues > 0 && trues < 8 {
        n = n + 128;
    }

    exit(n);
}
```

## Why the caller owns the state

`next(*rng)` takes the generator by pointer, so a sequence is reproducible from its seed and two
generators are independent. The alternatives were rejected for the same reason: a **hidden global** cannot
give a test a clean sequence and is usually clock-seeded, which makes every run differ — the opposite of
what a differential harness needs; and the **context** is for what a callee needs without being handed it
(an allocator), while a random sequence is usually something a caller owns deliberately. A caller who
wants either can build it on top by putting a `*Random` in their own context struct.

## xorshift64, and its honest edges

The algorithm is xorshift64 (Marsaglia): three shift-and-xor steps on `u64` with the shift amounts 13, 7,
17 that give the sequence its full period. Every operation is on an unsigned word so it *wraps* rather
than trapping — an unsigned word is right here because a signed one's `>>` would carry the sign bit in and
change the sequence. It was chosen because its correctness is *obvious*, which beats better statistics for
a standard library's baseline; a higher-quality generator (PCG, xoshiro) is a later decision.

- **A zero seed is replaced, not rejected.** xorshift is stuck at zero forever, so `seed(rng, 0)` would
  give a stream of zeros; `seed` silently substitutes `GOLDEN`, the kinder answer for what is usually an
  uninitialised variable.
- **`below` is half-open** (`[low, high)`), matching every other range in the library, and returns `low`
  for an empty or inverted range rather than trapping. Its `% span` reduction is very slightly biased
  toward the low end (the classic modulo bias), which is named rather than hidden — negligible for small
  ranges, and a rejection-sampling version is additive later.
- **`GOLDEN` is declared through `#run`** for a real reason: the seed literal exceeds `s64`'s range, and a
  bare constant defaults to `s64` and would not fit. There is no `name : u64 : value` form yet, so a
  `u64`-range named constant is spelled by `#run` of a `-> u64` procedure whose return gives the literal
  its type.

The exit code is **255** — eight groups of one bit, every one depending on the generator computing the
same value in both engines.

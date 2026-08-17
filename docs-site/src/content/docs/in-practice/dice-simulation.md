---
title: A deterministic dice simulation
description: Roll two dice ten thousand times with a seeded PRNG and tally the totals.
sidebar:
  order: 4
---

This program rolls two six-sided dice ten thousand times and tallies how often each total
comes up. It uses the [`Random`](/language/the-standard-library/#random) module, and its
point is *determinism*: because the generator is seeded and its arithmetic agrees bit-for-bit
between the engines, the histogram is identical every run and identical under `jr run` and
`jr build`.

```jr
#import "Basic";
#import "Random";

// Roll two six-sided dice many times and tally how often each total comes up.
// The generator is seeded, so the histogram is identical every run — and identical
// in the bytecode VM and the native binary.
main :: () {
    r: Random;
    seed(*r, 20260817);

    hist: [13]s64;             // indices 2..12 are the reachable totals

    rolls := 10000;
    i := 0;
    while i < rolls {
        a := below(*r, 1, 7);  // a value in [1, 7)
        b := below(*r, 1, 7);
        sum := a + b;
        hist[sum] = hist[sum] + 1;
        i = i + 1;
    }

    // Print the tally. A total of 7 has the most ways to occur, so it should win.
    s := 2;
    while s <= 12 {
        print_int(s);
        print(": ");
        print_int(hist[s]);
        print("\n");
        s = s + 1;
    }
}
```

Output:

```
2: 285
3: 511
4: 841
5: 1107
6: 1400
7: 1625
8: 1375
9: 1105
10: 859
11: 593
12: 299
```

The classic triangular distribution: 7, with the most ways to be rolled, is the most common,
and the tails fall away toward 2 and 12.

## How it works

**Seeding the generator.** A `Random` is a struct holding a single `u64` of state. `seed(*r,
20260817)` initialises it (a zero seed would be silently replaced with a golden constant,
since xorshift is stuck at zero). Because the seed is fixed, the whole run is reproducible.

**Rolling.** `below(*r, 1, 7)` returns a value in the half-open range `[1, 7)` — that is, 1
through 6 — advancing the generator each call. Two rolls summed give a total from 2 to 12.

**The histogram is a fixed array.** `hist: [13]s64` is zeroed on declaration, and we index it
by the total (2..12). Indices 0 and 1 stay zero; a `[13]` array has room through index 12,
and indexing it out of range would [trap](/language/arrays-and-views/#fixed-arrays), which is
why the array is sized to the largest total.

## Why determinism matters here

The `Random` state is **caller-owned**, not a hidden global seeded from the clock. That is a
deliberate choice: a clock-seeded global would make this program's output different every run
and impossible for the differential harness to check. Here, `seed(*r, 20260817)` fixes the
sequence, and the generator's `u64` arithmetic is defined to agree between the engines — so
"the VM and the native binary produce the same histogram" is a checkable fact, not a hope.
Change the seed and you get a different — but equally reproducible — run.

## What it demonstrates

- The `Random` module: `seed` and `below`, with caller-owned state.
- A fixed array as a histogram, sized to its index range.
- Determinism as a designed property, tied to Jairs'
  [two-engine agreement](/language/introduction/#two-engines-one-language).

Next: [3D vector math](/in-practice/vector-math/).

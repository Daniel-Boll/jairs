---
title: switch & exhaustiveness
description: switch over enums with exhaustiveness checking, no fallthrough, else for non-enums, and single scrutinee evaluation.
sidebar:
  order: 26
---

`switch` (ADR-0067) selects one arm by comparing a scrutinee against each `case`. Over an enum it is checked for **exhaustiveness**; over any other type an `else` arm makes it total. There is no fallthrough, and the scrutinee is evaluated exactly once.

```jr
#import "Basic";

Colour :: enum {
    RED;
    GREEN;
    BLUE;
}

/// Exhaustive over `Colour` with **bare** members, and no `else`.
describe :: (c: Colour) -> s64 {
    r := 0;
    switch c {
        case .RED;
            r = 1;
        case .GREEN;
            r = 2;
        case .BLUE;
            r = 4;
    }
    return r;
}

/// The same match written with **qualified** members.
describe_qualified :: (c: Colour) -> s64 {
    r := 0;
    switch c {
        case Colour.RED;
            r = 1;
        case Colour.GREEN;
            r = 2;
        case Colour.BLUE;
            r = 4;
    }
    return r;
}

/// A `switch` on an `s64`, made total by `else`.
classify :: (n: s64) -> s64 {
    r := 0;
    switch n {
        case 1;
            r = 10;
        case 2;
            r = 20;
        else;
            r = 30;
    }
    return r;
}

two :: () -> s64 {
    return 2;
}

/// Switches on a call, to show the scrutinee is evaluated once.
from_call :: () -> s64 {
    r := 0;
    switch two() {
        case 1;
            r = 100;
        case 2;
            r = 200;
        else;
            r = 300;
    }
    return r;
}

main :: () {
    n := 0;

    if describe(Colour.RED) == 1 {
        n = n + 1;
    }
    if describe(Colour.BLUE) == 4 {
        n = n + 4;
    }
    if describe_qualified(Colour.GREEN) == 2 {
        n = n + 8;
    }
    if classify(2) == 20 {
        n = n + 16;
    }
    if classify(99) == 30 {
        n = n + 32;
    }
    if from_call() == 200 {
        n = n + 64;
    }

    if n == 127 {
        exit(0);
    }
    exit(1);
}
```

Six behaviours are worth calling out:

- **Bare `.RED` works as a case.** The scrutinee's type is the expected type against which the arm's value resolves — the same mechanism `c == .GREEN` uses. `describe` relies on this.
- **Qualified `Colour.RED` is equally legal.** `describe_qualified` is `describe` with only the spelling changed; the two name the same members and return the same values.
- **No fallthrough** (ADR-0067 §5). An arm runs and the `switch` ends — `describe` returns one value rather than falling into the next arm.
- **Exhaustiveness over an enum, and `else` for anything else.** `describe` covers every `Colour` member and needs no `else`; omitting a member would be a compile error (E0258), and adding a redundant `else` there would be E0260 since it could never run. But an `s64` has no finite member set to be exhaustive over, so `classify` uses `else` as the catch-all to make itself total.
- **The scrutinee is evaluated once**, before any comparison. `from_call` switches on `two()`; the call happens once, not once per arm. (In the MIR snapshot this shows as a single `call` before the first comparison.)
- **Both engines agree**, because a `switch` lowers to the same branch chain an `if`/`else if` over the same comparisons would produce — no new MIR node and no back-end change.

The `exit` gives each check teeth under the differential harness: a wrong arm, a fallthrough, or a scrutinee evaluated twice would each produce a different exit status, so `jr run` and `jr build` can be asserted to agree.

An aside from the corpus: this wave (W4.5) moved *before* W4 in the plan. The plan had ordered it later "because exhaustiveness diagnostics want comptime type info", but checking showed that was a want, not a need — exhaustiveness over an enum needs only the member set, which is already populated during checking. Running `c == .GREEN` and `c == Colour.GREEN` confirmed the prerequisites already worked, so the wave came forward.

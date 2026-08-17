---
title: "#bake_arguments"
description: Partial application at declaration time — clone a procedure with some arguments fixed, producing a specialised procedure.
sidebar:
  order: 62
---

`#bake_arguments` is a partial application that produces a **specialised procedure** (ADR-0097).
`add_five :: #bake_arguments add(a = 5);` clones `add` with the parameter `a` **dropped from its list**
and `5` substituted for every use of it in the body. So `add_five` is an ordinary one-argument
procedure — callable, lowerable, inlinable, with nothing downstream needing to be taught about it.

```jr
#import "Basic";

/// Ordinary procedures, to specialise.
add :: (a: s64, b: s64) -> s64 {
    return a + b;
}

sub :: (a: s64, b: s64) -> s64 {
    return a - b;
}

/// A named bake of the **first** parameter.
add_five :: #bake_arguments add(a = 5);

/// A positional bake, which bakes the parameter at that index.
from_fifty :: #bake_arguments sub(50);

/// A named bake of the **second** parameter, so the kept parameter remaps from index 1 to 0.
minus_eight :: #bake_arguments sub(b = 8);

main :: () {
    // 5 + 37 == 42
    a := add_five(37);
    // 50 - 8 == 42
    b := from_fifty(8);
    // 50 - 8 == 42 — the same answer by the other bake, which is what proves the remap
    c := minus_eight(50);

    // Called again, so the clone is a reusable procedure rather than a one-shot: 5 + 0 == 5.
    d := add_five(0);
    // And a third call with a negative argument: 5 + (-5) == 0, so it contributes nothing — which is the
    // point, since a wrong substitution would make it contribute something.
    // 42 + 42 + 42 + 5 - 0 == 131
    exit(a + b + c + d - add_five(-5));
}
```

## The three spellings

- **Named** (`add(a = 5)`): uses the ordinary named-argument spelling rather than inventing a second
  syntax. `add_five` keeps only `b`.
- **Positional** (`sub(50)`): bakes the parameter at that index — here `a` — leaving `b` as the one
  live argument. `from_fifty(8)` is `50 - 8`.
- **Baking the second parameter** (`sub(b = 8)`): drops `b`, so the kept parameter `a` must be
  *remapped* from index 1 to index 0. `minus_eight(50)` is `50 - 8`. This is the case that would
  silently read the wrong parameter if the remap step were skipped — which is why `from_fifty` and
  `minus_eight` reach the same answer `42` by different routes.

## How it works, and why it was the right piece

The mechanism is the one `$N` comptime-value instantiation already uses: drop the marked parameters,
rewrite their uses into literals, remap the remaining indices. The one difference is *when* it happens —
during **lowering** rather than at an instantiation, because a baked procedure is a **declaration**, not
a call site. Reusing that machinery is why this was the right piece to finish the macro family with.

The exit code is the observable checksum. `add_five` is called twice more (`add_five(0)` gives 5, and
`add_five(-5)` gives 0), so the total is `42 + 42 + 42 + 5 - 0 == 131`, and both engines must agree. A
wrong substitution, a missed drop, or a bad remap would give both engines the *same* wrong number, so
only asserting on the value catches it.

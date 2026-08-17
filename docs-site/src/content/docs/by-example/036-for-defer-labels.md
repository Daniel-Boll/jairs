---
title: for, defer & labels
description: The for loop's iteration forms, labelled break and continue, and deferred statements that run on scope exit.
sidebar:
  order: 36
---

`for`, labelled `break`/`continue`, and `defer` share one design (ADR-0049) because they share
one question: **what is a scope exit?** `defer` needs the answer, `break` and `continue` need it,
and a label changes which scope they leave.

```jr
#import "Basic";

sum_view :: (xs: []s64) -> s64 {
    t := 0;
    for x: xs {
        t = t + x;
    }
    return t;
}
```

## The iteration forms

`for x: collection` binds each element to `x`. The element is a *copy*, so assigning to it would
not touch the underlying storage:

```jr
    for x: buf {
        total = total + x;
    }
```

A range is half-open — `0..4` runs four times, `0..0` runs none:

```jr
    for i: 0..4 {
        r = r + i;
    }
    for i: 0..0 {
        empty = empty + 1;   // never runs
    }
```

Both an element and an index can be bound, `for x, i:`, giving the body a real index local:

```jr
    for x, i: buf {
        indices = indices + i;
    }
```

Reversed iteration is `for < x:`. The fold in the example is order-sensitive on purpose (forward
gives 32, reverse 85), so a `for <` that silently ran forward would be caught:

```jr
    for < x: buf {
        rev = rev * 2 + x;
    }
```

A view iterates the same way as an array, but its length comes from a *load* of `.count` rather
than a constant baked in from a type — the two shapes the loop's bounds check was built to share:

```jr
    if sum_view(buf[]) == 15 {
        n = n + 32;
    }
```

Internally the induction variable is kept distinct from the element variable (sharing them once
turned a loop into an infinite one), and `continue` targets a dedicated step block so it still
advances the loop.

## Labelled break and continue

An unlabelled `break` leaves the innermost loop:

```jr
    for a: rows {
        for b: cols {
            inner = inner + 1;
            break;               // leaves the inner loop only
        }
    }
```

A loop can carry a label, and `break`/`continue` can name it to act on the outer loop. A labelled
`break` leaves both loops:

```jr
    outer: for a: rows {
        for b: cols {
            outer_hits = outer_hits + 1;
            break outer;         // body runs exactly once
        }
    }
```

A labelled `continue` restarts the named outer loop, so nothing after it in the inner body runs:

```jr
    lbl: for a: rows {
        for b: cols {
            if b == 0 {
                continue lbl;
            }
            skipped = skipped + 1;   // never reached
        }
    }
```

## defer

A `defer` statement runs when its scope exits, after the statements before it:

```jr
    {
        defer scoped = scoped + 1;
        scoped = scoped + 10;
    }
    // scoped == 11
```

Multiple `defer`s in one scope run in **reverse** order, so paired acquisition and release is
expressible:

```jr
    {
        defer order = order * 2;
        defer order = order + 3;
    }
    // order == 6: (0 + 3) then * 2
```

Inside a loop, a `defer` runs **per iteration**, not accumulated to the end of the procedure:

```jr
    for x: buf {
        defer per = per + 1;
    }
    // per == 4
```

Crucially, a `defer` runs on the **`break`** path too, not only on fall-through — the claim most
easily got wrong, since a `defer` that only ran at the closing brace would look correct in any
program that never breaks:

```jr
    for x: buf {
        defer on_break = on_break + 1;
        break;
    }
    // on_break == 1 — the defer registered in the first iteration runs
```

## Observable result

```jr
    if n == 8191 {
        exit(0);
    }
    exit(1);
```

Every assertion folds into the bitmask `n`, and encoding it in the exit status makes the loop and
`defer` behaviour observable so `jr run` and `jr build` can be asserted to agree byte-for-byte.

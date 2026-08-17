---
title: Multiple return values
description: A procedure returns several values at once, destructured at the call site with exact arity.
sidebar:
  order: 30
---

A Jairs procedure may return more than one value. This is the mechanism the language's error
model rests on (ADR-0008 chose it over exceptions and over a `Result` type): the canonical shape
is a value paired with a flag saying whether the value is meaningful.

```jr
#import "Basic";

divide :: (a: s64, b: s64) -> (s64, bool) {
    if b == 0 {
        return 0, false;
    }
    return a / b, true;
}

pair :: () -> (s64, s64) {
    return 40, 2;
}

triple :: (n: s64) -> (s64, bool, u8) {
    return n * 2, n > 0, 7;
}

padded :: () -> (u8, s64) {
    return 3, 999;
}
```

The return type is a parenthesised list, `-> (s64, bool)`. Internally this interns as a
*structural results aggregate* whose memory layout is exactly a struct's, so the same
caller-allocated calling convention that carries a returned struct carries a multi-value return.

## Destructuring at the call site

A call is destructured into a list of targets:

```jr
q, ok := divide(7, 2);
```

The `:=` form declares `q` and `ok` fresh. The plain `=` form assigns into locals that already
exist:

```jr
q, ok = divide(9, 3);
```

Two rules make this safe rather than merely convenient:

- **Exact arity.** The number of targets must match the number of results. This is what makes
  adding or reordering a result a compile error at every call site rather than a silent change of
  meaning. (The refusal for a wrong count lives in the invalid corpus, since a file that must
  parse cleanly cannot hold one.)
- **Positional binding.** The first target takes the first result, and so on. The example
  deliberately uses procedures like `pair` that return two values of the *same* type holding
  *different* numbers — 40 and 2 — because that is the only arrangement in which a swapped offset
  would be visible; had both been `true`/`true`-shaped, reading result 1 as result 0 would still
  look plausible.

## Discarding results with `_`

A `_` in a target position is a discard: it binds nothing and never becomes a local.

```jr
r, _ := divide(10, 5);   // last position discarded — the flag is thrown away
_, present := divide(8, 4);   // first position discarded — only the flag is bound
_, _ := divide(1, 1);    // every position discarded, same as a bare call statement
```

Both positions are exercised because a first-position discard and a last-position one take
different paths through the target list. Discarding *every* result is legal and means the same as
calling `divide(1, 1)` as a plain statement.

## Making the result observable

`main` folds each passing assertion into a bitmask `n`, then exits with a status that encodes
exactly which checks passed:

```jr
    if n == 131063 {
        exit(0);
    }
    exit(1);
```

This is the corpus convention: only two corpus programs print anything, so a computation has to
be made observable through the process exit status, otherwise the differential harness would be
comparing silence against silence. With the `exit`, `jr run` and `jr build` can be asserted to
agree byte-for-byte. Note the total here is 131063, *not* the full 131071 — the `bad` check is an
`if`/`else`, so exactly one of its two branches contributes.

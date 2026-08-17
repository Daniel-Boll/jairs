---
title: Errors and traps
description: How Jairs handles the two kinds of failure — recoverable errors as values, and traps for the unrecoverable.
sidebar:
  order: 12
---

Jairs draws a firm line between two kinds of failure, and handles them differently on
purpose. A **recoverable error** — a file that isn't there, a key that isn't in the map — is
an ordinary value you return and check. An **unrecoverable fault** — integer overflow, an
out-of-range index, a null dereference — is a **trap**: the program stops immediately, at a
known location. There are no exceptions, and there is no `panic!`-vs-`Result` split to learn;
there is this one distinction.

## Errors are values

Jairs' error model is Jai's: a procedure returns its result **and** a status, and the caller
must look at both. You saw the mechanics in [Procedures](/language/procedures/):

```jr
get :: (m: *Map(s64, s64), key: s64) -> (s64, bool) {
    // … returns (value, true) on a hit, (_, false) on a miss …
}

main :: () {
    v, ok := get(m, 42);
    if ok {
        use(v);
    } else {
        // handle the miss — no exception was thrown
    }
}
```

This is the whole model for expected failure. It composes with the standard library's
conventions: `List.push` returns `false` when an allocation fails, `Map.get` returns `(_,
false)` for a missing key, `String.find` returns `-1` when the needle is absent. Which shape a
routine uses is a small design decision each one makes — a two-value return when the *element*
has no out-of-domain value to spare, a sentinel when it does.

The planned `#must` attribute will make it a *compile error* to ignore the status flag,
closing the "forgot to check" gap. It is <span class="jairs-status absent">absent</span> today
and owed its own design.

## Traps

A trap is what happens when the program asks for something with no sensible answer. Jairs
traps rather than returning garbage or invoking undefined behaviour:

- **integer overflow** (`+ - *` and their compound forms), including negating the most-negative
  integer;
- **division or modulo by zero**;
- an **out-of-range array index** (unless bounds checks are off);
- a **shift count** out of range or negative;
- reading the **wrong case of a `variant`**;
- **calling through a null procedure pointer** (for example an uninstalled allocator);
- reading the **wrong type out of an `Any`** (`any_as` with a mismatched type).

A trap **names its source location** and prints a **backtrace** — the chain of procedure
frames that were live beneath it, innermost first. `jr run` reports a trap as exit code `4`;
the native binary faults the same way, and — the property Jairs is built around — both
engines report the *same* location, because the differential test checks that they do.

```
trap: integer overflow
  at add (prog.jr:12)
  at main (prog.jr:20)
```

Inlined frames do not appear in the backtrace, because at run time they did not exist — Jairs
reports what actually ran rather than reconstructing what the source looked like.

## Why traps, and not just more error values

The design value is that **a wrong answer is worse than a stop.** An overflowing `+` that
silently wrapped, or an out-of-range read that returned whatever was in memory, would let a
program continue computing on a value the programmer never intended. Traps convert those into
a located, reproducible failure. When modular arithmetic really is what you want, the wrapping
operators `+% -% *%` say so explicitly — the trap is the default precisely so that the
exceptions are visible.

## What a trap is not

A trap is **not** undefined behaviour. It is a defined outcome: stop here, with this message,
at this line. The only place Jairs has genuine undefined behaviour is a deliberately chosen
one — an out-of-range index when you built with `--no-bounds-check` — and that is the trade
you opted into, not a surprise.

Next: [Modules](/language/modules/), which is where `#import`, `#foreign`, and the standard
library come from.

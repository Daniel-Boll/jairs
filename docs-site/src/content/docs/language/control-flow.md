---
title: Control flow
description: if, while, for, switch, defer, and labelled loops.
sidebar:
  order: 5
---

Jairs' control flow is ordinary and visible — no hidden dispatch, no exceptions unwinding the
stack. This chapter covers the branching and looping constructs and the two that are a little
less familiar: `switch` with exhaustiveness checking, and `defer`.

## if / else

Braces are required around the body, but parentheses around the condition are **not**:

```jr
if sum > 5 {
    print("big\n");
} else if sum > 0 {
    print("small\n");
} else {
    print("zero or less\n");
}
```

A single statement may be written without braces:

```jr
if sum > 5  print("big\n");
```

The condition must be a `bool`. There is no "truthy" integer — `if n` where `n` is an `s64`
is a type error, by the same no-silent-conversion rule from [Values and
types](/language/values-and-types/).

## while

```jr
i := 0;
while i < 3 {
    i = i + 1;
}
```

`break` leaves the loop; `continue` skips to the next iteration.

## for

`for` iterates over an array, a view, or a numeric range. There are four shapes:

```jr
for x: buf        { … }   // element by element (x is a copy)
for x, i: buf     { … }   // element and index
for i: 0..n       { … }   // a half-open range: i goes 0,1,…,n-1
for < x: buf      { … }   // in reverse
```

A range `0..n` is half-open, so `0..4` runs four times and `0..0` runs none. The element in
`for x: buf` is a **copy** — assigning to `x` does not write back to `buf`. Iterating *by
reference* (`for *x`), treating a range as a first-class value, and iterating a user-defined
type are all <span class="jairs-status absent">absent</span> for now.

## Labelled break and continue

Loops can be labelled, and `break`/`continue` can name a label to act on an outer loop:

```jr
outer: for a: rows {
    for b: cols {
        if done  break outer;      // leaves BOTH loops
        if skip  continue outer;   // restarts the OUTER loop
    }
}
```

An unlabelled `break` or `continue` acts on the innermost loop, as usual.

## defer

`defer` schedules a statement to run when the enclosing scope exits — whatever path it exits
by, including a `break` out of a loop. Deferred statements run in **reverse order**, which is
what makes paired acquire/release read naturally:

```jr
{
    handle := open();
    defer close(handle);      // runs at the closing brace

    scratch := acquire();
    defer release(scratch);   // runs first, before close(handle)

    // … use handle and scratch …
}
```

`defer` is Jairs' replacement for RAII: there are no destructors, so cleanup is written
explicitly, and `defer` puts it next to the acquisition instead of at the far end of the
scope. A `defer` inside a loop body runs **once per iteration**, at the end of that
iteration — including on the iteration that `break`s.

## switch

`switch` matches a value against cases. There is **no fallthrough**: a case runs and the
switch ends.

```jr
describe :: (c: Colour) -> s64 {
    switch c {
        case .RED;     return 1;
        case .GREEN;   return 2;
        case .BLUE;    return 4;
    }
    return 0;
}
```

Over an `enum`, `switch` is **exhaustiveness-checked**: if you omit a member and provide no
`else`, that is a compile error — and adding an `else` that could never run is also refused.
This is one reason enums are worth having (see [Enums and flags](/language/enums-and-flags/)).

Over an integer there is no finite member set to be exhaustive about, so an `else` arm is
what makes the match total:

```jr
switch n {
    case 1;   r = 10;
    case 2;   r = 20;
    else;     r = 30;
}
```

Cases can be written with a bare member (`.RED`) or a qualified one (`Colour.RED`) — they
name the same thing. The scrutinee is **evaluated once**, before any comparison, so
`switch f() { … }` calls `f` a single time. A `switch` also destructures a
[`variant`](/language/structs-unions-and-variants/), taking the arm for whichever case is
live. Pattern matching, ranges, guards, multi-value cases, and `switch` as an expression are
all <span class="jairs-status absent">absent</span>.

Next: [Structs, unions and variants](/language/structs-unions-and-variants/), the three ways
Jairs groups fields.

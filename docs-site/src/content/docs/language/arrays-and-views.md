---
title: Arrays and views
description: Fixed-size arrays, bounds checking, and the view type that borrows them.
sidebar:
  order: 8
---

Jairs has two array-shaped types today: **fixed-size arrays** `[N]T`, which own their
storage, and **views** `[]T`, which borrow a run of elements. (A growable, heap-backed
dynamic array is provided by the [`List`](/language/the-standard-library/) module rather than
as a language built-in.)

## Fixed arrays

`[N]T` is an array of exactly `N` elements of type `T`, laid out inline. It is **zeroed by
default** and **bounds-checked**:

```jr
buf: [4]s64;          // four zeroed s64s
buf[0] = 1;
buf[1] = 2;
buf[2] = 4;
buf[3] = 8;
count := buf.count;   // 4
```

Indexing out of range **traps** with a source location. The length `N` may be a literal, a
**named constant** whose value is a literal, or a `$N` comptime-value parameter (see
[Polymorphism](/language/polymorphism/)):

```jr
N :: 4;
grid: [N]s64;         // fine — N is a literal one name away
```

A length that needs *evaluation* — arithmetic like `[2 + 2]u8`, a `#run`, or another file's
constant — is <span class="jairs-status absent">absent</span>, as are array *literals* like
`[1, 2, 3]`.

### Turning off the bounds check

Bounds checking is a build setting, not a language rule. You can build without it:

```sh
jr build prog.jr --no-bounds-check    # also: jr run always checks
```

or opt a single procedure out with `#no_abc` on its header, whatever the build says.
Compile-time execution (`#run`) **always** checks regardless — a trap at compile time is a
diagnostic, not a program behaviour, so eliding it would fold garbage into a constant.

With the check off, an out-of-range index is undefined behaviour — that is precisely the
trade. A valid program computes the same answer either way, in both engines; only the safety
net changes.

### Zeroing, and opting out

An array is zeroed slot by slot, and the compiler tracks definedness *per slot* — which is
why an array is treated differently from a scalar. `buf: [20]u8 = ---;` opts out of the
zeroing when you are about to fill it yourself.

## Views

A view `[]T` is a `{pointer, count}` pair — a borrowed window onto elements someone else
owns. You make one from an array with `buf[]`, index it with `xs[i]`, ask its length with
`xs.count`, and — crucially — **writes through a view reach the underlying array**:

```jr
sum_view :: (xs: []s64) -> s64 {
    t := 0;
    for x: xs {
        t = t + x;
    }
    return t;
}

main :: () {
    buf: [4]s64;
    buf[0] = 1; buf[1] = 2; buf[2] = 4; buf[3] = 8;

    total := sum_view(buf[]);      // pass a view of the whole array
}
```

A view can be **returned from a procedure**, which is what makes it useful as an interface
type: a routine can hand back a window into a buffer it was given.

### view() — a view from a pointer and a count

When you have a raw pointer and a length — the shape a heap allocation gives you — you build
a view with `view(p, n)`:

```jr
// (illustrative) sort the live elements of a growable list in place
sort_ints(elements(list));         // `elements` returns a []s64 built via view()
```

This is how the standard library's `List`, `Sort` and the view type cooperate on one buffer
with no copy. The element type comes from the pointer, so nothing is asserted; the **count is
unchecked** — a pointer's allocation size is tracked nowhere — and that is stated plainly
rather than pretended away.

Sub-slicing (`buf[1..3]`) and `==` on views are <span class="jairs-status absent">absent</span>.

## Arrays vs views, at a glance

| | `[N]T` | `[]T` |
| --- | --- | --- |
| Owns storage | yes, inline | no, borrows |
| Length | part of the type | a runtime field (`.count`) |
| Bounds-checked | yes | yes |
| Returned from a proc | as a value (a copy) | as a window |

Next: [Operators and overloading](/language/operators-and-overloading/), where the arithmetic
and comparison rules — including the ones that differ from C — are laid out in full.

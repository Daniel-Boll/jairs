---
title: Arrays and views
description: Fixed-size arrays, bounds checking, and the view type that borrows them.
sidebar:
  order: 8
---

Jairs has four array-shaped types today: **fixed-size arrays** `[N]T`, which own their
storage; **views** `[]T`, which borrow a run of elements; **dynamic arrays** `[..]T`, which
own a growable heap block; and `#simd [N]T`, a vector held in a machine register rather than
memory. This chapter covers all four, plus the two attributes — `#soa` and `#align`/`#place`
— that control a struct's layout.

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

A length that needs *evaluation* — arithmetic like `[2 + 2]u8`, a `#run`, or a constant from
another file — is <span class="jairs-status absent">absent</span>: the length must be an
integer literal or a name for a constant whose value is one, because the compiler resolves an
array's length before the const-evaluator that would compute the other kinds exists yet at
that phase.

### Array literals

A fixed array can be written as a literal by naming its element type before the brackets:

```jr
a := s64.[10, 20, 30];    // a [3]s64
b := u8.[1, 2, 255];      // element type types each literal, so an overflow is a type error
```

`T.[a, b, c]` is a `[N]T` where `N` is the number of elements — naming the type answers the
length, the element-constancy, and the context-typing questions in one spelling. The bare
`[1, 2, 3]` form (with the element type inferred from the elements) is <span
class="jairs-status absent">absent</span> — an array literal must name its element type.

An array literal is not a *place*: `s64.[1, 2][0] = 5` is refused, and a literal cannot yet be
folded into a compile-time constant (`A :: s64.[1, 2, 3];` is refused) — every use so far is
inside a procedure body.

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

## Dynamic arrays

`[..]T` is a **compiler-known** dynamic array: three words, `{data: *T, count: s64, capacity:
s64}`, laid out the same way a hand-written growable buffer would be. All three fields are
places — readable and writable — because a library `push` needs to update `.count` and
`.capacity` after reallocating:

```jr
xs: [..]s64;          // zeroed: data is null, count and capacity are 0
xs.count = 0;         // an ordinary field write, exactly like a struct's
```

The compiler owns the *type* and the *layout*; it does not own the *operations*. Growing,
pushing and freeing are library code — [`List`](/language/the-standard-library/) is built
directly on `[..]s64`, rather than providing its own growable type as a wrapper around it.
There is no dedicated `for` shape for a dynamic array yet: reach its elements through
`view(xs.data, xs.count)` or a library helper that returns one.

## Struct layout: #soa, #align and #place

Three attributes hand a systems program control over layout that the compiler otherwise
chooses for you.

`#soa(N)` turns a struct into **structure-of-arrays**: each field's declared type becomes
`[N]T` instead of `T`, so a loop reading one field stays contiguous instead of striding over
its neighbours.

```jr
Entities :: struct #soa(4) {
    x: s64;
    hp: u8;
}
// lays out as { x: [4]s64, hp: [4]u8 }
```

Indexing an `#soa` struct is legal only as the receiver of a field access — `e[i].x` means
`e.x[i]` — because `e[i]` alone has no type of its own; used any other way it is refused.

`#align` and `#place` are field attributes, written after a field's type:

```jr
Header :: struct {
    kind: u8 #place 0;      // an exact byte offset — two fields may overlap on purpose
    flags: u32 #align 16;   // a *minimum* alignment; the field's own alignment still applies
}
```

`#align N` raises a field's alignment to `max(natural, N)` — it can only go up, never down, so
there is no way to accidentally underalign something. `#place N` puts a field at an exact byte
offset, and two fields placed at the same offset legitimately overlap, the same trade an
untagged `union` already makes. Both exist because a struct that crosses the `#foreign`
boundary or describes a hardware layout sometimes has to match bytes the language does not
otherwise let you place.

## #simd — vectors at register width

`#simd` marks a fixed array as a **vector**, held in a machine register rather than in memory,
with arithmetic operators instead of just storage:

```jr
a: #simd [4]s32;
b: #simd [4]s32;
c := a +% b;        // integer vectors take only the wrapping operators
```

The legal widths are exactly what a 128-bit register holds — `16×s8`, `8×s16`, `4×s32`, `2×s64`,
`4×float32`, `2×float64`, signed and unsigned — because a vector type only earns its keep when
it is a real register; any other width is a compile error naming the six shapes. An integer
vector takes only the **wrapping** operators `+% -% *%` (a lane can't trap — no target has a
per-lane overflow flag); a float vector keeps `+ - * /`, since a float never traps in the first
place. Integer `/` on a vector, comparisons, and swizzles are all <span
class="jairs-status absent">absent</span>.

## Arrays vs views, at a glance

| | `[N]T` | `[]T` |
| --- | --- | --- |
| Owns storage | yes, inline | no, borrows |
| Length | part of the type | a runtime field (`.count`) |
| Bounds-checked | yes | yes |
| Returned from a proc | as a value (a copy) | as a window |

Next: [Operators and overloading](/language/operators-and-overloading/), where the arithmetic
and comparison rules — including the ones that differ from C — are laid out in full.

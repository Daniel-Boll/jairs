---
title: The standard library
description: The in-Jairs modules — Basic, String, Sort, Array, List, Map, Math, Random.
sidebar:
  order: 18
---

Jairs' standard library is written **in Jairs**, not inside the compiler. That is a design
commitment, not an accident: it forces the language to be expressive enough to write its own
library, and it means the library is something you can read to learn what the language means.
Every module here is a `.jr` file under `modules/`.

A recurring constraint shapes the containers: a parameterised struct declared in a module was,
until recently, unusable across a module boundary, so `Array`, `List` and `Map` are provided as
**concrete `s64` instances** rather than fully generic types. Where you see `Array(s64)` or
`Map(s64, s64)`, that is why. It is the honest state of the library today, and the walkthroughs
in [Book III](/in-practice/) use exactly these concrete types.

## Basic

The bottom of the library — imported by essentially every program. It reaches libc through
`#foreign` and provides output and raw memory:

```jr
print(s: string)            // write s to stdout
print_line(s: string)       // print s then a newline
print_error(s: string)      // write s to stderr
print_int(n: s64)           // write n in decimal
write(fd, buf, count)       // the #foreign syscall underneath print
exit(status: s64)           // terminate the process
malloc(size) -> *u8         // raw allocation; null on failure
free(p: *u8)                // release; free(null) is a no-op
talloc(n) -> *u8            // temporary-storage bump arena
reset_temporary_storage()   // rewind the arena
```

`Basic` also declares the `Type_Info` and `Any` structs that [reflection](/language/reflection/)
uses. There is **no float printing** — `print_int` has no floating-point counterpart yet.

## String

String operations, split into a non-allocating half and an allocating half. The
non-allocating ones just read:

```jr
equal(a, b) -> bool          starts_with(s, prefix) -> bool
compare(a, b) -> s64         ends_with(s, suffix) -> bool
find(haystack, needle) -> s64 (or -1)   contains(h, n) -> bool
byte_at(s, index) -> s64 (or -1)        is_empty(s) -> bool
```

`equal` is what `==`-on-strings points you to (recall strings don't compare with `==`).
`byte_at` exists because `s.data[i]` doesn't compile — reading a byte from a `*u8` needs help.

The allocating half produces new strings through `context.allocator`, which the caller frees:

```jr
concat(a, b) -> string       substring(s, start, count) -> string
to_upper(s) -> string        to_lower(s) -> string
free_string(s: string)       // free one you got from the above
```

Because these use `context.allocator`, install one first (an uninstalled allocator traps) —
and a caller who wants arena behaviour installs an arena and gets it for every routine at once.

## Sort

A generic sort, where the **caller** supplies the ordering:

```jr
sort(xs: []$T, less: (T, T) -> bool)    // stable insertion sort, in place
sort_ints(xs: []s64)                    // the concrete s64 convenience wrapper
is_sorted(xs: []$T, less) -> bool
ints_sorted(xs: []s64) -> bool
```

It is **insertion sort** — `O(n²)`, said plainly — chosen because it is stable, needs no
allocation, and is short enough to read. The caller passing `less` rather than the module
requiring `<` is a language fact: selecting an operator implementation per instantiated type is
something the language cannot yet do, so the comparison is a parameter. `sort_ints` exists as
the concrete wrapper because a `$T` template can't be called across a module boundary.

## Array and List

Two container shapes with different contracts:

**`Array(s64)`** — a **fixed-capacity** array (16 elements), no heap, no cleanup:

```jr
push(a, v) -> bool    // false when full
pop(a) -> (s64, bool)     get(a, i) -> (s64, bool)     set(a, i, v) -> bool
clear(a)   is_empty(a) -> bool   is_full(a) -> bool
```

**`List(s64)`** — a **growable**, heap-backed array that **owns** its memory:

```jr
push(a, v) -> bool    // false only on out-of-memory; capacity doubles from 4
pop(a) -> (s64, bool)     get(a, i) -> (s64, bool)     set(a, i, v) -> bool
clear(a)   is_empty(a) -> bool
elements(a) -> []s64      // a view over the live elements — feed it to sort_ints
free_data(a)              // YOU must call this; there are no destructors
```

`List` is a separate module from `Array`, not a rewrite, precisely because their contracts
differ: an `Array` needs no cleanup, a `List` owns memory you must `free_data`. `elements`
returns a view, so `sort_ints(elements(list))` sorts a list in place with no copy — the library
composing with itself.

## Map

An open-addressed hash table, `s64 -> s64`:

```jr
put(m, key, value) -> bool     get(m, key) -> (s64, bool)
has(m, key) -> bool            remove(m, key) -> bool
size(m) -> s64                 free_map(m)
```

Linear probing with tombstone deletion, grown at 3/4 load. Its hash uses **wrapping** `u64`
arithmetic (`*%`) — the overflow is the mixing working — so both engines compute the same
bucket, which a differential-tested hash table depends on absolutely.

## Math

Exact, closed-form functions plus libm wraps:

```jr
abs min max sign clamp pow gcd          // integer, exact
floor ceil round fabs                    // float, exact
sqrt sin cos exp ln powf                 // via libm through the FFI
```

`Math` deliberately shipped its transcendentals as **libm wraps** rather than in-language
approximations: libm is correctly rounded and both engines call the same libm, so `sqrt(2.0)`
is bit-identical in the VM and native code. An in-language approximation's last bit could
differ between engines — the one thing the differential harness treats as a failure.

It also has **vector math**: `Vector2`, `Vector3`, `Vector4` with `+ - * /` and `==` operators
(scalar multiply in both orders), plus `dot*`, `cross`, `length*`, `normalize*`, `distance*`,
`lerp*`, and a `Matrix4` with the usual transforms. The operators cross the module boundary, so
`a + b` on an imported `Vector3` just works. A `Quaternion` is
<span class="jairs-status absent">absent</span>.

## Random

A deterministic xorshift64 generator whose state the **caller owns**:

```jr
Random :: struct { state: u64; }

seed(rng, value)             // a zero seed is replaced with a golden constant
next(rng) -> u64             // the next 64 random bits
below(rng, low, high) -> s64 // a value in [low, high)
coin(rng) -> bool
```

State is caller-owned rather than a hidden global so it is testable, and its `u64` arithmetic
agrees bit-for-bit between the engines — a sequence that differed would fail the harness on its
first call.

## Reading the source

Every one of these is a readable `.jr` file, and reading them is a good way to learn idiomatic
Jairs — how the allocator protocol is used, how a two-value return is spelled, how a view
borrows a buffer. The [Book III](/in-practice/) programs put several of them together.

Next: [Tooling](/language/tooling/) — the compiler driver, the language server, and the editor
integration.

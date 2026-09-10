---
title: Memory
description: The context, allocators, temporary storage, and pointer arithmetic — how Jairs manages memory without a GC.
sidebar:
  order: 11
---

Jairs has no garbage collector and no reference counting. Memory is managed explicitly, but
not *tediously*: an implicit `context` carries an allocator down every call chain, so a
routine deep in your program can allocate without every caller threading an allocator argument
through by hand. This chapter explains that machinery.

## The context

Every ordinary Jairs procedure receives a hidden **leading** parameter, the **context**, passed
by pointer. You never write it, but you can read and write it:

```jr
main :: () {
    context.allocator = my_alloc;   // install an allocator for everything below
    do_work();                       // do_work and its callees see it
}
```

Because the context is passed by pointer, a callee reads what its caller wrote — that is the
whole point. It also means a write is visible *downward and sideways but not upward in a
scoped way*: `f` setting `context.allocator` and returning leaves it set from the caller's view
too, because they share one context object. To isolate a callee's changes you use
`push_context`.

A `#c_call` procedure (the FFI shape) gets **no** context — it has nowhere to put one — which
is why a `#c_call` procedure cannot call an ordinary Jairs procedure.

## push_context

`push_context { … }` runs a block with its **own copy** of the context, restored on exit:

```jr
push_context {
    context.allocator = arena_alloc;
    build_something();          // uses the arena
}
// context.allocator is back to what it was
```

This is how you scope an allocator to a phase of work without leaking the choice to the rest
of the program.

## The allocator protocol

`context.allocator` is a **procedure pointer** (plus `context.allocator_free` and
`context.allocator_data` for the paired free and any state). Installing an allocator is one
line, and a callee allocates through it without knowing which allocator it got:

```jr
p := context.allocator(64);      // allocate 64 bytes through whatever is installed
```

A fresh context starts with a libc-like default allocator pair (ADR-0216); the state word and
temporary-storage fields start zero. A program may replace the pair with wrappers around libc or
with an arena, and every routine below picks up the active policy. A `#foreign` procedure still
cannot be installed directly because its calling convention lacks the hidden context parameter.

## Raw memory: malloc, free, null

At the bottom sits libc, reached through the foreign function interface in `modules/Basic`:

```jr
p := malloc(1024);     // *u8, or null on failure
if p == null { … }
free(p);               // passing null is a defined no-op
```

`null` is the null pointer. In the compile-time VM, `malloc`/`free` are satisfied from the
VM's own linear region, so a pointer round-trips there too — which is what lets a growable data
structure be tested at compile time.

## Temporary storage

For scratch memory a computation throws away wholesale — building a string to print, say —
Jairs has a per-context bump arena:

```jr
p := talloc(256);              // hand out 256 bytes from the arena
// … use p …
reset_temporary_storage();     // rewind the whole arena at once
```

`talloc` bumps a cursor; `reset_temporary_storage()` rewinds it, freeing everything at once.
There is no per-piece free — that is the point of an arena. Pointers handed out before a reset
are dangling after it, exactly as after a `free`.

## Pointer arithmetic

Pointer arithmetic is **element-scaled** and **unchecked**:

```jr
q := p + 3;        // advance by 3 elements (3 * size_of(T) bytes)
r := 3 + p;        // same, either operand order
s := q - 1;        // back up one element
d := q - p;        // signed distance: 3 elements
```

`p + n`, `n + p`, `p - n` and same-type `p - q` on a `*T` all use element units. A difference
returns signed `s64`, so `*u8 - *u8` measures bytes while `*s64 - *s64` measures `s64` elements.
The pointers must have one identical type and a non-zero-sized pointee.

`p[n]` index sugar and pointer ordering (`<`, `>`) remain
<span class="jairs-status absent">absent</span>. Arithmetic is unchecked — running past an
allocation, subtracting unrelated pointers, or producing a non-element-aligned/unrepresentable
difference is undefined behaviour.

## Typed allocation

Because `malloc` returns raw `*u8` and you cannot `cast` it to a `*T`, heap storage of a
specific type goes through `typed`/`untyped` (covered in [The type
system](/language/the-type-system/#typed-allocation)):

```jr
data := typed(s64, malloc(n * size_of(s64)));
// … use data as an ordinary *s64 …
free(untyped(data));
```

This is the primitive the [`List`](/language/the-standard-library/) and `Map` modules are
built on. A module that *owns* heap memory — like `List` — names that fact in its type and
gives you a `free_*` to call, because with no destructors, cleanup you do not see is cleanup
that never happens.

## The larger picture

The rule underneath all of this: **memory is a resource you manage, and the language makes the
management visible rather than automatic.** `defer` (from [Control
flow](/language/control-flow/)) pairs a free with its allocation; the context routes the
allocator; `typed` marks where raw bytes become a typed pointer. None of it is hidden, and
none of it is a garbage collector.

Next: [Errors and traps](/language/errors-and-traps/).

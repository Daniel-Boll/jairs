---
title: The implicit context
description: A hidden per-call `context` struct passed by pointer, the leading hidden parameter, and the `#c_call` opt-out.
sidebar:
  order: 42
---

Every ordinary Jairs procedure receives a hidden `context` — a value of a compiler-declared struct,
passed by pointer. A write to one of its fields is visible to callees, because they share the same
object. This page exercises that ABI: who sees whose writes, that `main`'s context is zeroed, that
declared arguments still land correctly alongside the hidden one, and that `#c_call` opts out
entirely.

```jr
#import "Basic";

/// Reads the context's one field.
read_allocator :: () -> s64 {
    return context.allocator_data;
}

/// A declared argument *and* the context, so the leading hidden parameter
/// does not push `n` to the wrong position.
add_ctx :: (n: s64) -> s64 {
    return n + 100;
}

/// Sets the field and calls a reader, which must see the new value.
bumped :: (v: s64) -> s64 {
    context.allocator_data = v;
    return read_allocator();
}

/// A #c_call procedure: no context at all.
raw :: (n: s64) -> s64 #c_call {
    return n * 2;
}

main :: () {
    n := 0;

    // main's context is zeroed, so its callees read 0 until something writes.
    if read_allocator() == 0 {
        n = n + 1;
    }
    if context.allocator_data == 0 {
        n = n + 2;
    }

    // A write here is visible to a callee.
    context.allocator_data = 5;
    if read_allocator() == 5 {
        n = n + 4;
    }

    // `bumped` sets the field on the context it was handed and its callee sees it.
    if bumped(9) == 9 {
        n = n + 8;
    }
    // ...and reads 9 now — the by-pointer semantics, observed from the caller's side.
    if context.allocator_data == 9 {
        n = n + 16;
    }

    // The leading hidden parameter does not disturb a declared argument.
    if add_ctx(23) == 123 {
        n = n + 32;
    }

    // A #c_call procedure runs with no context.
    if raw(21) == 42 {
        n = n + 64;
    }

    if n == 127 {
        exit(0);
    }
    exit(1);
}
```

## Passed by pointer, so writes are shared

The context is one object reached by pointer, not a copy. That is why `context.allocator_data = 5`
in `main` is then read as `5` by `read_allocator`, and why `bumped` — which sets the field on the
context it was handed and calls `read_allocator` — sees the value it just wrote. A by-value context
would make "set the field, then call" silently not work, which would defeat the purpose entirely.

The flip side is that a callee's write is visible back in the caller: after `bumped(9)`,
`context.allocator_data` reads `9` in `main`, because `bumped` modified the very pointer it was
handed. Isolating a callee's writes needs a separate construct, `push_context`, covered on the next
page.

## `main` starts zeroed

`main` has no Jairs caller, so the entry stub creates its context and zeroes it. Reading
`context.allocator_data` before anything writes yields `0` — a defined value, not garbage. This is
what lets a program rely on an uninstalled allocator field reading null rather than pointing
somewhere arbitrary.

## The hidden parameter is leading

The context is a *leading* hidden parameter, inserted before the declared ones. `add_ctx` takes both
a context and a declared `n`, and `add_ctx(23)` returns `123` — proving `n` landed on the right
parameter. If the hidden pointer had displaced the declared arguments, this would have added the
context pointer to `n` instead of `100`.

## The `#c_call` opt-out

A procedure marked `#c_call` receives no context. `raw` is declared `(n: s64) -> s64 #c_call` and
runs fine — `raw(21) == 42` — which is what proves the opt-out is real rather than cosmetic. This
is how a Jairs procedure can be handed to C code that knows nothing about the hidden parameter.

## A note on this file's history

The `context` struct originally had a single placeholder field. When the real allocator protocol
landed, this file was *rewritten* rather than extended — a first for the corpus. What it tests is
the ABI (that a context is one object reached by pointer), not what the fields mean, so it now
exercises that through `allocator_data`, the field that is still a plain `s64`. The protocol itself
lives on the [allocator page](/by-example/041-allocators/).

## Teeth

Each assertion contributes a bit to `n`. `127` means every read saw the write it should have and no
write leaked where it should not. The `exit` makes that observable, so the two engines can be
asserted to agree about a value that only exists because a pointer was threaded through every call
correctly.

See also [Book I — The Jairs Language](/language/introduction/).

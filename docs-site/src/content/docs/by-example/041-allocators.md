---
title: The allocator protocol
description: An allocator as a pair of procedure pointers plus a state word, installed in the context and reached from any callee.
sidebar:
  order: 41
---

An allocator in Jairs is not a magic built-in — it is data a program installs in the context: two
procedure pointers (`allocator`, `allocator_free`) and a state word (`allocator_data`). Any callee
can allocate by reading `context.allocator` and calling through it, without ever knowing what was
installed. This page installs two different allocators and proves the switch is honoured on every
call.

```jr
#import "Basic";

/// The allocate half. A wrapper around libc malloc, because a #foreign
/// procedure cannot fill a procedure-pointer field.
libc_alloc :: (n: s64) -> *u8 {
    return malloc(n);
}

/// The release half, returning nothing.
libc_free :: (p: *u8) {
    free(p);
}

/// A second allocator that records how many bytes it was asked for.
counting_alloc :: (n: s64) -> *u8 {
    context.allocator_data = context.allocator_data + n;
    return malloc(n);
}

/// Allocates without knowing which allocator it is using.
alloc_through_context :: (n: s64) -> *u8 {
    return context.allocator(n);
}

/// Releases through the context, likewise.
free_through_context :: (p: *u8) {
    context.allocator_free(p);
}

main :: () {
    n := 0;

    // Nothing installed yet: the context is zeroed, so the allocator is null.
    if context.allocator_data == 0 {
        n = n + 1;
    }

    // Install the libc wrapper.
    context.allocator = libc_alloc;
    context.allocator_free = libc_free;

    // A callee allocates through the context without knowing what is installed.
    p := alloc_through_context(24);
    if p == null {
        exit(1);
    }
    n = n + 2;

    p.* = 99;
    if p.* == 99 {
        n = n + 4;
    }

    free_through_context(p);
    n = n + 8;

    // Swap in the counting allocator.
    context.allocator = counting_alloc;
    q := alloc_through_context(16);
    if q == null {
        exit(1);
    }
    if context.allocator_data == 16 {
        n = n + 16;
    }

    r := alloc_through_context(8);
    if r == null {
        exit(1);
    }
    if context.allocator_data == 24 {
        n = n + 32;
    }

    free_through_context(q);
    free_through_context(r);
    n = n + 64;

    if n == 127 {
        exit(0);
    }
    exit(1);
}
```

## An allocator travels with the call

`alloc_through_context` is the procedure that proves the protocol. It never sees the installation
in `main`; it just reads `context.allocator` and calls it. Because the context is threaded through
every call, an allocator installed at the top is reachable from any callee — that is the entire
argument for having a context.

The state word travels the same way. `counting_alloc` writes `context.allocator_data`, and `main`
reads the accumulated total back afterwards. A callee reads what its caller wrote and can write
what its caller will read, which is what makes a stateful allocator (a bump arena, say) possible at
all.

## The field is read at every call

Installing `counting_alloc` over `libc_alloc` and then allocating twice moves `allocator_data` from
`0` to `16` to `24`. That the total accumulates proves two things: the swap took effect (the new
allocator ran), and the field is genuinely re-read on each call rather than folded once at install
time.

## Why a wrapper, not `malloc` directly

`libc_alloc` and `libc_free` are one-line wrappers, and they have to be. A `#foreign` procedure
*cannot* be installed into a procedure-pointer field directly — `context.allocator = malloc` is a
diagnostic (E0256). A foreign procedure's type carries a C-call convention, while a procedure-pointer
field's type is always the ordinary Jairs convention, so the assignment does not typecheck. The
wrapper is the required shape, and it is what a reader should copy.

## Void-returning procedure pointers

`allocator_free` is a procedure pointer that returns nothing. Its field type is written `(*u8)`
with no arrow — the void-returning pointer form. This was unspellable before this feature landed:
`-> void` is refused because `void` has no type name, and `(*u8)` alone used to demand an arrow.
`libc_free` matches it — a procedure declared `(p: *u8) { … }` with no return.

## Teeth

Both engines allocate from different sources — the VM from its own region, native from libc — so
the addresses differ, but the bytes written and read back do not. Each assertion adds a bit to `n`;
`127` means the protocol, the swap, and the state word all held. A wrong allocator, a stale field
read, or a lost state word lands on a different exit status.

See also [Book I — The Jairs Language](/language/introduction/).

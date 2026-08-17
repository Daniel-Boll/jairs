---
title: null & a memory source
description: The `null` keyword taking its type from context, and getting real writable memory from libc's `malloc`.
sidebar:
  order: 40
---

Before a program can write an allocator it needs two things the compiler was refusing: a way to
spell a null pointer, and a way to get memory. This page shows both — the `null` keyword and a
call to libc's `malloc` — and proves the memory that comes back is real by writing a byte and
reading it back.

```jr
#import "Basic";

main :: () {
    n := 0;

    // A null pointer, written with the keyword rather than a cast.
    p: *u8 = null;
    if p == null {
        n = n + 1;
    }
    if p == null {
        n = n + 2;
    }

    // Real memory from libc. Its address is undefined, but that it is *not* null is not.
    p = malloc(16);
    if p == null {
        exit(1);
    }
    n = n + 4;

    // A byte written through the pointer reads back.
    p.* = 42;
    if p.* == 42 {
        n = n + 8;
    }
    p.* = 200;
    if p.* == 200 {
        n = n + 16;
    }

    // Release it, then a defined no-op free of a null pointer.
    free(p);
    nothing: *u8 = null;
    free(nothing);
    n = n + 32;

    // A second allocation, to prove malloc works more than once.
    q: *u8 = malloc(1);
    if q == null {
        exit(1);
    }
    q.* = 9;
    if q.* == 9 {
        n = n + 64;
    }
    free(q);

    if n == 127 {
        exit(0);
    }
    exit(1);
}
```

## `null` takes its type from context

`null` is a keyword, not a value with a type of its own. Like an integer literal, it takes its
type from the surrounding context: in `p: *u8 = null` the annotation `*u8` supplies the type. A
bare `null` with nowhere to draw a type from is a diagnostic (E0257), so there is exactly one way
to write a null pointer of a given kind. A `cast(*u8, 0)` is deliberately not that way — it is
still refused.

Notice the second `free` uses a typed local, `nothing: *u8 = null`, rather than `free(null)`
directly. A bare `null` argument would try to take its type from the callee's parameter, and the
semantic tests exercise these files with imports left unresolved on purpose — so a context-free
`null` there would be the E0257 case. Binding it to a typed local first sidesteps that while still
handing `free` a genuine null pointer.

## The sentinel that equals itself

The whole point of a null pointer is that it is a sentinel a program can *test*. Two null pointers
compare equal, so `p == null` is true while nothing has been allocated. After a real allocation
`p == null` becomes false — and that is the observable fact, not the address itself, which is
undefined. The engines are compared on null-ness, never on which number the OS happened to return.

## Memory that reads back

`malloc(16)` returns a `*u8` to sixteen bytes. Writing through it with `p.* = 42` and reading
`p.* == 42` back proves the storage is real and writable without depending on the address.
Overwriting with `200` and re-reading proves the location is stable rather than read-once. The
write goes through `p.*` — the pointer dereference — because indexing a bare pointer is pointer
arithmetic, which is a later feature; one byte at the front is enough to prove the storage exists.

`free(null)` is defined as a no-op, so a program can release unconditionally without first checking
for null. The second allocation, write, and free show `malloc` works more than once and that the
first `free` did not corrupt the allocator.

## What is deliberately absent

`malloc` reaches libc directly. In the bytecode VM this happens through libffi in runtime mode, so
the pointer there is a genuine host address too — one of the very few places a real host pointer is
legitimate in the VM. A comptime `#run malloc(…)` would *not* work and is deliberately absent: the
language gates FFI at compile time.

## Teeth

The `exit` at the end is what makes the file observable. Each passing assertion adds a distinct bit
to `n`, so all seven summing to `127` means everything held; a wrong answer about null-ness or a
byte that did not survive the round-trip lands on a different `n`, and thus a different exit status.
Running the program under `jr run` and `jr build` and asserting the two exit codes agree turns
those bits into a test.

See also [Book I — The Jairs Language](/language/introduction/).

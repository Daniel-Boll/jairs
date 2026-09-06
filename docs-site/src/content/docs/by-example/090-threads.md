---
title: "Threads: spawn, join, spin lock"
description: "A pthread_create binding, a #c_call thread body with no context and no allocator, and a spin lock built from atomic_compare_exchange because pthread_mutex_t has no spellable layout."
sidebar:
  order: 90
---

`Thread` is a binding, not a runtime: `spawn`, `join`, `joinable`, `yield_now`, and a spin lock built on
the atomics rather than on `pthread_mutex_t` (ADR-0175, ADR-0177). It closed W11, the last of the twelve
waves, and it needed one language feature first — a *spellable* `#c_call` procedure type — because a
thread body is handed to C and nothing in this language could name that shape until then.

## What a thread body must be

```jr
libc :: #system_library "c";

/// `pthread_create`. The handle is written through `thread`.
///
/// `start` is spelled with its convention (ADR-0175 §1). Without that, a Jairs procedure could not be named
/// here at all: its type would be the context-taking one and no coercion exists between the two.
pthread_create :: (thread: *u8, attr: *u8, start: (*u8) -> *u8 #c_call, arg: *u8) -> s64 #foreign libc "pthread_create";

/// `pthread_join`. Takes the handle **by value**, not by pointer.
///
/// Spelled `*u8` because a `pthread_t` *is* a pointer on both supported platforms. Passing the address of the
/// handle instead compiles and returns `EINVAL` at run time, which is how this was got wrong first.
pthread_join :: (thread: *u8, result: *u8) -> s64 #foreign libc "pthread_join";

/// A running thread.
Thread :: struct {
    /// The `pthread_t`. Null before a successful spawn and after a join.
    handle: *u8;
}

/// Starts `body` on a new thread, passing it `argument`.
///
/// `#must`: a spawn fails when the system is out of threads, and a caller who ignores that then *joins* a
/// null handle — which is `EINVAL` reported one step away from its cause.
spawn :: (body: (*u8) -> *u8 #c_call, argument: *u8) -> (Thread, bool) #must { ... }

/// Waits for `thread` to finish.
///
/// Nulls the handle on success, so a second join is refused by this module rather than by libc: joining an
/// already-joined `pthread_t` is undefined behaviour in C, not an error.
join :: (thread: *Thread) -> bool #must { ... }

/// Whether `thread` is still ours to join.
joinable :: (thread: *Thread) -> bool { ... }
```

An ordinary Jairs procedure takes the hidden `context` parameter (ADR-0001), so C calling one would pass
its `argument` where the context belongs. `#c_call` on `body`'s type is what makes the two conventions
distinguishable — before ADR-0175 it could not be *written* in a type at all, so a thread body could be
declared and called directly, but never handed to `pthread_create`. A thread body has this shape:

```jr
bump :: (arg: *u8) -> *u8 #c_call {
    shared := typed(s64, arg);
    i := 0;
    while i < 1000 {
        _ = atomic_add(shared, 1);
        i = i + 1;
    }
    return null;
}
```

`argument` is a `*u8` because C's is. A caller with a typed pointer passes `untyped(p)`, and the body
reads it back with `typed(T, arg)` — the one erasing boundary this language permits and names, rather
than a general pointer cast.

## Sharing a counter without losing an increment

```jr
#import "Basic";
#import "Thread";

bump :: (arg: *u8) -> *u8 #c_call {
    shared := typed(s64, arg);
    i := 0;
    while i < 1000 {
        _ = atomic_add(shared, 1);
        i = i + 1;
    }
    return null;
}

main :: () {
    counter := 0;

    a, aok := spawn(bump, untyped(*counter));
    if !aok { exit(90); }
    b, bok := spawn(bump, untyped(*counter));
    if !bok { exit(91); }
    c, cok := spawn(bump, untyped(*counter));
    if !cok { exit(92); }

    if !join(*a) { exit(93); }
    if !join(*b) { exit(94); }
    if !join(*c) { exit(95); }

    // A second join must be refused by the module rather than by libc, where it is undefined behaviour.
    if join(*a) { exit(96); }

    // 3000 exactly. Less means an increment was lost, which is what a non-atomic add does.
    if counter != 3000 { exit(1); }

    exit(42);
}
```

Three threads each add 1000 to one shared counter through `atomic_add`, and the total is **exactly**
3000, verified across five runs of the compiled binary. This is not a corpus program: the bytecode VM
cannot spawn a thread at all — `pthread_create` needs a machine address for the thread body, and an
interpreter has no machine code to point at — so `Thread`'s concurrency is proved by a `jr-cli`
integration test that builds and runs the real binary, the same split ADR-0158 made for `Process`. The
same program written with `shared.* = shared.* + 1` instead of `atomic_add` was measured at **1000**
on one run of three — two thousand increments silently lost — which is why this file exists rather than
a paragraph promising the number is right.

## The spin lock, because `pthread_mutex_t` has no spellable layout

```jr
/// The value an unlocked spin lock holds.
UNLOCKED :: 0;

/// The value a held spin lock holds.
LOCKED :: 1;

/// Takes the spin lock at `lock`, waiting until it is free.
///
/// **A spin lock, not a mutex**: `pthread_mutex_t`'s layout is opaque and platform-sized. This burns CPU
/// while contended and yields on every failed attempt, which is what keeps a single-core machine from
/// live-locking — a bare spin with no yield deadlocks there whenever the holder is descheduled.
///
/// Not `#must`: it does not fail, it waits.
acquire :: (lock: *s64) {
    while !atomic_compare_exchange(lock, UNLOCKED, LOCKED) {
        yield_now();
    }
}

/// Releases the spin lock at `lock`.
///
/// An atomic **store** rather than a compare-exchange: only the holder releases, so there is nothing to
/// check, and a store is the cheaper of the two.
release :: (lock: *s64) {
    atomic_store(lock, UNLOCKED);
}

/// Whether the lock at `lock` is currently held.
///
/// For a DIAGNOSTIC, never for a decision — by the time a caller reads the answer it may be stale.
is_locked :: (lock: *s64) -> bool {
    return atomic_load(lock) == LOCKED;
}
```

`lock` is a bare `*s64`, not a struct wrapping one: a one-field struct would need `typed`/`untyped` at
every call site and buy nothing. `pthread_mutex_t` is 64 opaque bytes on macOS and a different size on
Linux, and this language has no way to say "an opaque N-byte thing whose N I looked up" without
hard-coding N per platform — the same wall `Socket`'s `sockaddr_in` hit and lost more of. So the offered
substitute burns CPU while contended, and the module says so rather than papering over it: a caller
holding a lock across a syscall wants a real mutex.

## What a thread body cannot do

A spawned body has **no context**, so it cannot allocate: `context.allocator` is exactly what `#c_call`
opts out of. A worker that needs memory takes it through `arg`, which is the discipline C imposes anyway.

A spawned body cannot trap usefully either. The shadow call stack a backtrace walks is one module-wide
object with one depth counter, and two threads pushing onto it race — a trap in a spawned thread still
*stops the program*, but the message may name the wrong frames. Making the stack per-thread needs
thread-local storage in both native back ends, which is not built.

## What is absent, and why

There is <span class="jairs-status absent">absent</span> `Mutex`, for the layout reason above, and <span
class="jairs-status absent">absent</span> everything a real concurrency runtime has beyond a spawn, a
join and a spin lock: no channel, no thread pool, no `Thread_Local` storage class, and no fence. Each
wants a caller first, and none is needed for a counter, a lock, or a spawn. `atomic_add` and friends are
language intrinsics rather than procedures in this module — see [Atomics](/by-example/091-atomics/) — so
a program that shares one counter between two threads it did not spawn still has them without importing
`Thread` at all.

See also [Book I — The Jairs Language](/language/introduction/).

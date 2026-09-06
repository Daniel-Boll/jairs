---
title: Concurrency
description: Threads, atomics, the memory model, and what a thread body cannot do.
sidebar:
  order: 19
---

Jairs has real OS threads, real atomics, and a spin lock — and it draws a hard line, stated
rather than hidden, around what those three do not give you.

## Threads

`modules/Thread` is a thin binding over `pthread_create`/`pthread_join`, not a runtime: no
thread pool, no channel, no condition variable.

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
    // counter is now 3000
}
```

`spawn(body, argument)` starts `body` on a new thread and hands it `argument`, answering
`(Thread, bool) #must` — a caller who ignores a failed spawn and then joins a null handle sees
`EINVAL` one step from the real cause, which is what `#must` exists to close off. `join`
answers `bool #must` too, and it **nulls the handle on success**, so a second `join` on the
same `Thread` is refused by this module rather than being undefined behaviour in libc.
`joinable(thread)` asks whether a thread is still *ours to join* — not whether it is still
running, since a finished-but-unjoined thread is still joinable and answering otherwise would
invite a caller to leak it. `yield_now()` yields the processor.

A thread body's type is exactly `(*u8) -> *u8 #c_call`, because that is `pthread_create`'s own
signature. That is why `#c_call` procedure-pointer types matter beyond the FFI chapter — a
thread body could not be written in Jairs at all without one to spell it.

## Atomics

`atomic_load`, `atomic_store`, `atomic_add` and `atomic_compare_exchange` are **language
intrinsics**, not procedures in `modules/Thread` — you can use them without importing it at
all:

```jr
counter: s64;
_ = atomic_add(*counter, 1);          // adds 1, yields the value *before* the add
ok := atomic_compare_exchange(*counter, 1, 2);   // installs 2 if counter held 1
```

They are intrinsics rather than library calls because they must lower to a real machine
instruction in three engines, and no library call can do that. Every atomic here is on an
`s64`, and every one is **sequentially consistent** — there is no way to ask for a weaker
ordering. `atomic_add` **wraps** on overflow rather than trapping, matching the hardware: an
atomic add is one instruction with no overflow check, and trapping in the VM's interpreter
would make it disagree with what the native back ends actually execute.

The three thousand increments in the first example are the whole point of using `atomic_add`
rather than `shared.* = shared.* + 1`: the same three-thread program with a plain
read-modify-write measured **1000** instead of 3000 on one real run — two thousand increments
silently lost to a race. An atomic is also never moved, duplicated, or removed by any
optimisation pass, and a plain access racing a write anywhere is a data race with no defined
outcome, exactly as in C.

## The spin lock

There is no `Mutex`. `pthread_mutex_t` is 64 opaque bytes of platform layout on macOS and a
different size on Linux, and Jairs has no way to say "an opaque N-byte thing whose size I
looked up" without hard-coding that number per platform — the same wall `Socket`'s
`sockaddr_in` hit. So the substitute is a spin lock over a bare `*s64`, built on
`atomic_compare_exchange`:

```jr
lock := UNLOCKED;
acquire(*lock);      // spins on atomic_compare_exchange, yielding on every failed attempt
// ... the critical section ...
release(*lock);       // an atomic STORE — only the holder releases
held := is_locked(*lock);   // a diagnostic only; stale by the time you read it
```

`acquire`, `release` and `is_locked` take a bare `*s64` rather than a one-field lock struct,
because a struct would need `typed`/`untyped` at every call site for no benefit. Stated
plainly rather than papered over: `acquire` **burns CPU while contended**. A caller holding
this lock across a syscall wants a real mutex and should say so, because this is not one.

## Three consequences a caller must know

**A `#c_call` thread body has no context, so it cannot allocate.** `context.allocator` is
exactly what `#c_call` opts out of — that is not incidental, it is the reason `#c_call`
procedure types had to exist before `Thread` could. A worker that needs memory takes it
through `arg`, already allocated by whoever spawned it.

**A trap in a spawned thread may name the wrong frames.** The shadow call stack a backtrace
walks is one module-wide object with a single depth counter, and two threads pushing onto it
race. A trap still *stops the program* — it does not become silent or wrong in its effect —
but the frame list printed alongside it may not be the thread's own. Making the stack
per-thread needs thread-local storage in every back end, which does not exist yet.

**A module's own state is not thread-local.** Jairs has no `#add_context` — no way for a
module to declare a per-thread slot the way `context` itself is threaded through ordinary
calls — so a file-scope global two threads both reach is shared, not duplicated per thread.
`modules/Basic`'s own output buffer is exactly this: a file-scope global, and its own doc
comment says plainly that it is therefore not thread-safe — two threads printing at once
interleave inside one buffer, and the fix is `#add_context`, which does not exist yet.

Next: [Tooling](/language/tooling/), the compiler driver, the language server, and the
formatter.

---
title: Atomics
description: Four sequentially-consistent intrinsics — atomic_load, atomic_store, atomic_add, atomic_compare_exchange — that no compiler pass may move, duplicate or elide.
sidebar:
  order: 91
---

Atomics are **language intrinsics**, not procedures in any module: `atomic_load`, `atomic_store`,
`atomic_add` and `atomic_compare_exchange` lower to one machine instruction each in three engines, and no
library call can do that (ADR-0176). They are usable in any file that imports nothing at all — a program
sharing a counter between two threads has them without ever importing `Thread`.

## Load, store, add, compare-exchange

```jr
#import "Basic";

main :: () {
    total := 0;
    counter := 10;

    if atomic_load(*counter) == 10 {
        total = total + 1;
    }

    atomic_store(*counter, 20);
    if atomic_load(*counter) == 20 {
        total = total + 2;
    }

    // The *previous* value. A caller wanting the new one adds again; a caller wanting a distinct ticket per
    // thread needs exactly this.
    if atomic_add(*counter, 5) == 20 {
        total = total + 4;
    }
    if atomic_load(*counter) == 25 {
        total = total + 8;
    }

    if atomic_compare_exchange(*counter, 25, 99) {
        total = total + 16;
    }
    if atomic_load(*counter) == 99 {
        total = total + 32;
    }

    // A failed exchange must not write. This is the assertion that catches a compare-exchange lowered as an
    // unconditional swap, which would pass every positive check above.
    if !atomic_compare_exchange(*counter, 25, 7) {
        total = total + 64;
    }
    if atomic_load(*counter) == 99 {
        total = total + 128;
    }

    // **Wrapping**, not trapping: an atomic add is one machine instruction with no overflow check, so an
    // interpreter that trapped here would disagree with both back ends about a legal program.
    edge := 9223372036854775807;
    _ = atomic_add(*edge, 1);
    if atomic_load(*edge) == -9223372036854775808 {
        total = total + 256;
    }

    exit(total % 251);
}
```

This is `tests/corpus/valid/132-atomics.jr`'s first nine of eleven checks, verified separately: run alone
trailing `exit(total % 251)` shown above, this excerpt exits **9**. It is deliberately
**single-threaded**: the bytecode VM cannot spawn a thread at all, so the concurrency proof lives in a
`jr-cli` integration test (see [Threads](/by-example/090-threads/)), while this file is the *evaluation*
proof that all three engines — the interpreter, Cranelift's `atomic_rmw`/`atomic_cas`, and LLVM's
`atomicrmw`/`cmpxchg` — compute the same answer. The full corpus file adds a signed compare-exchange
against a negative value and exits **39**.

`atomic_add` returns the value **before** the addition, which is what makes it a ticket dispenser: two
threads calling it on the same counter get two distinct numbers back, one each. Returning the new value
instead would make that use impossible to write correctly, since a caller could no longer tell which
ticket was theirs after another thread's add landed between the addition and the read.

`atomic_compare_exchange` is the **strong** form and returns a `bool`: `true` when it swapped. A weak
version that can fail spuriously is a trap for a caller who does not expect it, and the value the compare
*found* is deliberately not returned — a caller who wants it can `atomic_load`, and a two-result
intrinsic would make the common case pay for the rare one.

## `s64` only, sequentially consistent, and never moved

Every atomic here takes a `*s64`; a `*u8` or a `*Point` is E0214, the ordinary mismatched-types error —
atomics take no width parameter, so `check_expr` reports the mismatch before any atomic-specific check
runs. A width parameter would mean deciding what an atomic `u8` means on a machine whose smallest atomic
is a word, and whether a struct may be exchanged — both real decisions this wave did not make, so the
type checker names the boundary rather than letting a caller find it by surprise.

Every operation is **sequentially consistent**, with no way to ask for less: `MemFlags::trusted()` in
Cranelift, `SequentiallyConsistent` in LLVM. Offering a weaker ordering before the memory model was
written down would be selling a guarantee nobody had described (ADR-0176 §3).

**An atomic is never moved, duplicated or removed by any compiler pass.** Every mid-end pass in this
project was written for a single-threaded program before atomics existed, and each was wrong about one
in its own way: a store-forwarding pass could have moved a plain write *across* an atomic, reordering it
past a synchronisation point; constant propagation could have folded a load of a location another thread
writes; dead-code elimination could have deleted a compare-exchange whose result nobody reads — deleting
the lock while keeping the critical section. `Rvalue::Atomic` is its own MIR variant precisely so every
one of those passes had to answer the question explicitly rather than fall through a wildcard arm.

## What a data race actually costs

There is no operator spelling for an atomic — no `a atomic+= 1`. An operator would make the *ordering*
invisible at the call site: `a += 1` and `atomic_add(*a, 1)` mean very different things to another
thread, and an audit of a concurrent program has to be able to find every synchronising operation by
searching for it.

**A plain access racing any write is a data race with no defined outcome, exactly as in C — and this has
been measured, not merely stated.** The three-thread counter from the [Threads](/by-example/090-threads/)
page, rewritten with `shared.* = shared.* + 1` in place of `atomic_add`, produced **1000 instead of
3000** on one run of three: two thousand increments silently lost, no diagnostic. A memory model whose
data-race clause has been *observed* is worth more than one that has only been promised (ADR-0177 §3).

`join` is the only ordering edge between threads this library provides: a value written before a thread
ends is visible after a successful `join` on it.

## What is absent, and why

There are <span class="jairs-status absent">absent</span> `atomic_sub`, `atomic_and`, `atomic_or`,
`atomic_xor` and `atomic_exchange` (an unconditional swap, as distinct from `atomic_compare_exchange`).
None is a harder feature to build than the four that exist — each is mechanical once load, store, add and
compare-exchange work — they simply have no caller yet: a counter needs `add`, a flag needs
`load`/`store`, a lock needs `compare_exchange`, and nothing built so far has needed the rest. There is
also <span class="jairs-status absent">absent</span> a weaker memory ordering (`relaxed`, `acquire`,
`release`) and <span class="jairs-status absent">absent</span> a standalone fence — both real features
whose absence is a decision not yet made rather than an oversight.

See also [Book I — The Jairs Language](/language/introduction/).

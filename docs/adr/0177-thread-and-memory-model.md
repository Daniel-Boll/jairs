# ADR-0177: `Thread`, the memory model, and W11 closed

- **Status:** Accepted
- **Date:** 2026-09-02
- **Deciders:** dboll
- **Closes W11 — Concurrency**, the last wave of the twelve. Built on ADR-0175's `#c_call` procedure type and
  ADR-0176's atomics.
- No design fork was put to the decider; the recommended approach was taken and the rejected
  alternatives are recorded below, per the session directive.

## Decision

### 1. `modules/Thread` is a binding, not a runtime

`spawn`, `join`, `joinable`, `yield_now`, and the spin lock `acquire`/`release`/`is_locked`. Nineteen lines of
`#foreign` and about sixty of Jairs.

**A thread body is a `#c_call` procedure taking a `*u8` and returning a `*u8`**, because that is
`pthread_create`'s signature. **Rejected: wrapping it in a Jairs-shaped `spawn(f: () -> void)`** with a
generated trampoline — the trampoline needs a `#c_call` procedure to *be*, which is the same requirement one
level down, plus a closure mechanism this language does not have.

**`#c_call` is load-bearing, not incidental.** An ordinary Jairs procedure takes the hidden context (ADR-0001),
so C calling one passes `arg` where the context belongs. That is why ADR-0175 had to come first.

**`pthread_join` takes the handle by value, not by pointer** — got wrong first, and it returned `EINVAL` rather
than crashing, which is the kind of mistake a type system cannot catch when both spellings are `*u8`.

`join` nulls the handle on success, so a **second join is refused by this module** rather than by libc, where
joining an already-joined `pthread_t` is undefined behaviour rather than an error. The test asserts the refusal.

### 2. A spin lock, because `pthread_mutex_t` has no spellable layout

`pthread_mutex_t` is 64 opaque bytes on macOS and a different size on Linux, and this language cannot say "an
opaque N-byte thing whose N I looked up" without hard-coding N per platform — the wall `Socket`'s `sockaddr_in`
hit (ADR-0158 §4) and lost more of.

So `acquire` spins on `atomic_compare_exchange` and **yields on every failed attempt**. The yield is not
politeness: a bare spin dead-locks on a single core whenever the holder is descheduled.

**Rejected: hard-coding the mutex size per platform.** It works until a libc update changes it, and then it
corrupts the stack — no diagnostic, no crash at the wrong line. **Rejected: no lock at all.** Atomics alone
cannot protect a multi-word invariant, and a caller would build a worse lock.

Stated plainly in the module docs: it burns CPU while contended, and a caller holding a lock across a syscall
wants a real mutex.

### 3. The memory model, written down because now something depends on it

- Every atomic is **sequentially consistent**. There is no way to ask for less, deliberately (ADR-0176 §3).
- An atomic is **never moved, duplicated or removed** by any pass in the mid-end (ADR-0176 §2).
- A **plain access racing any write is a data race with no defined outcome**, exactly as in C.
- A thread body has **no context**, so it cannot allocate (ADR-0057 §3). Memory arrives through `arg`.
- `join` is the only ordering edge between threads that this library provides. A value written before a thread
  ends is visible after its `join`.

**The third point is measured, not asserted**: the same three-thread program with `shared.* = shared.* + 1`
produced **1000 instead of 3000** on one run of three. A memory model whose data-race clause has been *observed*
is worth more than one that has been promised.

### 4. What W11 does not deliver, and why each is separate

**A per-thread shadow call stack.** §8.3 named "a per-thread stack in the runtime" as W11 work, and it is not
done. The shadow stack a backtrace walks is one module-wide object with one depth counter (ADR-0066 §1), so two
threads pushing onto it race — a trap in a spawned thread still *stops the program*, and may name the wrong
frames.

**Rejected: doing it here.** Thread-local storage needs a mechanism in both back ends (Cranelift's `TlsValue`,
LLVM's `thread_local` globals) plus a decision about the model, and it changes the trap path every existing
program uses. That is a wave, and bundling it would have meant shipping neither cleanly. **Rejected: leaving it
unsaid** — a wrong backtrace with no note is a bug report nobody can act on, so the module docs say it.

**A `Thread_Local` storage class**, a **channel**, a **thread pool**, and a **fence**. Each wants a caller, and
none is needed for a counter, a lock, or a spawn.

### 5. Why the concurrency test is not a corpus program

The corpus differential asserts that the bytecode VM and both native back ends agree, and the VM **cannot spawn
a thread** — ADR-0175 §4, and it is a property of interpreting rather than a missing wave.

So the split is: `valid/132-atomics.jr` proves all three engines *evaluate* atomics identically, and a `jr-cli`
integration test proves they *synchronise*. Same call ADR-0158 §3 made for `Process`.

**The test runs the binary five times**, because a lost increment is a race and a race that fails one run in
three passes one run in three. One run proving nothing is how a concurrency test becomes decoration.

## Consequences

- **W11 — Concurrency is DONE**, and with it **all twelve waves**. `Thread` is the twentieth module.
- **1069 tests** (1073 under gate 7), **255 corpus files**, 20 modules, 177 ADRs.
- **Three threads, 3000 atomic increments, exactly 3000**, five runs per invocation, in both native back ends.
- **The memory model is written down** and its data-race clause is measured.
- **Owed and named**: a per-thread shadow stack for correct backtraces under threads; a `Thread_Local` storage
  class; channels; a thread pool; a fence; and W12's remaining register-resident locals.
- **Three tooling traps fired on this wave's own files**, each caught by the gate that exists for it: the
  formatter dropped `#c_call` from a type (gate 5), the tree-sitter grammar reported an `ERROR` node over it
  (gate 6), and `codes.rs` caught a code collision (gate 3). None reached a commit.

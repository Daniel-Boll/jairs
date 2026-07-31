# ADR-0061: The VM satisfies `malloc`/`free` from its own linear region — correcting ADR-0060 §4

- **Status:** Accepted
- **Date:** 2026-07-31
- **Corrects ADR-0060 §4**, which asserted that the VM dereferences a `malloc`'d pointer through
  libffi "the pointer is a genuine host address there too". Running it disproved that: it faults.
  This ADR records why, and what the VM does instead. It is the same-wave correction ADR-0056 was to
  a claim about what compiles — a decision written from reasoning rather than from running, caught by
  running.
- **Still the fourth feature of W3**, part of the `null`-and-memory wave.

## Context

ADR-0060 §4 said the corpus's byte round-trip — `p = malloc(16); p.* = 42; p.* == 42` — would work
in the VM because libffi hands `malloc`'s real host address back and the VM dereferences it. **It
does not.** The VM exits with `invalid access of 1 bytes at address 0x873140010` on the write, while
the native binary computes the answer.

The reason is the VM's memory model, which its own module docs state plainly:

- **The VM's memory is one non-moving linear region**, wasmtime-style. A Jairs pointer is an
  **offset** into it, and every dereference is `base + offset` with a **bounds check** — the check
  that makes the VM a sandbox, turning a bad pointer into a diagnosable `Trap::BadAddress` rather
  than a host segfault.
- **The translation is one-way.** `write(s.data, …)` works because `s.data` is a VM offset
  translated *to* a host address for the duration of the FFI call (`host_pointer`). There is no
  inverse: a raw host address from libc `malloc` is not an offset into the VM's region, so the
  bounds check rejects it — correctly, because the VM has no idea whether that address is valid.
- **So ADR-0060 §4 was wrong about the direction.** A VM offset can become a host pointer for a
  call; a host pointer cannot become a VM offset for a dereference. `malloc`'s return is the second
  thing, and the VM cannot use it.

This was found by running the corpus file the same wave's ADR-0060 §4 promised it would pass. The
promise was written from the model's *intent* — a bridge that hands real addresses to C — without
checking the one operation the corpus needed, which is the *inverse*.

## Decision

### 1. In the VM, `malloc` allocates from the VM's own region; `free` is a no-op

`jr-vm`'s FFI bridge intercepts a foreign call whose symbol is `malloc` (with a pointer return) or
`free` (returning `void`), *before* marshalling arguments to libffi:

- **`malloc(size)`** calls `Memory::allocate(size, 16)` — the VM's own bump allocator, the same one
  that lays out string constants and call frames — and returns the resulting **offset** as the
  pointer value. That offset *is* a Jairs pointer the VM can dereference and bounds-check, so
  `p.* = 42` works.
- **`free(p)`** does nothing. The region is bump-allocated with no reclamation, exactly as call
  frames are (its module docs: "a stack mark, not a free list"). `free(null)` lands here too and is
  the same no-op libc guarantees.

The native back end is unchanged: it calls libc `malloc` and `free` for real host addresses.

**The two engines' pointer bits differ, and that is ADR-0060 §4's decision standing** — nothing
observes a pointer's bits, only calling or dereferencing through it. The differential harness
compares the byte round-trip and the null-ness tests, which agree; it never compares the address,
which is undefined in both engines for a different reason.

**Guarded on the return type.** A `#foreign` procedure a program happens to name `malloc` but that
returns a non-pointer is *not* rerouted — `is_pointer_return` checks the declared type first, so the
interception fires only for the real shape.

### 2. Comptime `malloc` stays refused, by the existing gate

The interception is in `ffi::call`, which the interpreter reaches only *after* `foreign()`'s
`Mode::Comptime` check has already refused a comptime foreign call (ADR-0006). So a `#run malloc(…)`
still fails with ADR-0006's message, unchanged — the interception is a runtime-only reroute, not a
new comptime allowance. Verified by running a same-file `#run malloc`, which still reports E0230.

**Rejected: mapping host pointers in the VM.** Track which pointers are host addresses and route
their dereferences to the host. It would make VM `malloc` a real libc call, matching native's bits.
Rejected because it punches a hole in the sandbox the linear-memory model exists to provide: a bad
pointer becomes a host segfault instead of a `Trap::BadAddress`, which is precisely what wasmtime's
model — and this one — is chosen to prevent. The VM's whole bargain is that a bug is diagnosable, and
a raw host pointer it must trust breaks that.

**Rejected: scoping the corpus down** so the VM never dereferences a `malloc`'d pointer. It would
leave `malloc` half-usable at runtime in the VM and the differential harness unable to exercise a
write-through — the one thing that proves the memory is real. The interception costs a dozen lines
and makes the feature whole in both engines.

### 3. What this does not change

- **Native is untouched** — real libc `malloc`/`free`.
- **ADR-0060 §1–§3, §5 stand** — `null` as a context-typed literal, the `#foreign` bindings, and the
  absent pointer arithmetic. Only §4's claim about *how the VM dereferences a `malloc`'d pointer* was
  wrong, and this replaces it.
- **The allocator wave is still unblocked**, and now genuinely: a program can allocate, write, read
  and free in *both* engines, which is what an allocator protocol will be built and tested against.

## Consequences

- **`jr-vm` gains `Memory::memory_mut` and two symbol-matched arms in `ffi::call`.** No new type, no
  new instruction — the allocator it uses already existed for frames and constants.
- **The VM's `malloc` leaks within a program**, because `free` reclaims nothing. Bounded by the
  region size, which turns exhaustion into a diagnosable `Exhausted` rather than a fault — the same
  trade the frame allocator already makes, and acceptable because comptime evaluation is short-lived
  and runtime native uses real `free`.
- **A corpus program can now allocate in both engines**, so `049-null-and-malloc.jr` runs to exit 0
  under `jr run` and `jr build` alike, and the differential harness holds them equal.
- **ADR-0060 §4 is superseded on one point** and cited from here, per the project's rule that a
  reversal is a new ADR rather than an edit. The rest of ADR-0060 stands.
- **The lesson is recorded where the failure modes are**: a claim about a runtime behaviour, written
  from a model's intent rather than from running the operation, is exactly the kind the differential
  harness exists to catch — and did, one operation after the ADR asserted it.

# ADR-0113: `Random` is a caller-owned xorshift64 generator — and a `u64` constant needs `#run`

- **Status:** Accepted
- **Date:** 2026-08-04
- **Deciders:** dboll
- **W7 sub-wave 11.** A pure-library generator, and one language gap it surfaced: a `u64`-range named constant
  cannot be declared directly.

## Context

A random generator's whole value is that its sequence is reproducible. Probed first: `u64` xorshift arithmetic
(`^`, `<<`, `>>`, `%`) agrees **bit-for-bit** between the two engines — which a PRNG depends on absolutely, since
a generator whose sequence differed between the comptime VM and native code would fail the differential harness
on its first call.

## Decision

### 1. The caller owns the state

`next(*rng)` takes a `*Random` by pointer, so a sequence is reproducible from its seed and two generators are
independent.

- **Not a hidden global.** A global's state is shared program-wide, so a test cannot get a clean sequence, and
  the usual reason for a global — clock seeding — makes every run differ, the opposite of what a differential
  harness needs.
- **Not the context.** The context is for what a *callee* needs without being handed it (an allocator); a random
  sequence is usually a caller's deliberate possession, and a context-carried one cannot have two independent
  streams in one scope. A caller who wants context-carried randomness puts a `*Random` in their own context.

### 2. xorshift64, because obvious correctness beats better statistics here

Three shift-and-xor steps, **exact and deterministic** — every bit reproducible and identical in both engines.
A higher-quality generator (PCG, xoshiro) is a later decision with a statistical-quality argument; xorshift64 is
the one whose correctness is *obvious*, which is what a standard library's first generator should have. It is
**not** cryptographically secure, and the module says so — a library that let a caller mistake it for one would
be doing harm.

**A zero seed is replaced by `GOLDEN`, not rejected.** xorshift is stuck at its zero fixed point, so `seed(rng,
0)` would give a stream of zeros a caller might take a while to notice; substituting a defined non-degenerate
sequence is kinder than a trap for what is usually an uninitialised variable.

`below` is **half-open** (`[low, high)`), matching every other range in the library, and returns `low` for an
empty range rather than trapping — the `clamp`/`substring` reasoning. The modulo bias is named in the docs and
declined as a reason to complicate the first version.

### 3. A `u64`-range named constant needs `#run` — a real gap, with a clean workaround

`GOLDEN`'s value (the golden-ratio fraction, `11400714819323198485`) **exceeds `s64`**, and a bare
`GOLDEN :: <literal>` has no type context, so it defaults to `s64` and does not fit (E0204). There is **no
`name : u64 : value` form** — `parse` rejects the second `:`. So a `u64`-range constant is spelled by
`#run golden_seed()`, a `-> u64` procedure whose return type gives the literal its context.

That is a genuine language gap, recorded rather than worked around silently: a typed constant declaration
(`name : T : value`) is the clean fix and is its own decision. Until then, `#run` of a typed procedure is the
idiom, and it is worth a reader knowing it — an unsigned constant near the top of the range is a normal thing to
want (a hash seed, a bit mask, a sentinel).

## Consequences

- **A deterministic, reproducible generator exists**, and `valid/092` pins the property that matters most:
  the same seed gives the same first value, in both engines. Every one of its eight bits depends on the
  generator computing an identical value in the VM and native code.
- **No new diagnostic code, no compiler change** — the third consecutive pure-library sub-wave (after `String`'s
  allocating half and `Math`).
- **A language gap is on record**: no `name : T : value`, so a `u64`-range constant needs `#run`. That is the
  fourth thing writing the standard library has surfaced that the compiler side had not (after two leaked ICEs,
  the module-diagnostics gap, and FFI floats), which is the argument for a stdlib in the language continuing to
  pay out.
- **Deferred with reasons**: a float in `[0, 1)` (wants a float from a `u64`'s bits — a divide, exact, so
  additive); a better generator (statistical-quality decision); clock seeding (the non-deterministic default the
  explicit struct exists to avoid); a typed constant declaration (its own language decision).

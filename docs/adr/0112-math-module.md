# ADR-0112: `Math` ships the exact closed-form functions, because a float cannot cross the FFI boundary

- **Status:** Accepted
- **Date:** 2026-08-04
- **Deciders:** dboll
- **W7 sub-wave 10.** A module whose shape a probe decided: the obvious `Math` was not writable, and what is
  writable is worth having on its own.

## Context

The natural `Math` wraps libm — `sqrt`, `sin`, `pow` as `#foreign` declarations. Probed first, and it does not
compile: **a float cannot cross the FFI boundary yet**. `sqrt :: (x: float64) -> float64 #foreign libc "sqrt"`
is refused, "passing FloatType to a foreign procedure arrives with a later wave". So libm is unreachable and the
module must be pure Jairs.

## Decision

### 1. The exact, closed-form functions only

`abs`, `fabs`, `min`, `max`, `sign`, `clamp`, `pow` (integer), `gcd`, and `floor`/`ceil`/`round` on a `float64`.
Every one is expressible with arithmetic and comparison the language already has, **exactly** — so both engines
compute identical bits and the differential harness holds them there.

**A transcendental approximated in Jairs was rejected, not deferred by omission.** An approximation's last bits
depend on the order the arithmetic is evaluated, and the comptime VM and native Cranelift may round a fused
multiply-add differently — so the two engines could disagree on the last ulp, which is the one thing this
project's harness treats as a failure. A transcendental belongs behind the FFI boundary (libm is correctly
rounded) or behind a decision about ulp tolerance, and neither is a library call.

### 2. `floor` is in and `sqrt` is out, and the line is exactness

`floor`/`ceil`/`round` are as much "float functions" as `sqrt`, and they are in because they are **closed-form**:
`cast(s64, x)` truncates toward zero (ADR-0037), and a sign-based adjustment turns that into a floor, a ceiling
or a round — identical bits in both engines. `sqrt` is not expressible without a loop whose rounding the two
engines need not share. The distinction is exactness, not difficulty, and it is the same distinction that keeps
the transcendentals out.

### 3. Honest edges rather than convenient ones

- `abs(min_of_s64)` **traps** (through ADR-0002's checked subtraction) rather than returning a negative
  "absolute value": there is no correct result, and a silent wrong one is worse.
- `pow` with a negative exponent returns 0, because the true answer is a fraction an `s64` cannot hold.
- `clamp` applies `low` last, so a crossed range (`low > high`) has the defined answer `low` rather than an
  order-dependent one.
- `abs` and `fabs` are **separate procedures**, not one `$T`, because `0 - x` differs by type and there is no
  operator resolution across a template's instantiated type (the reason `Sort` takes a comparison, ADR-0104 §3).

## Consequences

- **A useful, correct `Math` exists**, and `valid/091` checks each `floor`/`ceil`/`round` against its exact
  expected value — including negatives, where truncation and flooring diverge — so a wrong last bit is a failing
  bit in both engines.
- **No new diagnostic code, no compiler change** — the second consecutive pure-library sub-wave.
- **What it costs the reader is stated in the module's own docs**: no `sqrt`, and why. A `Math` that silently
  lacked the transcendentals would be a worse surprise than one that says so at the top.
- **Deferred with reasons**: the transcendentals (want FFI floats or a ulp decision); `is_nan`/`is_inf` (want
  bit inspection of a float, i.e. `transmute`, deferred); a `float32` set (additive once this shape is proven).
  **FFI floats are the unblocker**, and they are a language sub-wave — the same shape as typed allocation
  unblocking `List`.

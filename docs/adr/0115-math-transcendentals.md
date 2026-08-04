# ADR-0115: `Math`'s transcendentals are libm wraps, now that a float can cross the FFI boundary

- **Status:** Accepted
- **Date:** 2026-08-04
- **Deciders:** dboll
- **W7 sub-wave 13.** The payoff of ADR-0114. `Math` (ADR-0112) shipped without `sqrt`, `sin`, `log` and named
  FFI floats as the reason; ADR-0114 delivered them; this adds the transcendentals the right way.

## Context

ADR-0112 §1 was explicit: a transcendental *approximated in Jairs* would make the two engines disagree on the
last ulp, so `Math` shipped only its exact closed-form half and waited for the FFI-float boundary. ADR-0114
opened that boundary. So the transcendentals are now writable as what they should always have been — **wraps of
libm** — rather than as approximations the differential harness would reject.

## Decision

### 1. `sqrt`, `sin`, `cos`, `exp`, `ln`, `powf` are `#foreign libc` declarations

libm's symbols live in the C library `Basic` already binds (ADR-0114), so `Math` imports `Basic` for `libc` and
declares each as a thin `#foreign` wrap. They are **correctly rounded** and **identical in both engines**,
because both call the same libm — `sqrt(2.0)` is bit-for-bit the same in the VM and native code, which is the
exactness ADR-0112 §1 said an in-language approximation could not have.

**`ln`, not `log`.** C's `log` is the natural logarithm, but the name misleads a reader expecting base 10; `ln`
says which base without a comment, and `log10` is a separate wrap when a caller wants it.

**`powf`, not an overload of the integer `pow`.** They take different types and there is no overload resolution
that would pick between them (the reason `Sort` takes a comparison, ADR-0104 §3). `powf(2.0, 0.5)` is a square
root, which the integer `pow` cannot express.

### 2. The exact half stays in Jairs

`floor`, `abs`, `gcd` and the rest remain computed in Jairs, exactly — they are on the exact side of ADR-0112's
line and need no boundary crossing. The module now has both kinds, and the docs say which is which and why: a
reader should know that `floor` is exact-in-Jairs while `sqrt` is a libm call, because the two have different
failure modes if libm is ever absent.

## Consequences

- **`Math` is complete** as a general-purpose numeric module, and ADR-0112's deferred item is closed. The arc
  is worth noting: a library (0112) named a language feature it needed, the language delivered it (0114), and the
  library collected (0115) — three sub-waves, each honest about what it could not yet do.
- **`valid/094` checks the transcendentals against exact libm results** — `exp(0)`, `sqrt` of a perfect square,
  `powf(2, 0.5) == sqrt(2)` — with `==` rather than a tolerance, because both engines call the same library so
  there is no ulp to tolerate. The exact half is checked beside them, confirming the wraps did not disturb it.
- **No new diagnostic code, no compiler change.** This is a pure-library sub-wave built on ADR-0114's capability.
- **Deferred**: `atan2`, `log10`, `floor`/`ceil` as libm wraps (the Jairs ones are exact and preferred);
  hyperbolic functions and the rest of libm (additive, one `#foreign` line each, added when a caller asks).

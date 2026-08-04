# ADR-0114: A float may cross the FFI boundary — passed in a float register, in both engines

- **Status:** Accepted
- **Date:** 2026-08-04
- **Deciders:** dboll
- **W7 sub-wave 12.** The language unblocker two library sub-waves named: `Math` (ADR-0112) could not wrap
  libm's `sqrt`/`sin` without it. The refusal was explicit — "passing FloatType to a foreign procedure arrives
  with a later wave" — so this is that wave.

## Context

Integers and pointers cross the FFI boundary as a machine word (ADR-0060 §2). A float **cannot** ride the same
path: on both SysV (x86-64) and AAPCS (arm64), a float argument is passed in a **floating-point register**
(`xmm0`, `d0`), not an integer one. Passing the bits as a `u64` puts them in an integer register the callee
never reads — so `sqrt` would compute on whatever was in `xmm0`, a plausible-looking wrong number, silently.

## Decision

### 1. libffi is told the argument and return are floats; native uses a float `AbiParam`

**The VM's libffi path** (`jr-vm/src/ffi.rs`) builds a **per-argument** CIF type: a word for an integer or
pointer, `Type::f32`/`Type::f64` for a float — which is what makes libffi place it in a float register. The
float value is decoded from its stored bits into a host `f32`/`f64` (kept alive across the call, because
`libffi::arg` borrows its operand), and a float return is read with `signature.call::<f32>`/`<f64>` rather than
`<u64>` and re-encoded to the declared width's bits.

**The native path** needed nothing new: a `#foreign` procedure's Cranelift signature is already built from each
parameter's `Repr`, and `clif_type` already returns `F32`/`F64` for a float scalar, which `CallConv::SystemV`
places in the float register. The two engines reach the same ABI by different routes, and the differential
harness confirms they agree.

**Passing the bits as an integer was the alternative, and it is wrong on every real ABI** — not merely
suboptimal. The bug would be a silent wrong number, which is this project's named worst case, so the register
placement is load-bearing rather than an optimisation.

### 2. A `float32` narrows at the boundary, keyed on the parameter type

libffi's `float` is 32-bit, and a Jairs `float32` holds its value in the low 32 bits of its word (ADR-0040 §3).
So `marshal` decodes with the **parameter's** `FloatKind`, not a blanket `f64` — passing a 64-bit pattern where
32 bits are expected would be a wrong call, and keying on the declared type is what prevents it. `sqrtf` (a
`float32` in and out) exercises this beside `sqrt`.

### 3. Scope kept honest

This ships the **capability**: a `#foreign` procedure may take and return a float, exercised by a corpus file
that calls `sqrt`, `sqrtf` and `pow` in both engines. It does **not** add libm to `Basic` (a set of `#foreign`
declarations — additive, separate) nor lift `Math`'s transcendentals (they can now be a libm wrap, which is
`Math`'s next sub-wave). The `#foreign` declarations in `valid/093` are local to the file, which is the honest
scope for a capability test.

## Consequences

- **`Math`'s transcendentals are unblocked**, and so is any numeric FFI. The gap ADR-0112 §consequences named
  — "FFI floats are the unblocker, and they are a language sub-wave, the same shape as typed allocation
  unblocking `List`" — is closed, and the parallel held: a library named the language feature it needed, and
  the language delivered it as its own sub-wave.
- **Both engines agree exactly**, because both call the *same* libm, which is correctly rounded — so
  `sqrt(16.0) == 4.0` is an exact comparison, not a tolerant one. This is the case ADR-0112 §1 said an
  *approximation* could not have, and it is why the transcendentals belong behind the boundary rather than
  approximated in Jairs.
- **No new diagnostic code**; two by-design refusals ("passing FloatType…", "a foreign procedure returning
  FloatType…") are lifted, each having named this wave.
- **Deferred with reasons**: an aggregate-of-floats by value (the struct-ABI decision, still deferred for
  integers too); a `float` *variadic* argument (`printf("%f")` promotes `float` to `double`, its own rule).

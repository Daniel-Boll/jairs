# ADR-0002: Integer overflow always traps

- **Status:** Accepted
- **Date:** 2026-07-25
- **Deciders:** dboll

## Context

Integer arithmetic that overflows the destination type is one of the classic
sources of silent bugs. The main language options are:

1. **Wrap always** (C's unsigned behaviour, and the practical default of much C
   code): overflow silently produces a modular result. Fast, but a wrong result
   propagates with no signal.
2. **Undefined behaviour on overflow** (C's signed behaviour): the optimiser is
   free to assume it never happens, which turns overflow into miscompilation.
3. **Trap in debug, wrap in release** (Rust's default): overflow panics in debug
   builds and wraps in release builds. This makes debug and release
   *semantically different programs* — a bug can be invisible under test and
   live in production.
4. **Always trap**: overflow is always an error, in every build, detected at
   runtime with a source location, or at compile time when the operands are
   known.

A trapping default is only tenable if the language *also* provides explicit
wrapping operators. Hash functions, PRNGs, and checksums are defined in terms of
modular arithmetic; without wrapping operators they cannot be written at all —
and the Jairs standard library, which is written in Jairs, contains exactly
these. `PLAN.md` §5 records this as a risk discovered "while writing the
stdlib", which is why wrapping operators were promoted into wave W1.

## Decision

Integer overflow **always traps**. There is no wrapping default, no undefined
behaviour, and no debug/release semantic difference. A trap is:

- a **runtime panic with a source location** when it happens at run time, in
  both the VM and the native backend (they must agree — this is a slice exit
  criterion in `PLAN.md` §1.4); and
- a **compile error** when the overflow is detectable at compile time (e.g. a
  constant expression that overflows).

Explicit wrapping operators `+%`, `-%`, and `*%` (and their compound-assignment
forms `+%=`, `-%=`, `*%=`) provide modular arithmetic where it is genuinely
wanted. These are lexed and reserved in the Jairs-0 slice and become usable in
wave W1.

## Consequences

### Positive

- One semantics for arithmetic across debug, release, VM, and native. What
  passes a test is what ships.
- Overflow is a reported error with a location, not a silent wrong answer.

### Negative

- A per-operation overflow check unless the backend/optimiser can prove it away.
  The check is explicit in MIR (like bounds checks — see ADR-0003) so an
  optimisation pass can eliminate provably-safe ones.
- Code that genuinely wants modular arithmetic must say so with `+% -% *%`
  rather than getting it for free.

### Follow-on work this forces

- **Into wave W1:** the wrapping operators `+% -% *%` (and compound forms) are
  promoted forward from their natural home so that hash/PRNG/checksum code — and
  therefore the stdlib — can be written. See
  `tests/corpus/valid/013-wrapping-ops.jr`.
- **Into the slice:** the trap must be emitted identically by the VM and by
  Cranelift, and the differential test harness must confirm they agree.
- MIR must represent the overflow check explicitly so a later pass can strip
  provably-safe checks.

## Alternatives considered

- **Wrap always (C-like).** Rejected: silent modular results are exactly the bug
  class trapping is meant to eliminate.
- **Undefined behaviour on overflow.** Rejected: hands the optimiser a licence to
  miscompile.
- **Trap in debug, wrap in release (Rust's choice).** Rejected because it makes
  the debug and release builds *semantically different languages*; a program can
  be correct under test and wrong in production, which is the worst possible
  place for the discrepancy to surface.

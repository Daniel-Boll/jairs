# ADR-0003: Bounds checks are a build setting

- **Status:** Accepted
- **Date:** 2026-07-25
- **Deciders:** dboll

## Context

Array indexing needs a runtime bounds check to be safe, but a systems language
also needs the ability to remove that check where the programmer has decided the
cost is not worth paying. Jai's model is a *build setting*: bounds checking is on
by default and can be turned off for a build, with a local opt-out at the call
site. Two implementation strategies present themselves:

1. Make bounds checking **implicit in lowering** — emit or omit the check while
   turning indexing into machine operations, driven by a flag read at lowering
   time.
2. Make the bounds check an **explicit operation in the IR** that a dedicated
   build-config pass strips, with a local `#no_abc` directive to suppress it for
   a specific index.

Arrays are a wave W1 feature, not part of Jairs-0. But whether the check is
visible in the IR is an architectural property of MIR, and it is far cheaper to
design MIR with an explicit `bounds_check` operation from the start than to tease
one out of ad-hoc lowering later.

## Decision

Bounds checking is a **build setting**, exactly as in Jai. MIR carries an
explicit `bounds_check` operation. A build-configuration pass strips these
operations when checking is disabled for the build, and `#no_abc` suppresses the
check locally at an individual index. The check is a first-class, visible thing
in the IR — never an implicit side effect of lowering an index expression.

## Consequences

### Positive

- The check is inspectable, optimisable, and strippable as a unit; a
  const-propagation pass that proves an index in range can delete exactly that
  operation.
- The build-config pass has one clear job: remove `bounds_check` ops.
- `#no_abc` is a local override with an obvious IR meaning.

### Negative

- MIR is slightly larger because the check is a distinct operation rather than
  folded into indexing.

### Follow-on work this forces

- **Into the slice:** MIR's design must include a `bounds_check` operation and a
  build-config stripping pass, even though Jairs-0 has no arrays to index — the
  representation cannot be retrofitted cheaply.
- **Into wave W1:** arrays (`[N]T`, `[]T`, `[..]T`) and the `#no_abc` opt-out
  land together, consuming the machinery designed here.

## Alternatives considered

- **Implicit in lowering.** Rejected: a check that only exists as a decision made
  during lowering cannot be inspected, cannot be stripped as a unit, and cannot
  be individually eliminated by an optimisation pass. It also makes the
  build-setting behaviour a special case in the lowering code rather than one
  self-contained pass.

# ADR-0008: Error handling is Jai's, with a reserved effect-row slot

- **Status:** Accepted
- **Date:** 2026-07-25
- **Deciders:** dboll

## Context

Error handling is a decision that pervades every function signature, so getting
it wrong is expensive to fix: changing the error model later can mean re-typing
every procedure in the language and the standard library. Jairs starts with
Jai's approach — **multiple return values plus a `#must` marker** that forces a
returned value to be consumed — which is simple, has no runtime machinery, and
fits a no-exceptions language.

But an effects/error-row system (in the style of algebraic effects or a typed
error row) is an attractive future direction, and the thing that makes it a
rewrite is that it wants a slot in the *function type* that a Jai-style model
does not have. Reserving that slot now costs essentially nothing and avoids
re-typing every signature later.

Note that multiple return values are themselves a wave W2 feature and `#must`
lands with it; the Jairs-0 slice has single-return procedures only. What is
decided *now* is the shape of the function-type representation.

## Decision

Error handling **starts as Jai's**: multiple return values plus `#must`. But the
function-type representation in `jr-pool` **reserves an effect-row slot from the
start**, so that an effects system can be added later without re-typing every
signature. The slot is inert today.

## Consequences

### Positive

- The initial model is simple and needs no runtime support, matching the
  no-exceptions design value.
- A future effects/error-row system can be introduced without touching every
  existing signature, because the slot it needs already exists in the type.

### Negative

- The function type carries a field that is unused today, a small amount of
  representational weight paid against a future that may or may not arrive.

### Follow-on work this forces

- **Into the slice:** `jr-pool`'s function-type representation must include the
  (currently inert) effect-row slot now.
- **Into wave W2:** multiple return values and `#must` land with the flow-and-
  scope wave.
- **Into a future wave:** an effects system, if pursued, fills the reserved slot
  rather than restructuring the type.

## Alternatives considered

- **Jai-style multiple returns + `#must`, with no reserved slot.** Rejected only
  in the narrow sense that omitting the slot would force a re-typing of every
  signature if effects are ever added; the *behaviour* today is identical, so the
  slot is pure insurance at negligible cost.
- **A full effects system from the start.** Rejected: far too much design and
  implementation weight for the slice, and unproven for this language; the
  reserved slot keeps the door open without paying for the room now.

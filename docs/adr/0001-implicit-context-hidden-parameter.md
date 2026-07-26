# ADR-0001: Implicit context is a hidden trailing parameter

- **Status:** Accepted
- **Date:** 2026-07-25
- **Deciders:** dboll

## Context

Jai passes an implicit `context` — the current allocator, logger, and other
ambient state — to every ordinary procedure, so that allocation and logging can
be redirected per-callsite without threading a parameter through every
signature by hand. There are two plausible ways to make that ambient value
reach a procedure body:

1. A **thread-local**, read at the top of each procedure. Zero ABI cost,
   invisible in the type system, but a hidden global: it cannot be reasoned
   about statically, it interacts badly with coroutines and `#c_call`
   boundaries, and it means the *type* of a procedure tells you nothing about
   whether it touches the context.
2. A **hidden trailing parameter**, added to the calling convention. It costs
   one register per call, but it is honest: the context flows through the call
   graph like any other value, `#c_call` procedures can be given a different
   convention explicitly, and the function *type* can record whether the
   convention includes it.

`context` itself is a wave W3 feature, not part of the Jairs-0 slice. But the
*calling convention* is fixed in MIR from the very first native call, and
retrofitting a convention change is a rewrite of every lowering path and every
`#foreign` boundary. The decision therefore has to be made now even though the
feature ships later.

## Decision

The implicit context is a **hidden trailing parameter** in the calling
convention. Ordinary Jairs procedures carry it; `#c_call` procedures opt out and
use the platform C convention instead. Every `#foreign` procedure is `#c_call`
implicitly, because it targets a C ABI that knows nothing about Jairs' context.

Function *types* carry a context flag as part of their identity, so the type
system can distinguish a context-taking procedure from a `#c_call` one, and MIR
encodes the flag in its calling-convention representation from day one.

## Consequences

### Positive

- The context is statically visible: whether a call passes it is a property of
  the callee's type, not a hidden runtime fact.
- `#foreign`/`#c_call` interop is clean — the boundary is exactly where the
  convention flag flips, and it is expressed in the type.
- No thread-local machinery, no interaction with green threads or fibers.

### Negative

- One register of per-call overhead for ordinary procedures.
- The MIR calling-convention representation is more complex from the start.

### Follow-on work this forces

- **Into the slice (Jairs-0):** MIR's calling convention must encode the context
  flag now, and the function-type representation in `jr-pool` must include it,
  even though no slice procedure reads the context.
- **Into wave W3:** the actual `context` value, `push_context`, and the
  `#c_call` opt-out semantics land with the runtime-core wave.
- Every `#foreign` declaration is treated as `#c_call`; see
  `tests/corpus/valid/019-foreign.jr`.

## Alternatives considered

- **Thread-local context.** Rejected: it makes the context a hidden global,
  invisible in the type system, hostile to `#c_call` boundaries and to any
  future coroutine/fiber support, and it removes the compiler's ability to know
  from a procedure's type whether it participates in the context protocol.

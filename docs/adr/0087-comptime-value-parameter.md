# ADR-0087: A comptime-value parameter `$N: s64` — the surface, with the call refused pending instantiation

- **Status:** Accepted
- **Date:** 2026-08-04
- **Deciders:** dboll
- **W5 sub-wave 6, first half.** This delivers the *surface* of a comptime-value parameter: `$N: s64`
  parses, lowers, formats, and its body type-checks. A **call** to such a procedure is refused by design
  (E0271) pending the second half, which evaluates the argument to a constant and instantiates per value —
  exactly as ADR-0081 delivered `$T`'s surface and ADR-0082 made a call run.

## Context

Jai's `$` on a *value* parameter marks it **polymorphic over a compile-time-known value**: `make :: ($N:
s64)` is instantiated once per distinct `N`, and the useful case is a length — `buf: [N]T` inside such a
procedure, or a `struct($T)` field of type `[N]T`. It is the value-side mirror of the type-side `$T`
(ADR-0081), and the last piece before the macro family.

`$N: s64` **does not parse today**: `parse_param` expects an identifier after the optional `using`, so the
leading `$` is E0108 "expected a parameter name". This ADR was written only after confirming that by
running, per AGENTS.md's rule that a schedule's stated premise is checked before it is obeyed — a habit that
has caught a false schedule five times.

## Decision

### 1. `$N: s64` is a parameter whose *name* is preceded by `$`

The grammar's `param` gains an optional leading `$` before the name — distinct from `$T` in *type*
position, which is a `poly_type`. A `$` here marks the **parameter** comptime-polymorphic; its type
annotation is ordinary (`s64`, `u32`, …). `Param` gains a `comptime: bool`, the value-side counterpart of
a type parameter's `$`, and `ProcSig` records whether any parameter is comptime — a procedure with one is a
**template**, like a `$T` procedure, and gets no concrete signature usable at a call until instantiation.

**Why a flag on the parameter, not a new parameter kind.** A comptime-value parameter is an ordinary
parameter in every respect except *when* its argument is known — same name, same type annotation, same
default rules. A separate kind would duplicate all of that; a `bool` says exactly what differs.

### 2. Its body **is** type-checked, unlike a `$T` template's

This is the load-bearing difference from `$T`. A `$T` template's body cannot be checked because the
parameter's *type* is unknown until instantiation. A `$N: s64` parameter's type is **fully known** — only
its *value* varies — so `N` is a genuine `s64` in the body and the body type-checks soundly at template
time. A body error (`N + true`) is caught here, a sub-wave before instantiation, rather than deferred.

MIR for the template is still skipped, exactly as a `$T` template's is (`lower_file` gates on the
template mark): `N` has no runtime value until a call fixes it, so emitting code that read it as an ordinary
parameter would be a placeholder miscompile.

### 3. A call is refused by design (E0271) pending the second half

Until instantiation exists, a call to a comptime-value-parameterised procedure is refused with a
by-design code that names what arrives later — the same shape as `$T`'s original E0268 (ADR-0081 §2). This
is a refusal, not a gap: the construct is named as arriving in the next half, so nothing is lowered to a
placeholder. What the second half (a future ADR) adds is: evaluating each `$N` argument to a compile-time
constant *at the call site* — the sema↔VM mutual recursion ADR-0073 broke with an acyclic pre-pass, reused
rather than rebuilt — keying an instantiation on the tuple of argument *values* (the value-side analogue of
ADR-0005's structural key over types), and unlocking `[N]T` where `N` is such a parameter.

### 4. `[N]T` with a comptime-value `N` waits for the second half

`[N]s64` where `N` is a `$N` parameter needs `N`'s *value* at the point the array type is resolved, which
is exactly the const-eval-at-a-call the second half delivers. Until then a `$N` parameter is usable as an
ordinary `s64` value in the body (once instantiation runs), and `[N]T` over it is deferred with the call.

## Consequences

- **A new diagnostic code, E0271**, for a call to a comptime-value-parameterised procedure — by design,
  reworded or lifted when the second half lands, exactly as E0268 was.
- The `comptime` flag threads from the parser (`PARAM` gains an optional `$`) through the typed AST,
  `jr-hir`'s `Param`, and `jr-sema`'s `ProcSig`. The formatter emits the `$`, and dropping it would silently
  turn a comptime parameter into an ordinary one — the lossy-CST failure the formatter guards against, so a
  round-trip corpus file pins it.
- **The body checks at template time**, which is strictly more than `$T` offered at the equivalent stage,
  and it is sound because the parameter's type is known. This is the reason the two halves split where they
  do: everything that does not need the argument's *value* is in the first half.
- A corpus file in `tests/corpus/type-errors/` pins the by-design refusal (E0271), and one exercising the
  parse/format round-trip lives beside the `$T` files; the *running* corpus file waits for the second half,
  because a program that cannot call the procedure cannot yet observe a value.

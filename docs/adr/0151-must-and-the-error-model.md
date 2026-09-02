# ADR-0151: `#must` — the error-handling marker ADR-0008 chose and never built

- **Status:** Accepted
- **Date:** 2026-09-01
- **Deciders:** dboll
- **PLAN §8.6 step 2**, and it unblocks five of W7's remaining modules. **Fills ADR-0008's reserved
  effect-row slot**, which has been inert since the vertical slice.
- No design fork was put to the decider; the recommended approach was taken and the rejected
  alternatives are recorded below, per the session directive.

## Context

### The model was chosen in the slice and half-built for the whole programme

ADR-0008 decided this language's error handling before any of it existed: **multiple return values plus
a `#must` marker**, with an effect-row slot reserved in the procedure type so that a future effects
system would not mean re-typing every signature. It named `#must` six times and scheduled it "into wave
W2".

Multiple returns shipped in W2. `#must` did not ship at all — not in W2, and not in the twelve waves
since. So the model was half a model: a fallible operation could *return* a success flag, and nothing
could stop a caller ignoring it.

**That is what blocked five modules.** PLAN §8.1.1 found that `File`, `File_Utilities`, `Process`,
`Socket` and the useful half of `JSON` all wait on this, and that shipping them first would be worse
than waiting: the idiom whichever module was written first happened to use would *become* the error
model, chosen by accident rather than decided.

### The slot was reserved for exactly this

`EffectRow` has been a zero-sized struct in `jr-pool` since the slice, with a doc comment saying it
exists because "adding an effects system later would otherwise mean re-typing every signature in the
compiler". A test asserted it was zero-sized.

`#must` is an obligation on a *call*, which is what an effect is. So this wave does not add a mechanism
beside the reserved one — it puts the first thing into it.

## Decision

### 1. `#must` is a procedure attribute, and the obligation lives in the procedure's **type**

`f :: (…) -> (T, bool) #must { … }`, taken in the same attribute loop as `#c_call`, `#no_abc`,
`#expand` and `#modify`, so it may be written in any order with them. The flag lands in
`EffectRow { must }`, which is part of procedure-type identity.

**Why the type rather than a side table**, which was the real design decision here:

- **It crosses module boundaries for free.** A call site has its callee's *type* even when it has no
  HIR for the callee's declaration. A `ProcId`-keyed table would have needed the same cross-file
  threading `imported_procs` exists to do — for one bit.
- **It survives being taken as a value.** `f := only_one;` then `f(10)` still checks, because the
  pointer and the procedure have the same type. A table keyed on the declaration would have silently
  lost the obligation at exactly the point a caller might be trying to launder it.
- **It cannot be dropped by assignment.** Two procedure types differing only in `#must` are *different
  types*, so assigning a marked procedure where an unmarked one is wanted is a mismatch rather than a
  silent loss. That is the property the rewritten pool test now asserts.

**Rejected: a `ProcId`-keyed side table in `FileSignatures`.** It works for same-file calls and needs
new plumbing for imported ones, and it answers nothing for a procedure pointer.

**Rejected: growing `EffectRow` into a real effect row now** — named effects, a row per procedure.
ADR-0008's door stays open precisely because `must` is a *field* rather than the whole struct, and there
is no second effect to generalise from yet. One example is not a pattern.

### 2. The check is at one place, and `_ = f();` is the escape hatch

The refusal (**E0287**) lives in the `Stmt::Expr` arm of statement checking, because that is the *only*
position in the language where a value is produced and dropped. An initialiser, an argument, an
operand, a `return`, a target list — all of them *receive* the result. One check therefore covers the
whole language, and nothing has to be remembered at every expression position.

**`_ = f();` is accepted, and this is a deliberate trade.** An unbypassable check is one people route
around with a one-line wrapper procedure, which hides the decision instead of recording it. The point
is that ignoring a failure must be *visible* — greppable, reviewable, present in the diff — not that it
must be impossible. That is the same trade `#no_abc` already makes for bounds checks: the opt-out
exists and has to be spelled.

**It needed a new statement.** `_` was already a discard *inside a target list* (ADR-0052 §3), but
`_ = f()` for a single value is not a one-element target list: `destructured_results` refuses a single
position outright (E0251), because a destructuring statement over a single-return call is a genuine
mistake worth reporting. So `Stmt::Discard` is its own variant — reusing the tuple would have meant
weakening a check that catches a real error.

`Stmt::Discard` is a variant rather than a flag on `Stmt::Expr` for the reason `Stmt::LocalTuple` gives
about itself: every exhaustive match over `Stmt` should be forced to consider it. The two are
semantically identical — evaluate, drop — and differ only in whether the programmer *said* so, which is
precisely the distinction `#must` turns on.

**Rejected: no escape hatch at all.** Wrappers, invisibly.
**Rejected: `#must` per return value**, which Jai allows. It needs syntax inside the return list, and
the overwhelming common case is "all of them". Deferred rather than declined.

### 3. `#must` on a `void` procedure is refused at the declaration

**E0288.** There is nothing to receive, so the marker could never be violated and would never do
anything — and a reader who wrote it believes a check is running. ADR-0058 §3's rule about
silently-ignored directives, applied to the newest one.

Reported at the declaration, which is the mistake's own site; the call-site check stays deliberately
quiet for a `void` callee so one error is not also reported at every call.

## Consequences

- **The error model is complete as ADR-0008 scoped it.** A fallible operation returns a value beside a
  flag and the flag cannot be silently skipped, which is what the five blocked modules were waiting for.
- **`EffectRow` is no longer inert**, and the test that asserted it was zero-sized now asserts the
  property that actually matters: that the row participates in procedure-type identity, so the
  obligation cannot be laundered through an assignment.
- **`_ = expr;` is new syntax**, usable for any deliberate discard rather than only for a `#must` one.
- **The formatter lost `#must` on the first attempt**, which is the tenth wave in a row where that
  file has dropped a construct — and this one was the *unsound* direction: dropping it deletes a check,
  so every caller ignoring a failure silently starts compiling again. Caught by gate 5 on this wave's
  own corpus file.
- **E0289 is the first free code.**

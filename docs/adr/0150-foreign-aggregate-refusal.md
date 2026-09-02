# ADR-0150: A `#foreign` signature that cannot be lowered is refused, not crashed

- **Status:** Accepted
- **Date:** 2026-09-01
- **Deciders:** dboll
- **PLAN §8.6 step 1**, and the cheapest item in that section. It converts W10's hard gate (§8.1.2)
  from a crash into a stated limitation.
- No design fork was put to the decider; the recommended approach was taken and the rejected
  alternatives are recorded below, per the session directive.

## Context

### The ninth leaked internal error

Found by probing while writing PLAN §8, not by a test. This program declares cleanly and then fails
inside the compiler:

```jai
libc :: #system_library "c";
Pair :: struct { a: s64; b: s64; }
takes :: (p: Pair) -> s64 #foreign libc "takes";

main :: () { p: Pair; exit(takes(p)); }
```

```
error: procedure 0 in file 0 was defined without being declared   (Cranelift)
error: internal compiler error: no routine for file 0 proc 0      (VM)
```

Two *different* internal errors, for one legal-looking program, with no diagnostic anywhere. This is
the ninth occurrence of the shape `AGENTS.md` names as one of this project's two real failure modes.

**The refusal already existed.** `jr-codegen-llvm`'s signature builder says, in words,
`"an aggregate passed across a #foreign boundary"` — it just says it at a layer that has no span and
no user. The Cranelift path never declared the procedure at all, which is why its message is about a
definition rather than about a type. So this wave is not inventing a rule; it is moving one to where it
can be read.

### Why C cannot take these types today

Passing a struct by value across a C boundary is not a matter of copying bytes. It requires the
platform ABI's field-classification rules — which fields go in which register class, when the whole
struct goes in memory instead, when a hidden pointer appears. ADR-0051's `sret` did the *return* half
for Jairs's own calling convention; this is the *argument* half, for the C convention, on two
architectures, plus libffi's equivalent for the VM. That work is PLAN §8.1.2 and it gates W10.

## Decision

### 1. The refusal is at the declaration, and it is exhaustive over the pool

`jr-sema` refuses a `#foreign` signature carrying any type with no C representation, as **E0286**, at
the declaration site.

**At the declaration rather than the call**, because the signature is what cannot be lowered. A binding
that could never be called successfully *is* the error; refusing at the call would report one fact once
per call site and say nothing about a binding whose first caller has not been written yet — which is
the normal order for library bindings.

`foreign_boundary_refusal` matches the pool item **exhaustively** rather than with a `matches!`, so a
new type is a compile error there instead of silently becoming passable. That matters concretely: two
waves ago `#simd` added a type, and had this check existed with a `_ => None` arm, a vector would have
become quietly passable and produced exactly the reinterpretation this ADR is about.

**Rejected: refusing in `jr-mir`**, where the lowering actually fails. MIR has spans but a body's
refusal channel is `give_up`, which produces E0245 — a *warning* — so the program would still link and
still crash when the call was reached. That is the mechanism that let ADR-0120's four defects reach an
engine.

**Rejected: fixing it in each back end.** Both already fail; the problem is that they fail *late* and
in two different voices. A per-back-end diagnostic would be two copies of one rule, which is this
project's standard definition of two chances to disagree.

### 2. The message names the workaround, and differs by why

Not one sentence for every shape. A `string` is the aggregate a caller is most likely to try, and its
advice is specific and immediately actionable:

```
error[E0286]: `s` cannot cross a `#foreign` boundary: it is `string`
  = a `string` is a pointer and a count, and C has no such type: pass `s.data` and `s.count` as two arguments
  = pass a pointer instead — `*T` is one register, and the callee reads through it
```

while a struct gets the reason it is genuinely hard, and a return type gets out-parameter advice
because that is what the C signature would have done anyway. A view and a dynamic array share the
`string` advice, since they are the same kind of descriptor. An array is refused with the fact that C
decays one to a pointer and Jairs does not.

**A `#simd` vector is refused too, and it is the one refusal not about width.** A vector *is* one
machine register (ADR-0148 §1), so the honest reason is not "too big" but "no engine here declares one
across a C boundary" — passing it would be a silent reinterpretation rather than a call. Saying "too
large" would have been false and would have sent a reader looking for a smaller vector.

### 3. What stays passable

Scalars, pointers, floats, `bool`, an enum (its backing integer), a procedure pointer, and `void`. That
is exactly the set every existing `#foreign` declaration in `modules/` and the corpus already uses —
checked before writing the rule, so this refusal breaks nothing that works today.

`PoolId::ERROR` is deliberately *passable* here: poison has already been reported, and refusing it
again would double-report one mistake.

## Consequences

- **A crash becomes a diagnostic**, and W10's gate becomes describable: PLAN §8.1.2 is now a stated
  limitation with a code attached rather than an internal error nobody could act on.
- **E0286 is spent**, so E0287 is the first free code.
- **One corpus fixture**, `type-errors/079`, which records the two internal-error strings it replaced —
  because a fixture that only asserts the new code would lose the reason the file exists.
- **When §8.1.2 lands, this refusal narrows rather than disappears.** Aggregates become passable and
  the vector and `ResultsType` arms stay, so the exhaustive match keeps earning its place.

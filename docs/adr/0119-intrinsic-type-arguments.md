# ADR-0119: An intrinsic may take a parameterised type argument — three refusals of one construct

- **Status:** Accepted
- **Date:** 2026-08-06
- **Deciders:** dboll
- **W7 sub-wave 17.** The fourth of the small unblockers ADR-0118 named, and the one blocking `Map`: its
  allocation needs `size_of(Slot(K, V))`, which did not parse as a type.

## Context

`size_of`, `typed` and `view` take a **type**, but an intrinsic's argument is an **expression** — so
`Slot(s64, s64)` parses as a **call**, not a `TypeRef::Apply`. ADR-0118 §2 reverted `Map`'s conversion to a
generic struct for exactly this, and named it as small. It was small, and it was **three** separate refusals of
the same construct — which is why the first two fixes moved the error rather than removing it.

## Decision

### 1. Sema resolves a call-shaped type argument as a type application

`described_type` — the one function every intrinsic asks "what type is this argument?" — gains a `Expr::Call`
branch: the callee's name is the constructor, each argument is resolved **recursively** as a type (so
`Box(Box(s64))` works), and `Ctx::apply_resolved` interns the instance.

`apply_resolved` is a sibling of `resolve_apply` taking already-resolved `PoolId` arguments rather than
`TypeRefId`s, and everything from "constructor and arguments are known" onward moved into a shared
`instantiate_parameterised`. **Two copies of that tail would be two chances** for the recursion guard or the
binding save/restore to drift — the ADR-0086 §3 machinery is subtle enough that duplicating it would be the wrong
economy.

Recognised in sema rather than in the parser, because **the parser cannot know** that this particular call is in a
type position — only the intrinsic does, and `described_type` is where every intrinsic already asks.

### 2. The resolver's type-position flag had to become *sticky*

`jr-hir`'s resolver has an "inside an intrinsic's type argument" flag, which is what lets `s64` — an ordinary
identifier that resolves to no declaration — not be an unresolved name there. It was **assigned** per call
(`flag = this_callee_is_an_intrinsic`), so a **nested** call cleared it: `Slot(s64, s64)`'s own callee is not an
intrinsic, so `s64` inside it was E0201.

Now `flag = outer || intrinsic`. That is correct rather than merely permissive: a type argument's own arguments
are types all the way down, so once inside one, every nested argument position is still a type position.

### 3. MIR's `scan` accepts a **struct name** in callee position

The inner `Slot(...)` callee names a *struct*, and `scan` refused the body with "a call to something that is not
a procedure". The whole `size_of` folds to a constant, so that callee is never emitted — refusing for it would
refuse every program that measures a parameterised instance.

Recognised by **what the name is** rather than by the fold: a struct is not callable, so a struct name in callee
position is only ever a type application. That is the same shape as the existing arm for an enum-member receiver
(`Colour` in `Colour.GREEN`), and it is more robust than keying on the fold, which would depend on the const query
having run.

**Three refusals, one construct** — and each fix revealed the next, which is worth recording: a construct that
crosses phases needs each phase asked separately, and "the error moved" is progress rather than a failed fix.

## Consequences

- **`Map($K, $V)` is now a generic struct**, so all three containers are (ADR-0118's deferred half). `valid/095`
  passes unchanged, and the MIR snapshot did not move — the instances lay out as the concrete structs did.
- **`valid/098` pins the capability**: `size_of` of a two-parameter instance, of two *different* one-parameter
  instantiations (a wrong implementation could bind only the first argument or share a layout), of a **nested**
  application, and `typed` with a parameterised type — plus an ordinary `size_of(s64)` beside them.
- **No new diagnostic code.** Three refusals were lifted; none was a numbered diagnostic (two were internal
  `scan`/resolver behaviour, one an E0201 that should never have fired).
- **Three named unblockers remain**, all in *procedure* polymorphism rather than types: inference through a
  parameterised struct (`*Array($T)`), which would make the container procedures generic; cross-file `$T`
  procedure instantiation (E0268); and `using` on an imported struct.

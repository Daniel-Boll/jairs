# ADR-0084: A type variable is inferred through a pointer or view parameter

- **Status:** Accepted
- **Date:** 2026-08-04
- **Deciders:** dboll
- **W5 sub-wave 4.** ADR-0081 §4 and ADR-0083 §3 restricted inference to a parameter typed *directly*
  `$T`. This extends it one structural layer: `deref :: (p: *$T)` and `count :: (items: []$T)` now infer
  `T` from a `*U`/`[]U` argument.

## Context

A polymorphic procedure over a *pointer* or a *view* is the common case — a `sort` takes `[]$T`, a `swap`
takes `*$T` — and none could be called: inference bound a variable only from a parameter that was *exactly*
`$T`, so `*$T` given a `*s64` argument found no direct site and was refused (E0268, "cannot infer every
`$T`"). The restriction was deliberate (a placeholder while the direct case was built), and lifting it is a
one-layer extension of the same structural match, not a new mechanism.

## Decision

### 1. Inference matches the parameter's `TypeRef` structure against the argument's resolved type

When a parameter's declared type is `*$T` and the argument resolves to `*U`, bind `T = U`; when it is
`[]$T` against `[]U`, bind `T = U`; a direct `$T` against `U` binds `T = U` as before. The match walks both
in lockstep, peeling one constructor at a time, and binds the variable where a `TypeRef::Poly` meets a
concrete type.

This is `infer_var_in(param_type_ref, arg_type)`: recurse through `Pointer`/`View` on the `TypeRef` side
while peeling the matching `PointerType`/`ViewType` on the `PoolId` side, and bind at `Poly`. It is exactly
the direct case (`Poly` at depth 0) generalised to `Poly` at depth *n* under matching constructors.

### 2. A shape mismatch binds nothing, and is a mismatch, not an inference

If the parameter is `*$T` and the argument is a non-pointer, the structures do not align, so nothing binds.
That is not an error *here*: the variable is then unbound, the call is refused if no other position pins it
(ADR-0083 §3's rule, unchanged), or — if the argument was simply the wrong shape — the re-resolution and
per-instantiation argument check report an ordinary mismatch against the concrete parameter type. Inference
never *reports*; it only binds where it can, and the existing checks judge the rest.

### 3. One variable per structural position, still no two-way unification

The match is one-directional: it reads a binding *out of* the argument type given the parameter's shape. It
is not a unifier — there is no substitution into the argument, no occurs-check, no solving of `$A` against
`$B`. `swap :: (a: *$T, b: *$T)` works because both positions bind the same `T` (the second is checked
against the first's binding, ADR-0083 §3); `f :: (a: $A) -> *$A` needs no inference in the return at all.
What stays out is inferring a variable that appears *only* in a composite with another variable — nothing
in W5's scope needs it, and it is where a real unifier would begin.

### 4. What is still deferred

- **`$$T`**, **polymorphic structs**, **macros** — ADR-0083 §4's list minus the item this closes.
- **Two-way unification** (§3).
- **Inference through a *nominal* type's parameter** — `Array($T)` — which is polymorphic structs, not
  this.

## Consequences

- **A pointer or view polymorph is now callable**, which is most of what a generic `sort`/`swap`/`find`
  needs — the shapes ADR-0081's one-`$T` slice could declare but not call.
- **The change is confined to inference.** Re-resolution under the binding, the structural key, the clone,
  and per-instantiation checking are all unchanged — a nested `*$T` parameter re-resolves to `*U` once `T`
  is bound, exactly as a direct one re-resolves to `U`. Only the step that *finds* the binding reaches
  deeper.
- **A shape mismatch degrades to an ordinary type error** (§2), so `deref(42)` — a non-pointer where `*$T`
  is wanted — reports a mismatch rather than a confusing inference failure.
- **W5 sub-wave 4 done**; §4 names what remains (`$$T`, polymorphic structs, macros).

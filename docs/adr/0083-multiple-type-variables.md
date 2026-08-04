# ADR-0083: A polymorphic procedure may introduce several type variables

- **Status:** Accepted
- **Date:** 2026-08-04
- **Deciders:** dboll
- **W5 sub-wave 3.** ADR-0081 §4 restricted sub-wave 1 to one `$T`, and ADR-0082 instantiated it. This
  lifts the restriction: `pair :: (a: $A, b: $B)` now instantiates, keyed on the tuple of both bound types.

## Context

Sub-wave 2's instantiation refused a procedure with more than one type variable (E0268, "more than one
`$T` is not instantiable yet"), a by-design boundary while the one-variable path was built. Everything that
path built — inference from a directly-`$Var` argument, the structural key, the `proc_bindings` binding
map, the HIR clone, the MIR redirect — is **plural by nature**; sub-wave 2 collapsed each to the
single-variable case. ADR-0005 even fixed the key as a *tuple*. So this sub-wave is mostly the removal of a
restriction, not a new mechanism.

## Decision

### 1. A signature's `poly_vars` may hold more than one variable, and the key is the tuple of their bindings

`pair :: (a: $A, b: $B) -> A` introduces `A` and `B`. `check_polymorphic_call` infers each from the first
parameter typed *directly* as that variable (§3), forms the structural key as the `Vec<PoolId>` of bound
types in the variables' **first-seen order** (ADR-0005, ADR-0083's ordering making it deterministic), and
records `(proc, bound types)` for the expansion pass. Two calls with the same tuple share one
instantiation; `pair(s64, bool)` and `pair(s64, s64)` are distinct tuples and distinct instantiations,
because their bodies differ.

The one-variable path was exactly this with a length-one tuple. Lifting it is replacing the "`poly_vars`
has length 1" guard and the single `bound: PoolId` with the whole list.

### 2. An instantiation carries one binding per variable

`FileHir::proc_bindings` maps `(ProcId, variable, type)`; sub-wave 2 pushed one entry per instantiation,
this pushes one *per variable*. The signature phase already binds *every* variable a signature introduces
(it iterates `poly_vars`), so it needs no change beyond reading a per-variable binding rather than assuming
one — which it already does by keying `proc_bindings` lookups on `(proc, var)`.

`expand_instantiations` takes a `Vec<(variable, type)>` per instantiation instead of a single pair, and
pushes each. Nothing else in the clone changes: the body and parameter `TypeRef`s are copied structurally
regardless of how many variables the signature has.

### 3. Each variable is inferred from its first *direct* position

Variable `A` binds from the argument at the first parameter whose type is exactly `$A`; `B` from the first
`$B`. A later bare `A` or `B` (another parameter, the return) is checked against the binding. This is
sub-wave 1's rule per variable, unchanged.

**Nested-position inference stays out** (ADR-0081 §4): a `$T` reachable only through `*$T` or `[]$T` is not
an inference site, because that needs a unifier this wave still does not build. A signature whose variable
appears *only* nested — with no direct position to bind it — is refused (E0268, reworded), not
half-inferred.

### 4. What is still deferred

- **`$$T`** — a comptime-only *value* parameter, which interacts with const-eval. Its own sub-wave.
- **Nested-position inference** (§3) — inferring `$T` from `*$T`.
- **Polymorphic structs** — `Array($T)`. A parameterised *type* needs the type-value machinery to carry a
  type constructor, which is more than a procedure's variables.
- **Macros** (`#modify`, `#bake_arguments`, `#expand`) — the remaining W5 family.

## Consequences

- **The one-variable restriction and its E0268 message are gone**, replaced by inference over the whole
  `poly_vars` list. A refusal remains only for a variable with no direct binding site (§3), which is a real
  limitation rather than an arbitrary count.
- **The structural key is a tuple, as ADR-0005 always specified.** Sub-wave 2's one-`PoolId` key was the
  degenerate case; nothing about the caching or de-duplication changes, because a one-element tuple and a
  bare id dedupe identically.
- **`valid/067`'s single-variable cases still hold** — the generalisation is a superset, so the sub-wave-2
  corpus runs unchanged, and a new `valid/` file adds the multi-variable cases.
- **W5 has three sub-waves done**, and §4 names what remains, so the next starts from a boundary.

# ADR-0082: A polymorphic call instantiates by expanding the HIR, checked and lowered per instantiation

- **Status:** Accepted
- **Date:** 2026-08-04
- **Deciders:** dboll
- **W5 sub-wave 2.** ADR-0081 delivered the `$T` surface and refused a call (E0268). This lifts the
  refusal: a call to a polymorphic procedure now **instantiates**, producing a concrete procedure that
  both engines run like any other.

## Context

ADR-0081 §2 fixed the model — instantiate at the call, in the check phase, keyed structurally (ADR-0005) —
and deferred the mechanism because it is the wave's largest piece: an instantiation is a **new procedure**
that is not in the source's `procs` arena, and the whole compiler keys a procedure by `ProcRef =
(FileId, ProcId)`. Threading a new identity through the signature phase, MIR, both engines and the
differential is the work.

**There is a precedent that is isomorphic.** ADR-0073's computed `#insert` had the same shape — "produce
program elements a source file did not literally contain, then check and lower them like any other". It
solved it by building an **expanded `FileHir`**, re-resolving and re-checking that tree
(`checked_expanded`), and lowering it; `file_mir` already branches on whether an expansion exists.
Instantiation reuses exactly this: append a substituted `Proc` per instantiation to the expanded HIR, and
every downstream pass treats it as an ordinary procedure with **no new keying**.

## Decision

### 1. `check_call` infers `$T` and records the instantiation

When a call reaches a polymorphic callee, `check_call` binds each `$T` from the corresponding argument's
type, forms the structural key (ADR-0005: the tuple of bound `PoolId`s — one, this sub-wave), and records
`(callee proc, bound types)` against the call expression — the way `type_info_calls` is recorded (ADR-0075
§2), so the expansion pass reads one type inference rather than repeating it. The E0268 refusal is removed.

A call whose `$T` cannot be inferred — because an argument did not type — records nothing and reports the
argument's own error; it does not fabricate an instantiation.

### 2. An expansion pass appends a substituted procedure per distinct key

`file_mir` gains an instantiation branch beside the `#insert` one. When a file has polymorphic calls, it
builds an expanded HIR: for each **distinct** structural key, it appends a clone of the polymorphic `Proc`
to the `procs` arena, and rewrites each matching call's callee to the new `ProcId`. De-duplication is the
structural key (ADR-0005), so `id(s64)` from two calls appends one procedure.

The clone is **not** substituted in the HIR — its parameter and return `TypeRef`s still say `$T` and `T`.
Substitution happens in **sema**, via the `type_bindings` map ADR-0081 added: the instantiation carries
its bindings (`T = s64`), and when sema computes the instantiation's signature and checks its body, it
populates `type_bindings` so every `$T` and bound `T` resolves to the concrete type. This keeps `jr-hir`
free of `PoolId` (it has never depended on `jr-pool`, and a `TypeRef::Resolved(PoolId)` would couple them
for one feature) and puts the substitution where types already live.

**The instantiation's `poly_vars` is empty.** It is a concrete procedure — its `$T` are bound — so it is
lowered and declared like any other, which is what §3 relies on.

### 3. `checked_expanded` recomputes signatures over the expanded tree, and the body is checked per instantiation

`checked_expanded` reused the *unexpanded* signatures for `#insert`, because an insert is body-scoped and
cannot add items (ADR-0072 §5). An instantiation **does** add procedures, so signatures are recomputed over
the expanded HIR — the appended instantiations need entries. This is the one way instantiation's expansion
differs from `#insert`'s, and it is why the branch is separate rather than shared.

Each instantiation's body is checked once, against its concrete signature (ADR-0081 §2): a body correct for
`s64` may be wrong for a struct (`a + b` where the struct has no `+`), and that must be a diagnostic, not a
miscompile. The check runs with the instantiation's bindings in scope.

### 4. Instantiations lower and run as ordinary procedures

Once expanded, an instantiation is a concrete `Proc` with a concrete signature, so MIR lowers it, both
engines emit it, and the differential runs it — none of them learns anything about polymorphism, exactly as
ADR-0081 §3 promised. The polymorphic *template* still produces no MIR and no declaration (ADR-0081 §2); the
*instantiations* produce both.

### 5. What is still deferred

- **`$$T`** (comptime-only parameters), **multiple distinct type variables**, **macros**, **polymorphic
  structs** — ADR-0081 §4's list, unchanged.
- **A polymorphic call from a polymorphic body** — `f :: (x: $T) { g(x); }` where `g` is also polymorphic —
  needs instantiation to run *during* the checking of an instantiation, a fixpoint this sub-wave does not
  build. Refused, named, so it is a boundary rather than a miscompile.
- **Cross-file instantiation caching's full generality.** Within a file the structural key dedupes; a type
  argument that is itself imported works because the pool is shared, but the caching *across* files is
  ADR-0005's promise that a later sub-wave verifies at scale.

## Consequences

- **The expanded-HIR machinery gains a second client**, which is the first evidence ADR-0073's structure
  generalises beyond `#insert` — the same way ADR-0075 §2's `Basic`-lookup gained a second client in `Any`.
- **Signatures are recomputed on the expanded path** for the first time, because instantiation adds items
  where `#insert` did not. The `#insert` branch keeps reusing them; the two branches differ in exactly this.
- **A polymorphic program is checked by checking its instantiations**, so the differential's coverage of
  polymorphism is the coverage of the concrete procedures it produces — `id(42)` and `id(true)` are two
  ordinary programs to it.
- **E0268 is removed** as a call refusal and repurposed nowhere; the code is retired to the "past its
  block" list, its corpus file (`type-errors/066`) replaced by a `valid/` file that *runs* an instantiation.
- **This does not finish W5.** §5 names what remains, so the next sub-wave starts from a boundary.

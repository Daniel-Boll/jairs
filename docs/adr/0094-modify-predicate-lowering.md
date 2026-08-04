# ADR-0094: A `#modify` predicate is lowered as an ordinary procedure and cloned per instantiation

- **Status:** Accepted
- **Date:** 2026-08-04
- **Deciders:** dboll
- **W5 sub-wave 7e.** This **amends ADR-0093 §2**, which was wrong about what the evaluation needs. The
  predicate is now a real lowered procedure, cloned per instantiation with that instantiation's bindings —
  everything up to *running* it. Running it is the remaining step, and this ADR states honestly what it
  still needs.

## Context

ADR-0093 §2 said evaluating a `#modify` predicate would need "a way to lower a body *from text* outside
`LowerCtx`", which it called an API change. **That was wrong**, and the correction is worth recording because
it made the sub-wave much smaller than its own design predicted:

- `lower_body` takes an **AST `Block`**, and a `#modify` block *is* one — `Proc::modify_block()` hands it
  over directly. No text round-trip, no new entry point.
- So the predicate can be lowered **at the template**, once, by the same `lower_body` every procedure uses.

## Decision

### 1. The predicate is a synthetic procedure, lowered at the template

`lower_proc` lowers a `#modify` block into its own `Proc`: no parameters, returning `bool`, body = the block.
It gets a synthetic unexported name (`$modifyN`) so the signature phase computes its signature — that phase
only does so for a *named* item — and is deliberately **not** in `scope`, so nothing can call it by name.
`Proc::modify` changed from `Option<String>` (ADR-0093 §1's text) to `Option<ProcId>`.

Three exclusions follow, each of which the build discovered by failing:

- **`FileHir::predicate_vars`** records the guarded template's `$T` names against its predicate. A predicate
  has no `poly_vars` of its own, but its body says `type_info(T)` — so without this, checking the template's
  predicate reported E0261 "needs a type", the same gap ADR-0092 fixed one level up. Sema seeds
  `poly_var_names` from it and withholds.
- **MIR does not lower a predicate's body** and **`declarations()` does not declare it**: it is a
  compile-time guard, not runtime code. Missing either gave the linker `function "jr$0$0" ... must be
  defined but is not` and then `procedure 4 was defined without being declared` — the same pair a macro
  produced (ADR-0091 §1), caught the same way, by the corpus differential.

### 2. Each instantiation clones the predicate with its own bindings

`expand_instantiations` clones the predicate alongside the instantiation, giving the clone that
instantiation's `proc_bindings` — so `type_info(T)` inside it describes *that* bound type (ADR-0092 §1).
`FileHir::modify_predicates` pairs `(instantiation, predicate clone)`.

**Cloned rather than shared**, and that is not an optimisation: two instantiations of one template must
evaluate the predicate against *different* bindings. Sharing one procedure would evaluate it once and apply
the answer to both — wrong for at least one of them, and silently so.

### 3. What remains, stated precisely

Running the clone. It is an ordinary no-parameter `bool` procedure in the **expanded** tree, so it needs that
tree's MIR and the VM — and `instantiated()` runs *before* `file_mir`, while `file_consts` evaluates against
the *unexpanded* tree. So this genuinely does need a new query (or a second evaluation pass inside
`file_mir`), which is the one thing ADR-0093 §2 got right about the size.

E0274 continues to refuse a call meanwhile, so nothing is silently unguarded — which is the property that
makes shipping this increment safe rather than half-built.

## Consequences

- **The predicate is real code now**, checked by sema like any body, with its `type_info(T)` resolving in
  each clone. Everything except the final evaluation is in place and green.
- **ADR-0093 §2 is amended, not merely refined**: its stated blocker did not exist. The lesson is the one
  AGENTS.md already states — trace the machinery before estimating, because `lower_body`'s signature was the
  whole answer.
- Three exclusions (`predicate_vars` for sema, MIR body skip, codegen declare skip) are the same three a
  macro needed. A fourth construct that is "checked but never run" should expect the same three.

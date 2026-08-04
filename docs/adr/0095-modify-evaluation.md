# ADR-0095: A `#modify` predicate runs at compile time and a `false` refuses the instantiation

- **Status:** Accepted
- **Date:** 2026-08-04
- **Deciders:** dboll
- **W5 sub-wave 7f.** ADR-0093 delivered the surface (refusing a call, E0274) and ADR-0094 lowered the
  predicate and cloned it per instantiation. This **runs** it: a `false` refuses the guarded instantiation
  with E0275, and E0274 is retired. `#modify` is complete.

## Context

ADR-0094 §3 left exactly one step and stated precisely what it needed: the predicate clone is an ordinary
no-parameter `bool` procedure in the **expanded** tree, so running it needs that tree's MIR *and* the VM —
and neither existing host had both. `instantiated()` runs before any MIR exists; `file_consts` evaluates the
*unexpanded* tree.

## Decision

### 1. The predicate runs in `file_mir`, right after the expanded tree is lowered

`file_mir` is the one place with all three things: the expanded HIR, its MIR (just produced), and access to
the VM. `evaluate_modify_predicates` walks `FileHir::modify_predicates`, calls each clone, and returns one
diagnostic per `false`. Those ride out on `MirResult::expanded_diagnostics` — the channel a computed
`#insert`'s and an instantiation's own diagnostics already take — so `file_diagnostics` picks them up with **no
new plumbing and no new query**, which is what ADR-0094 §3 hoped for and could not confirm.

**E0275 is `jr-db`'s**, beside E0230 and E0271, because this is where the evaluation happens. Its message
names the *predicate* rather than reading like a compiler fault, because a rejection is the author's intent:
they wrote a guard precisely so some instantiations would be refused.

### 2. A predicate that fails to *run* is not a rejection

A trap, an unsupported operation, or a context that could not be allocated leaves the instantiation
**standing**, and any real problem is reported by the ordinary refusal path. "The guard could not be
evaluated" and "the guard said no" are different findings, and only the second is what the author asked for
— conflating them would turn a compiler limitation into a false rejection of correct code.

### 3. Two things the build discovered, both by running

- **A predicate clone's body must be lowered to MIR.** ADR-0094 skipped it in *both* MIR and
  `declarations()`, which was right for the native back end and wrong for the VM: a body with no MIR has no
  routine, and the call returned `no routine for file 0 proc 4`. MIR now lowers a clone and only
  `declarations()` skips it — the two exclusions are for different reasons and had to be separated. A
  *template's own* predicate is still skipped in MIR, because `T` is unbound there and only clones are ever
  evaluated.
- **A predicate takes the hidden context parameter**, like every Jairs procedure (ADR-0057 §4). Calling it
  with no arguments gave `called a procedure taking 1 arguments with 0`. The context's layout is read
  *before* the VM borrows the pool, the same order `run_main` uses and for the same reason: the mutex is not
  reentrant.

## Consequences

- **`#modify` is complete**, and `E0274` is **retired** rather than reused — the way E0120 and E0122 were —
  with a note at its old site so a reader searching for it finds out why. That is the fourth by-design
  refusal this project has raised and then lifted (E0268 for `$T`, E0271's first meaning for `$N`, E0272's
  first meaning for `#expand`, now E0274), and each named the sub-wave that removed it.
- A template can now enforce its own constraints in code: `#modify { return type_info(T).id ==
  type_info(s64).id; }` refuses every other instantiation, with the rejection pointing at the guarded
  procedure. `imports/invalid/015` pins it.
- The predicate is evaluated **once per instantiation**, not once per call, because it is cloned per
  instantiation (ADR-0094 §2) and instantiations are deduped structurally (ADR-0005). Two calls at the same
  type therefore evaluate the guard once — which is both faster and the only answer that cannot be
  inconsistent between them.
- `#bake_arguments` is the last of W5's macro family.

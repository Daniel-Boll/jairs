# ADR-0047: an enum member is found through its *type*, and a refused body that runs is a diagnostic rather than a crash

- **Status:** Accepted
- **Date:** 2026-07-28
- **Deciders:** dboll
- **Amends:** ADR-0017 §4's refusal channel, which is kept and given a *report* at the point a
  refused body is actually needed. ADR-0041's `enum_member_of` restriction is lifted, and the
  reason it existed is honoured rather than overruled — see §1.

## Context

ADR-0046 §7 recorded a discovery made by running things: a bare `.GREEN` on an **imported** enum
works and gives the right value, while the qualified `Colour.GREEN` on the same enum fails with

```text
error: internal compiler error: no routine for file 0 proc 0
```

`PLAN.md` §7 named the crash as the thing to fix first, because a crash tells a user nothing.
Four facts were established by running the compiler before this ADR was written, and two of them
changed the shape of the fix.

- **`jr check` reports zero errors on the crashing program.** So this is not "a refused body
  after an error" — it is a **well-typed program that passes checking and then crashes the
  compiler**. That is a strictly worse category than ADR-0017 §4 anticipated, and it is what
  makes the second half of this ADR necessary rather than cosmetic.
- **The refusal only bites when the body is *called*.** A refused `unused :: ()` sitting beside a
  working `main` runs fine and exits 5. So `assemble.rs`'s existing reasoning — skip a refused
  body, because a program that never calls it runs — is *correct*, and only incomplete.
- **Sema already types `Colour.GREEN` as the enum.** The expression's own type is the enum type,
  which means the member can be found from the *type* with no reference to any HIR arena. This is
  the fact that makes §1 cheap: ADR-0041's restriction existed because `enum_member_of` reaches
  the enum through `Res::Item` and an `EnumId`, and there is a route that does not.
- **ADR-0046 already added that route.** `enum_member_value(ty, name)` was written for the bare
  form, which has no receiver to resolve. It works unchanged for the qualified form.

## Decision

### 1. An enum member is found through the expression's **type**, not through its receiver's declaration

`enum_member_of` resolved the receiver name to a `Res::Item`, read an `EnumId` out of the HIR,
and built a `DeclId` from it. That is why an imported enum was refused: the `EnumId` indexes
*another file's* arena, which is exactly the cross-body read ADR-0017 §3 keeps out of the built-MIR
query. The restriction was right about the hazard.

It is unnecessary, because there is a second route to the same answer. `jr-sema` types
`Colour.GREEN` as the **enum type**, and `Item::EnumType` carries the `DeclId` — so:

```rust
// The member comes from the expression's own type, which sema has already resolved.
// No HIR arena is read, so no cross-body dependency is created.
Expr::Field { name, .. } if self.enum_member_value(ty, name).is_some() => …
```

ADR-0017 §3's rule is **honoured, not bent**: the pool's member side table is keyed on `DeclId`,
and `FileSignatures::record_in` has already populated it for every imported module before this
query runs. Nothing here reaches into another file's HIR.

**Rejected: widen `enum_member_of` to handle `Res::Imported`** by looking the enum up through
`FileSignatures`, mirroring what ADR-0018 §5 did for cross-file *callees*. This would work and it
is more machinery than the problem needs: the callee case had to go through signatures because a
call needs the callee's *signature*, where a member needs only a value the pool already holds.
Choosing the type-directed route also deletes a function rather than growing one.

**Consequence worth naming:** the two spellings now resolve by the *same* mechanism, which is why
they can no longer disagree. Before this, `.GREEN` worked and `Colour.GREEN` crashed on the same
enum — an asymmetry no user could predict, arising purely from which of two code paths a wave had
happened to touch.

### 2. A refused body that is actually **called** is reported, not left to crash

`assemble.rs` skips a body MIR refused, and its doc comment argues that calling one "produces
`VmError::Internal` from the interpreter's own lookup, which names the procedure — a better error
than a wall of refusals at assembly time for procedures nobody wanted."

The first half is right and the second half is wrong. Skipping is correct: a refused body nobody
calls should not stop a program (verified — it exits normally today). But the failure mode for one
that *is* called is an **internal compiler error surfaced to the user**, on a program `jr check`
called clean. No user can act on `no routine for file 0 proc 0`.

So the *entry point* is reported, at the point it is needed:

- **E0245 is a *warning*, raised for every refused body in `file_diagnostics`** — so `jr check`,
  `jr run`, `jr build` and the LSP all see it through the one path they already share.

  The severity was **decided by measurement, not by preference**. An error was implemented
  first, and it rejected six files in `tests/corpus/imports/valid/` plus a `Cycle_B` fixture
  that had been silently unlowerable since they were written: each reads an imported *constant*,
  which `jr-mir` still refuses. Those programs work today, so erroring would have broken working
  code to report a compiler gap. A warning states the gap without rejecting anything.

- **The entry point is a hard error.** `run_main` checks `main`'s own body before assembling, and
  fails with a message naming it. That is what actually closes the crash: `main` is called by
  definition, so a warning alone would have let the ICE straight back in through the door the
  severity opens. Only `main` is checked — a refused body deeper in the program matters only if
  it is *reached*, and deciding that statically is a call graph this query deliberately does not
  build.
- **The wording says what the user can do**, which is nothing about their own code —
  so it says so, and asks for a report. A diagnostic that pretends the user made a mistake is
  worse than one that admits the compiler has a hole (the ADR-0043 lesson about a diagnostic
  being accurate and useless, applied to blame).
- **The skip stays.** A refused body that is never called still costs nothing, because that is
  the case the original reasoning got right.

**Rejected: report every refused body at assembly time.** This is what the existing comment
argues against, and it is still the wrong trade: a wave that has not yet implemented a construct
refuses bodies containing it, and a wall of diagnostics for procedures nobody calls would make
every partial feature look broken.

**Rejected: make `lower_body`'s refusal carry a user-facing diagnostic.** Tempting — the refusal
knows *why*, in a string like "an enum member sema did not resolve". Rejected because those
strings are deliberately compiler-facing (`Poisoned::Here`), and ADR-0017 §4 made every refusal
*silent* on purpose: the body is refused either because an earlier phase reported the cause or
because a feature has not landed, and a second diagnostic on the same line is noise. Reporting at
the *entry point* keeps that property while closing the hole, because there is exactly one place
per run where a refusal becomes user-visible.

**Rejected: panic with a better message.** A crash is a crash. The compiler has a hole and the
right response is a diagnostic naming it, not a tidier abort.

### 3. E0245 is a *compiler* diagnostic, and the only one

It is the first code in this project that reports a **compiler limitation** rather than a program
error — E0231 was the first warning, and this is the second, and the first admission. That is a category worth
having exactly once: a single code, raised at one place, meaning "this program is legal and this
compiler could not lower it".

It belongs to `jr-db`, beside E0230's const-eval error, because `jr-db` is what owns the built-MIR
query and therefore the only crate that can see a refusal without inverting a dependency.

The code is **E0245**. `PLAN.md` §7 recorded E0245 as the first free code, which was checked
against `jr-sema`'s and `jr-db`'s `code.rs` rather than believed — the rule ADR-0039 §3a
established after `AGENTS.md`'s "first free code" note turned out to be stale.

## Consequences

- **`enum_member_of` is deleted**, along with its `ItemKind`/`ConstValue` walk. The qualified and
  bare forms now share `enum_member_value`, so the guard on the qualified arm becomes the same
  predicate the bare arm already used. A wave that removes a function is rarer than one that adds
  one and worth noting as evidence the route was right.
- **An imported enum is fully usable**: `Colour.GREEN`, `.GREEN`, comparison, a parameter, a
  `cast` to its backing integer. A corpus program under `tests/corpus/imports/valid/` exercises
  it, which is the directory that existed for exactly this and had no enum case.
- **The `no_routine` ICE becomes unreachable from a user program.** It stays in `jr-vm` as an
  internal invariant — the interpreter is still right to refuse a call it cannot resolve — but the
  path that reached it from a legal program is now gated by E0245.
- **`jr build` needs the same gate as `jr run`.** Both resolve an entry point, and a check in only
  one would leave the other crashing — the asymmetry this ADR exists to remove, reintroduced in a
  different pair.
- **A test asserts the diagnostic rather than the fix.** The imported-enum bug is fixed, so a
  `jr-cli` test constructs a refused body deliberately — a reference to an imported *constant*,
  which `jr-mir`'s docs record as still unconditionally refused — and asserts both halves: a
  warning at check time, and a hard failure naming `main` when it runs. Chosen so the test
  survives the bug that motivated it being gone.
- **E0245 immediately found work nobody had asked it to.** Six `imports/valid/` files and the
  `Cycle_B` fixture were unlowerable and silent. `Cycle_B` is fixed here — it now returns a
  literal and uses its import through a *call*, which ADR-0018 §5 made lowerable — because a
  fixture whose job is proving cycles legal should not depend on a feature that does not exist.
  The six corpus files are left as they are, with the warning as the honest record: each is
  testing *resolution*, and rewriting them to avoid an imported constant would weaken what they
  test.
- **Two `jr-db` tests asserted `diags.is_empty()`** and now assert `!diags.has_errors()`. That is
  the distinction E0245 introduces: a file can be warning-clean or error-clean, and those tests
  were about type errors.

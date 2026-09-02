# ADR-0168: `$$` in a return type — E0290, found by auditing a table against itself

- **Status:** Accepted
- **Date:** 2026-09-02
- **Deciders:** dboll
- **Not a wave.** A defect found while auditing PLAN's wave table at W10's close, plus the three stale markers
  that led to it.
- No design fork was put to the decider; the recommended approach was taken and the rejected
  alternatives are recorded below, per the session directive.

## Context

### The audit, and why it looked at all

W10 closed, so §7 needed rewriting and the wave table needed checking. Three of the table's inline
**`[NOT DELIVERED]`** markers turned out to be stale — `it`/`it_index`, `[..]T` and `$$T` had all shipped.

That is the rot `AGENTS.md` warns about, one level up from where it warns about it: the markers were added in one
wave to correct a *different* rot (the table claiming things W2 never delivered), and then went stale themselves.

**Each was re-verified by probe rather than by trusting either document**, because PLAN and `AGENTS.md`
*disagreed*: the table said `$$T` was undelivered, `AGENTS.md` said ADR-0137 delivered it. Both were partly
right, and the probe is what established which part.

### The probe found an ICE

`$$T` as a **parameter** works and is exercised by `valid/110`. `$$T` as a **return** type checked clean and the
call died with:

```text
error: internal compiler error: no routine for file 0 proc 3
```

**The tenth instance** of the leaked-internal-error shape this project tracks — a legal-looking program, no
diagnostic, an internals message asking the user to report a bug. Neither document knew about it, because the
return position had never been written.

## Decision

### 1. `$$` in a return type is refused — E0290

`$$` is `$` **plus** "and the argument is a compile-time constant" (ADR-0137 §1). So it marks a *parameter*,
where there is an argument to bake. **A return has no argument**, so the second `$` has nothing to say and
`-> $$T` is `-> $T` with a typo in front of it.

So the construct is not unimplemented — it is *meaningless*, which is the strongest case for a refusal rather
than a feature. The help says so: *write `$T`*.

**Rejected: making `-> $$T` mean `-> $T`.** Silently accepting a decoration that cannot mean anything trains a
reader to think the second `$` does something, and the day it means something in some other position, every
existing `-> $$T` changes meaning. A refusal costs one edit and keeps the notation honest.

**Rejected: implementing whatever `-> $$T` might mean.** Nothing coherent is available. "The return value is a
compile-time constant" is a claim about a *body*, not a signature, and it is what `#run` already expresses.

### 2. Refused in lowering, and it walks the result list

**In lowering**, for the reason E0276 is (ADR-0096): the validity of a type *decoration* at a declaration site is
judged where the signature is built. **Owned by `jr-hir`**, continuing its block.

**The check walks every `TypeExpr` the return position holds**, not just a bare one, so `-> (s64, $$T)` is caught
too — the fixture pins both, because a check on a bare type would pass the tuple form and leave the ICE reachable
by one extra character.

### 3. The fixture lives in `imports/invalid/`, not `type-errors/`

Filed under `type-errors/` first, where it **failed two harness assertions** rather than passing: that directory's
contract is that its files *"parse, lower and resolve cleanly"* and are rejected by **sema**. E0290 comes out of
lowering, so `type_error_corpus_files_parse_cleanly` refused it, and
`type_error_corpus_files_report_exactly_what_they_declare` saw a downstream E0251 instead.

**The rule was met by moving the file, not by weakening it** — which is what E0262, E0273 and E0276's fixtures
each did before it, and the comment block in `imports_invalid_corpus_fails` now records six such moves. A
directory whose contract is about the *stage* an error comes from is more useful than one that accumulates
exceptions.

### 4. The wave table's three stale markers are struck through, and the note above it says what happened

`it`/`it_index` (ADR-0133, and ADR-0135 for a range with an index), `[..]T` (ADR-0136's syntax and ADR-0140's
operations, which *deleted* ADR-0107's hand-rolled `List($T)`), and `$$T` as a parameter (ADR-0137).

**The two remaining markers are honest and stay**: W1's `[..]T` entry is struck through in place, and W8's
parallel codegen was **measured and refused** (ADR-0149) — which is a result, not an omission, and marking it
"not delivered" without that sentence would misread a decision as a gap.

## Consequences

- **A leaked ICE is a diagnostic.** Tenth instance closed, the same way ADR-0150 closed the ninth.
- **E0290 is `jr-hir`'s; E0291 is the first free code.** `crates/jr-cli/tests/codes.rs` caught the stale
  `FIRST_FREE` claim immediately, which is the second time the enforced registry has paid.
- **1059 tests**, **253 corpus files** — the fixture moved directories rather than adding to the count, since
  `type-errors/` lost one as `imports/invalid/` gained one.
- **PLAN's wave table is true again**, and its "not delivered" note now records that markers correcting rot can
  themselves rot. That is the only durable lesson here: **a claim about the code is only as good as the last time
  someone ran it**, and two documents disagreeing is a signal to probe rather than to pick.

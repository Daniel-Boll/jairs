# ADR-0101: `noted_insert` generates code for every noted declaration — the metaprogram loop, inside the fold

- **Status:** Accepted
- **Date:** 2026-08-04
- **Deciders:** dboll
- **W6 sub-wave 4.** ADR-0100 §2 said notes could be counted and named but **not looped over**, and named the
  static-data wave that would lift it. That is true for *inspection* and **wrong for generation**, and this ADR
  says why — the loop belongs inside the fold, where it needs no table at all.

## Context

**A capability found by probing, not by planning.** `#insert note_value(f, "gen")` **already worked** before
this sub-wave: `@gen "n = n + 5;"` on a declaration, spliced into a body, ran. Three shapes were checked and
all three worked with no changes — two splices in one body, a splice that calls a procedure, and a splice of an
*absent* note (empty and quiet). So the *effect* half of a metaprogram was already shipped and undocumented,
which is the kind of thing PLAN §1.5 exists to surface.

**What was missing was the loop over it**, and ADR-0100 §2's argument looked like it forbade one:

> A folding intrinsic is answered at check time, so a `for` variable — which exists only at run time — can
> never be its argument.

That argument is sound, and it forbids a loop **in the program**. It says nothing about a loop **inside the
fold**. Noticing that distinction is what made this sub-wave possible without the static-data table.

## Decision

### 1. `noted_insert(note, template)` emits the template once per noted declaration

```jai
alpha :: (x: s64) -> s64 @counted { … }
beta  :: (x: s64) -> s64 @counted { … }

#insert noted_insert("counted", "n = n + #(3);");
// folds to:  n = n + alpha(3);n = n + beta(3);
```

`#` stands for the declaration's name. The fold walks `noted_declarations` (ADR-0100 §1, declaration order),
substitutes, and concatenates; `#insert` then splices the result through the mechanism **ADR-0073 already
built**. So this adds one fold and reuses the query, the fold channel (ADR-0099 §2), and the splice.

**`#` rather than `$name` or `{}`**: a single character that is not valid in a Jairs identifier and is not
already an operator, so a template containing one is unambiguous. `$` is taken by polymorphism, `{}` reads as a
block, and a word-shaped placeholder could collide with a real name in the generated text.

**A template must be a literal** (E0277, the note intrinsics' shared code), for the reason every argument to a
fold must be.

**Nothing matching answers `""`**, which `#insert` accepts as "splice nothing" (ADR-0072 §4). A generated
section is therefore simply empty in a file with nothing to generate for, rather than a diagnostic about a
program that is correct — the same call §3 of ADR-0099 made for an absent note.

### 2. Why a loop in the fold is the *right* shape for generation, not a workaround

A run-time loop **could not do this at all**. Generated code has to exist before checking: a loop that ran
after the program was compiled could not declare a procedure, add a field, or emit a statement. So generation
is inherently a compile-time fold, and reaching for the static-data table here would be reaching for a tool
that cannot perform the task.

That splits ADR-0100 §2's deferred work cleanly in two, which is worth stating because the halves have
different fates:

- **Generation** — acting on every noted declaration by *emitting code*. Done here, with no table.
- **Inspection** — a *run-time* loop over declarations, reading names and types as values. Still owed the
  static-data wave, still bundled with `Type_Info`'s variable-length field list (ADR-0078).

ADR-0100 §2 treated those as one thing and deferred both. It was right about the mechanism and wrong about the
scope, and this is the correction — the same shape ADR-0094 took to ADR-0093 §2, whose stated blocker also
turned out not to apply.

### 3. Three rejected alternatives, each for its cost

- **Wait for the static-data table and a real `for`** — cannot deliver generation at all, per §2.
- **Return the names as one space-separated string** and let the script build the code — needs `String` to
  split, which is W7. ADR-0080 §3's rule: a facility whose consumer does not exist yet.
- **A `#for_each_note name { … }` directive** — rejected in ADR-0100 §2 and rejected again for the same
  reason: a second, hidden iteration construct with its own scoping rules, in a language that has `for`.

### 3. A folded value keyed by `ExprId` is **stale** once a body expands — the sharpest placeholder yet

Building this found a bug that predates it and that no verifier could have caught. `file_consts` records a
folded call's value against `(ExprScope, ExprId)` in the **unexpanded** tree; `file_mir` reads those values
against the **expanded** one; and a computed `#insert` renumbers every id after its splice. So with **two**
computed `#insert`s in one body, the second's recorded value landed on whatever expression now held its old id.

In `valid/082` that meant a `string` sitting on an arithmetic operand, and the failure was the MIR verifier
panicking with `mixed operand types` — **not** a diagnostic. This is the well-typed-placeholder family
AGENTS.md names, in its sharpest form so far: the two earlier instances (`Stmt::Error`, `Rvalue::Undef`) were
placeholders that happened to be legal values, while this one is *a genuine value computed from the same
program*, merely attached to the wrong expression. Nothing in the type system distinguishes it.

Fixed by **clearing the unexpanded entries and re-recording from the expanded check** in `file_mir` — the only
pass that saw the ids MIR will use. The clearing is the load-bearing half: a stale entry the expanded check
does not happen to replace is exactly the wrong value at a live id. `ConstValues::clear_run` exists for this.

**The general rule this instance confirms**, already learnt once for the insert-operand map (ADR-0072 §2, which
is keyed by *span* for precisely this reason): a result computed in one pass and consumed after an expansion
must be keyed by something expansion preserves, or re-derived on the far side. An `ExprId` is neither.

One insert had always worked, which is why this survived two sub-waves: `has_note` and `note_value` are read in
ordinary expression position, and `noted_insert` is the first intrinsic whose *whole point* is to appear as an
`#insert` operand — so it is the first thing that put two folded calls in one expanding body.

## Consequences

- **A metaprogram can now do the whole job**: find declarations by note, and generate code for each. That is
  W6's headline claim reduced to something a build script can actually write, and `valid/082` writes one.
- **No new diagnostic code**, and E0279 remains the first free code. Every note intrinsic's "unreadable at
  check time" refusal is E0277, which is now four intrinsics wide — right for the reason ADR-0099 gave: they
  are one mechanism, and a reader who hits any of them needs the same page.
- **A previously undocumented capability is now pinned**: `#insert note_value(f, "gen")` splicing a note's
  payload as code has a corpus file, so it cannot silently regress. It worked by composition and nobody had
  written it down.
- **Teeth-checked**: emptying the generated text moves `valid/082`'s exit from 78 to 73, so the generation is
  load-bearing in both engines rather than decorative.
- **Deferred, each with a reason**: a template referring to a note's *payload* as well as its name (wants two
  placeholders and an escaping decision); generating **declarations** rather than statements, since `#insert`
  at file scope is refused by ADR-0072 §5 and lifting that is its own decision; a separator other than plain
  concatenation.

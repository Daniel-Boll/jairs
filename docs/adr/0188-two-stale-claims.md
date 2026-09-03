# ADR-0188: two stale claims in the compiler, each costing a working program

- **Status:** Accepted
- **Date:** 2026-09-03
- **Deciders:** dboll
- Not a wave. Two defects found by writing `modules/Simp` against Jai's real API, recorded together
  because they are the **same failure** in two places: a comment that stated a limitation which had
  stopped being true, and which nothing checked.

## Context

Both were found the same way — a program that should compile did not, and the message pointed at the
call rather than at the cause. Neither had a test, and neither could have: both need a *second file*
or a *computed insert*, and the corpus had no program combining either with the feature it broke.

## Decision

### §1 — A constant's value is keyed by `ItemId`, and an expansion renumbers those

**Symptom.** `modules/GL` refused two procedure bodies with *"a file-level item has no value until
jr-vm"* — the E0245 gap report — for constants declared plainly as `TEXTURE_MIN_FILTER :: 10241;`. The
*last* constants in the file lost their values while earlier ones kept them, and **moving a constant
earlier broke a different procedure**. That asymmetry is what named the cause: an index shift.

**Cause.** `file_consts` evaluates the **unexpanded** tree. `modules/GL`'s library declaration is a
computed `#insert #run gl_library()`, which adds an item, and every `ItemId` after the splice moves. The
constant values are keyed by `ItemId`, so `consts.item(id)` looked up a shifted index and missed.

ADR-0184 §2 wrote this hazard down — *"`ItemId` is not stable across the re-lowering that consumes an
evaluated operand"* — and the code three lines away already handled it for one map: `folded_calls` are
cleared and re-recorded from the expanded check, under a comment calling the stale-value case "the
well-typed placeholder family in its sharpest form yet". **The `ItemId`-keyed map was not.** The hazard
was known, documented, and fixed in one of the two places it applies.

**Fix.** Re-key the item map after expansion, **by name**. An insert only *adds* items, so a
declaration's name is the identity that survives the re-lowering — the same reasoning ADR-0072 §2 uses
to key an insert by its *span*. An offset was rejected: it would have to know how many items each splice
contributed and where, which nothing records, and it would be wrong the moment a file had two inserts.

**One-line repro, for whoever meets this again:**

```jairs
#insert #run gen();
A :: 11;
main :: () { print_int(A); }
```

Before the fix, `main` was refused. It now prints 11.

**Why it refused rather than miscompiled, which is luck and not design.** A shifted index missed the
map entirely and produced `None`, which the lowering path turns into an honest refusal. A shift that
landed on *another constant of the same type* would have produced a **wrong value that type-checks** —
exactly what the `folded_calls` version of this bug did, where it surfaced as a verifier panic. That
outcome is still reachable by a file whose shift happens to align; nothing in the fix depends on the
alignment, so it is closed either way, but the *severity* of the class deserves recording.

### §2 — Sema said it could not reach an imported signature, and it could

**Symptom.** `Simp.set_shader_for_color()` — a call to a procedure whose only parameter has a default —
was *"this procedure takes 1 argument, but 0 were supplied"*. The **identical call inside the module**
worked. So a default argument silently did not apply across a module boundary, and a named argument was
refused there too.

That is the shape that wastes the most time: the gap looks like a property of the *call* rather than of
the boundary, so a reader checks the signature, the argument, and the default before suspecting the
import.

**Cause.** `callee_sig` returned `None` for `Res::Imported`, under this comment:

> A call to an imported procedure resolves through the other file's signatures, which this crate does
> not hold — so a named argument on a cross-file call is not supported and says so rather than silently
> ignoring the name.

**The premise was false.** `Ctx::imports` carries every imported module's `FileSignatures`, and it
always has — `entry_for_import` twelve lines away reads them. What was genuinely missing was an *index*:
`FileSignatures::proc_sig` is keyed by `ProcId`, an importer holds the exporting module's signatures but
not its HIR, and `SigEntry` carried no `ProcId`. So there was no route from a name to a signature, and
the comment described that absence as a decision.

**Fix.** `SigEntry` gains `proc: Option<ProcId>`, set for `SigKind::Proc` and `SigKind::Operator`, and
`Ctx::imported_proc_sig` walks name → entry → `ProcSig`. `callee_sig` uses it.

**Why a field on `SigEntry` and not a name-keyed map on `FileSignatures`.** A second map would be a
second thing to keep in step with the first, and the `ProcId` is already in hand at the one place a
`SigEntry` for a procedure is built. The field is `Option` rather than a separate `SigEntry` variant
because every consumer that does not care can ignore it, and a variant would make every match learn
about procedures.

### §3 — What these two have in common, and the rule they earn

Both are a **comment asserting a limitation, with nothing that checks the assertion**. AGENTS.md tracks
this family and this is the fifth and sixth entry:

| # | Claim | Cost |
|---|---|---|
| 1 | `jr-syntax`'s code table said E0131 was free | a collision |
| 2 | `file_consts`' early-out feature list | three separate wrong refusals |
| 3 | `TrapKind::ALL`'s `len() == 11` | four kinds never checked |
| 4 | `checked_expanded` reused signatures "because `#insert` adds no items" | a leaked ICE (ADR-0184) |
| 5 | `folded_calls` re-keyed, item values not (§1) | two refused bodies in `modules/GL` |
| 6 | "this crate does not hold the other file's signatures" (§2) | defaults dead across every module |

**Entries 4, 5 and 6 are all ADR-0184's `#insert` work or its neighbours**, and 5 is the sharpest: the
hazard was written down *and fixed in the adjacent map*. So the rule is not "write better comments" —
the comment was accurate when written, and #5's author knew the hazard.

The rule is: **when a fix re-keys one map because an identity moved, every map keyed by that identity is
suspect.** #5 would have been caught by asking "what else is keyed by `ItemId`?" at the moment
`folded_calls` was cleared. That question is cheap and it is not asked, because a fix that makes the
symptom go away feels finished.

And for #6, the older rule holds: **a comment saying a thing is impossible is a claim about the code,
and it is only as good as the last time someone ran it.**

## Consequences

- A constant declared after a computed file-scope `#insert` has its value. `modules/GL` compiles clean,
  and its per-OS library declaration no longer costs it the constants below it.
- Default and named arguments work across a module boundary, which is what lets `modules/Simp` and
  `modules/Window` have Jai's defaults-heavy signatures at all.
- **`SigEntry` grew a field**, so every construction site had to decide: eleven sites, all mechanical,
  and the two that carry a `ProcId` are the two that name a procedure.
- Still owed and now named: **a corpus program that combines a computed `#insert` with constants after
  it**, which is what would have caught §1. The repro in §1 is that program.

## Verification

- §1: the one-line repro prints `11`; four constants after a computed insert print `11 22 33 44`;
  `modules/GL` and `modules/File` check with 0 errors and no E0245.
- §2: a two-file program where the imported callee has a defaulted parameter exits `7` — `f(4)` with
  `b := 3` — and the same call was E0216 before.
- The Jai-shaped drawing program that motivated both builds, links and draws, exit 63.
- All seven gates green.

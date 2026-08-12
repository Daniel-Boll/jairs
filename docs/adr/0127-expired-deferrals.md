# ADR-0127 — Expired deferrals, and a narrower message for `void`

**Status:** Accepted
**Date:** 2026-08-12
**Amends:** no decision. It corrects statements that had stopped being true, and records which
promises from completed waves are still outstanding. No feature is added or removed.

## Context

A user hit this while probing something unrelated:

```
error[E0207]: declarations inside a procedure body are not supported yet
  = nested procedures and local constants arrive in wave W2
```

W2 completed **six waves ago**. Worse, `PLAN.md` §2.1's W2 row never listed nested procedures or
local constants at all — it is `for` with `it`/`it_index`, `for <`, labelled `break`/`continue`,
`defer`, `using`, multiple return values, named and default arguments, and `#scope_*`. So the note
named a wave that had both **passed** and **never owned the feature**, while reading to a user like a
schedule.

This is the class ADR-0125 named as the highest-value thing its audit found: *an expired
justification reads as a considered decision while being false*. ADR-0125 swept `PLAN.md`'s "Open"
list for it. It did not sweep the **code**, and the code had the same rot.

A second instance came from the same session. `size_of(void)` folds to **0** — probed, and genuine,
since `size_of` refuses an unresolvable name with E0261 — while naming `void` in type position
reported "unknown type name `void`" with the note "`void` is not a type name in Jairs". Two
diagnostics contradicting each other about whether a type exists is worse than either being terse.

## Decision

### 1. `void`'s message says only what is true

`void` is `PoolId::VOID`. It is storable — a zero-sized value still gets a distinct address, which
`Memory`'s own docs require — and `size_of(void)` is 0. What is *not* true is that it has a spelling
in type position. So the diagnostic now says exactly that:

```
error[E0212]: `void` cannot be used in type position
  = `void` is a real type and `size_of(void)` is 0, but it has no spelling in a type annotation
  = a procedure that returns nothing omits the `->` entirely; there is no `x: void` and no `*void`
```

The code stays **E0212** — no new code, so `AGENTS.md`'s enforced first-free-code claim is
untouched — but the primary message is no longer "unknown type name", because `void` is not
unknown. `type-errors/073` pins it. The `*void` half of the help earns its place: Jai's `null.*`
reads zero bytes precisely because its `null` is a `*void`, so that is the next thing a reader
tries.

### 2. No diagnostic or comment names a wave that has shipped

Swept and corrected. Each now states what is **owed** rather than when it will arrive, because an
unscheduled gap is honest and a fabricated schedule is not:

| Site | Said | Truth |
|---|---|---|
| `jr-hir` E0207 | "arrive in wave **W2**" | W2 shipped, and never owned it. Nothing owns it |
| `jr-hir` E0207 (`#run` stmt) | "arrives in wave **W4**" | W4 shipped; `x := #run f();` works, and a bare `#run f();` also checks clean, so this arm may be dead |
| `jr-sema` E0237 | evaluator "arrives with full `#run` in wave **W4**" | The evaluator exists. The real constraint is ordering — signatures are typed before const-eval (ADR-0018 §3), as E0233 already said |
| `jr-sema` E0247 | needs "the iteration protocol wave **W5**'s macros unlock" | W5 shipped the macros (ADR-0091). The protocol was never defined |
| `jr-vm` `ffi.rs` | `to_c_string()` "arrive with wave **W3**" | W3 shipped without them |
| `jr-diag` | `InstantiationFrame` defined early "even though polymorphs land in wave **W5**" | See §3 |
| `jr-syntax` `kind.rs` ×9 | tokens "reserved, wave W1/W2/W6" | floats, `union`, `for`, `defer`, `using`, `xx`, bitwise and `@` all landed |
| `jr-sema` `lib.rs` | "this crate does the first two" | It does all four |
| `jr-sema` ×2 | definite assignment is "wave **W3**'s job" | Shipped as E0245 in `jr-mir`, though only as a warning |
| `jr-cli` `differential.rs` | `cast` "reserved until wave **W1**" | `cast` landed (ADR-0037); `print_int` runs (ADR-0125) |

`kind.rs`'s `CAST_KW` already carried a comment recording that it *itself* had said "reserved, wave
W1" for three waves after `cast` landed. That was a warning nobody generalised, so eight of its
siblings had the same defect on the same page.

Left alone deliberately: references to **W6** (`#foreign_at_comptime`) and **W8** (LLVM, parallel
analysis), which are genuinely future; attributions of the form "(ADR-0012, wave W4)", which record
where something *was delivered*; and comments that already narrate their own past staleness.

### 3. `PLAN.md` §2.1 records which promises a completed wave did not keep

Six features were promised by a wave that has since been declared complete, and are absent. Probed,
not inferred:

- **`[..]T` dynamic arrays** (W1) — E0124. The growable array that exists is the `List($T)`
  *library* type (ADR-0107), not language syntax.
- **`it` / `it_index`** (W2) — `for xs { it }` does not parse; only `for x: xs` works.
- **`$$T`** (W5) — E0107, `$` must be followed by a name.
- **Instantiation backtraces** (W5) — `InstantiationFrame`, `with_frame` and a renderer all exist
  and are **used only by `jr-diag`'s own tests**. No production site constructs a frame, so no real
  diagnostic carries a backtrace. The type was defined in the slice *specifically* so this would not
  need retrofitting (`PLAN.md` §5); the retrofit is owed anyway, which is worth stating plainly
  because the pre-emptive work has so far bought nothing.
- **`Math` vec/mat/quat** (W7) — ADR-0115 declared "**`Math` is complete**" while §2.1's W7 row
  promises vectors, matrices and quaternions. W7 is open, so this is not overdue; the *completeness
  claim* is what was wrong.
- **Nested procedures and local constants** — never in any wave's scope, yet E0207 blamed W2.

Also recorded, since it is the same shape one level down: **array lengths accept a named constant**
(ADR-0070) and **enum members do not** (E0237). One mechanism, generalised in one place.

## Alternatives rejected

**Deleting the wave references instead of rewriting them.** A bare "not supported yet" loses the
information a reader wants next, which is whether anyone intends to do it. Saying "no wave owns
this" is *more* informative than a wave number, because it is true and it sets an expectation the
project can keep.

**Re-pointing each deferral at a plausible future wave** — W8 for `[..]T`, W9 for backtraces. This
is what produced the defect: every one of these notes was a guess written with confidence, and a
guess decays into a false promise the moment the named wave ships without it. A wave number in a
diagnostic is a commitment, and it should appear only where §2.1 actually carries the row.

**Building the six missing features here.** Each is a real wave with its own forks — `[..]T` needs
an allocator policy and a growth rule already argued in ADR-0107; `it`/`it_index` needs an iteration
protocol; backtraces need a call-site chain threaded through instantiation. Bundling any of them
into a wording sweep would make a regression in either unattributable, which is the argument
ADR-0086 §1 and ADR-0117 both made for splitting.

**Making `size_of(void)` an error instead**, so the contradiction resolves the other way. Rejected
because 0 is the *correct* size, `void` is genuinely storable, and `Memory` already depends on a
zero-sized value having an address. Removing a true answer to make a false message consistent is the
wrong direction.

## Consequences

- A reader who hits any of these refusals is told what is owed, not when it was due. Six absent
  features are now recorded as absent in the one document that schedules work.
- `type-errors/073` pins `void`'s message; the corpus grows by one file to **214**.
- No behaviour changes: no code path, no diagnostic code, and no accepted program is affected. The
  `jr-lsp` test that asserts on E0212's *other* branch (`unknown type name \`int\`` plus the builtin
  list) is untouched, which is the check that this sweep did not widen.
- **The residue is that prose is still prose.** This sweep was triggered by a user reading one
  diagnostic, not by a gate. Nothing prevents the next "arrives in wave WN" from being written, and
  ADR-0123's lesson — that the only claims which stay true are the enforced ones — applies here with
  no enforcement yet available. A lint that refuses the literal phrase "wave W" in a
  `with_note`/`with_help` string is conceivable and is not attempted here.

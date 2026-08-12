# ADR-0128 — A diagnostic inside an instantiation names the call that demanded it

**Status:** Accepted
**Date:** 2026-08-12
**Amends:** nothing decided. It delivers a capability `PLAN.md` §2.1's W5 row promised and W5 shipped
without, which ADR-0127 §3 recorded as owed.

## Context

ADR-0127 §3 found six promises a completed wave had not kept. This was the sharpest, because the
machinery *looked* present:

`jr-diag` has carried `InstantiationFrame`, `Diagnostic::with_frame`, a `backtrace` field and a
`render_backtrace` renderer **since the vertical slice**. Its own doc comment explained why it was
defined so early — "retrofitting instantiation backtraces after the fact is a known failure mode
(`PLAN.md` §5)". W5 then shipped in fifteen sub-waves, and **nothing ever constructed a frame**.
`with_frame` and `InstantiationFrame::new` were called only by the renderer's own unit tests. So the
type existed, the tests passed, and no real diagnostic carried a backtrace — the pre-emptive work had
bought nothing, while making the feature look delivered to anyone grepping for it.

What a user saw instead: `add :: (a: $T, b: T) -> T { return a + b; }` called as `add(true, false)`
reported

```
error[E0223]: operator `+` is not supported for `bool`
 --> f.jr:2:12
```

pointing at the **template's** line — code the reader may never have opened, and which is correct for
every other instantiation of it. Nothing said which call produced `$T = bool`.

## Decision

**Every diagnostic produced while checking an instantiation's body is stamped with the call site that
demanded that instantiation.**

Three pieces, and the middle one is where the actual gap was.

### 1. The call site is recorded on the instantiation

`Instantiation` carried `template`, `bindings` and `comptime_values` — and **no span**. That absence,
not the renderer, is why this never shipped. It gains `site: Option<InstantiationSite>`, holding the
rendered frame and the `ExprScope` the demanding call sat in.

`jr-db`'s `instantiated_from` already walked `type_call_sites`, discarding the site and keeping the key;
it now keeps **one representative site per distinct key**. The first demand is the one recorded, because
a second call with the same bound types reuses the same clone — one body can carry only one backtrace —
and "first" is deterministic, which a snapshot depends on.

The span is the **call's**, never the template's, for ADR-0043's reason one level out: the template's
span is already the diagnostic's primary span, and the only thing that locates *this* user's mistake is
the call. A missing site yields `None` rather than a frame pointing somewhere plausible, because a
backtrace naming the wrong line is worse than none — a reader trusts it and stops looking.

### 2. Frames are attached by watermark, not threaded

`check_file`'s body loop records `Diagnostics::len()` before checking a body and stamps everything added
since. The alternative — passing a frame to each `push` — would touch hundreds of call sites that know
nothing about polymorphism, and be forgotten by the next diagnostic anyone adds. One call at the one
place that knows the body *is* an instantiation cannot rot that way.

`Diagnostics::attach_frames_since` is the single new API. A public `iter_mut` would also work and would
widen the sink for every consumer so one caller could stamp a field — the trade ADR-0123 refused when it
declined a `pub const CODES` for a test's convenience.

### 3. The walk is bounded

`instantiation_backtrace` follows `called_from` through the `BodyId → ProcId` map `check_file` already
builds, innermost frame first — the order the renderer prints and the order a reader wants: the thing
that broke, then why it was asked for. Capped at `MAX_BACKTRACE_FRAMES = 8`, like `MAX_OPT_ROUNDS` and
`MAX_INSTANTIATION_ROUNDS`, because a recursive template could otherwise cycle and a diagnostic path is
the worst place to hang.

## What this delivers, and what it does not

**Delivered**: a single frame, correct and pinned.

```
error[E0223]: operator `+` is not supported for `bool`
 --> bt1.jr:2:12
  |
2 |     return a + b;
  |            ^^^^^
  note: in instantiation of `add($T = bool)` (bt1.jr:5:10)
```

The description names the template **and** its bindings, because "in instantiation of `add`" would not
distinguish the `bool` call from a correct `s64` one beside it.

**Not delivered: a multi-level chain**, and the reason is worth recording because it is ADR-0120's
lesson recurring. The walking code is in place and bounded, but sites are harvested from the **first**
round's check — so a call written inside a template's body is attributed to the *template's* body, whose
owning procedure is not an instantiation, and the chain stops after one frame. Probed: a two-level case
reports `inner($T = bool)` and not the enclosing `outer($U = s64)`.

Fixing it means harvesting sites from the **final** expansion round, which is exactly what ADR-0120 had
to do for redirects — "an instantiation's body is a clone with its own `BodyId`, so a call site no base-tree
redirect could name". The same sentence applies to a site. It is deliberately **not** done here: it is a
change to which round the harvest reads, and bundling it with the wiring would make a regression in
either unattributable.

Two rendering defects fixed on the way. `binding_type_text` fell back to `?` for a builtin, because the
signatures know a *declared* type's name and a builtin has no declaration — so the first working frame
read `add($T = ?)`. And `Renderer::render` appended the backtrace without a newline, gluing the first
frame onto the caret line (`^^^^^  note: in instantiation of …`); `annotate-snippets` does not terminate
its output.

## Alternatives rejected

**Attaching the frame where the instantiation is *recorded*** (`check_call`), rather than where its body
is checked. The diagnostics do not exist yet at that point — the clone's body is checked in a later pass
over the expanded tree — so there would be nothing to stamp.

**Reporting the diagnostic against the call site instead of the template.** This makes the message point
at code that does not contain the error, and for a template used correctly elsewhere it would move a real
defect out of view. The primary span belongs to the failure; the call belongs in a frame.

**Recording the site on `proc_bindings`** instead of a new field, since it is already keyed by `ProcId`.
Rejected because that vector means "this variable is bound to this type" and is read by two phases that
have no use for a span; widening it would make both carry a field for a third consumer's benefit.

## Consequences

- Instantiation diagnostics gain a `note:` line naming the call and the bindings. Nothing else changes:
  **1009 tests before and after**, and no corpus expected-output moved — which incidentally proves that
  no corpus file had an instantiation diagnostic, so nothing was pinning this behaviour either way.
- `crates/jr-db/tests/integration.rs` gains
  `a_diagnostic_inside_an_instantiation_carries_its_call_site`, asserting both that a frame exists and
  that it names the template and binding. Teeth-checked: disabling the attach fails it.
- One new public method on `Diagnostics`, and one new public struct in `jr-hir`.
- `FileHir` gains `instantiation_sites`. Exhaustive struct initialisation caught all three constructors,
  which is the house rule doing its job.
- The multi-level chain is now **owed with a known fix** rather than unexamined, and `PLAN.md` §2.1's W5
  row can drop its `[NOT DELIVERED]` marker for backtraces while §7 carries the remaining half.

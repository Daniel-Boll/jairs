# ADR-0034: there will be no reverse index; the cost is parsing, not searching

- **Status:** Accepted
- **Date:** 2026-07-28
- **Deciders:** dboll
- **Amends:** ADR-0030's consequences, which named a reverse index as the fix if measurement
  found the reference scan too slow. Measurement found the scan is not the problem.

## Context

ADR-0030 declined to build a reverse index for `references` without a measurement. ADR-0033
took the measurement, and `PLAN.md` §7 then promoted the index to the top of the open list
with the words "the measurement exists and says build it."

That reading was wrong, and this ADR exists because the *next* measurement said so.

`jr bench` reported `references` and `rename` at **55 ms** cold on a 36 000-line, 302-file
workspace, against under 1 ms for every other operation. The hundredfold gap is real. The
inference — that the gap is the *search* — was not checked.

Two probe operations settle it. On the same tree:

| Operation | cold | what it does |
|---|---|---|
| `parse_all_files` | **31 ms** | lex + parse every workspace file, nothing else |
| `resolve_all_files` | **55 ms** | the above, plus lower and resolve each file |
| `references` | **55 ms** | the above, plus the traversal and the matching |

So the budget is roughly **31 ms parsing (56%), 24 ms lowering and resolving (43%), and
0.5 ms doing the thing a reverse index would replace (1%)**. The 0.5 ms figure is not
inferred by subtraction alone — it is directly visible as the **warm** row, where the files
are already parsed and resolved and `references` still costs 0.53 ms.

A reverse index would have optimised the one percent.

## Decision

### 1. No reverse index. `references` is bounded by `resolved`, and that is where any work goes

The scan is O(name occurrences) over already-computed `ResolveMap`s, and it is already
cheap: 108 108 name visits across a `jr bench` run, at a total of half a millisecond per
request once the inputs exist. There is nothing to index.

**Rejected: build the index anyway, because the ADR that deferred it named it.** This is the
tempting one, and it is exactly the failure mode `AGENTS.md` calls "plans that contradict
themselves": §7 had already been rewritten to say *build it*, so building it would have felt
like following the plan. A plan is not evidence. The index would have added an invalidation
problem ADR-0030 explicitly warned about — "building one means invalidating one" — in
exchange for 1% of one request.

**Rejected: memoise the scan as a salsa query keyed on `DefId`.** Cleaner than a hand-rolled
index and it would make repeat requests free. Rejected for the same reason: repeat requests
are *already* 0.5 ms, and `DefId` holds a `PathBuf`, so making it a salsa key means interning
paths and inventing an invalidation story for a value that is cheap to recompute.

### 2. What the cost actually is, and why it is not a bug

The first whole-workspace request parses the whole workspace. ADR-0029 §3 said so in advance
and called it the price of discovering paths rather than loading files. The measurement
confirms the prediction rather than contradicting it: `workspace_load` is 41 ms on the same
tree, and it is the same work.

This is a **cold-start** cost, paid once per session per file set, not per request. After it,
`references` is 0.53 ms warm and 0.10 ms after an edit — both comfortably interactive. An
editor pays 55 ms on the first find-references of a session and nothing like it again.

Left as is, deliberately. The alternatives are all worse today:

**Rejected: parse the workspace eagerly at `initialize`.** It moves the same 55 ms to
startup, where it competes with the first `didOpen` and the first diagnostics — the requests
a user is actually waiting on. ADR-0029 §3 chose "on the first caller that needs it" and the
measurement gives no reason to revisit that.

**Rejected: parse workspace files in parallel.** The honest optimisation, and the one to take
*if* this ever needs taking: 302 independent parses is an embarrassingly parallel problem, and
salsa supports concurrent reads from snapshots. Rejected now because 55 ms once per session is
not a complaint anyone has made, and because parallel parsing interacts with the pool `Mutex`
(ADR-0016's side-channel) in ways that need their own investigation. Recorded here so the next
person measuring this does not start from scratch.

### 3. `jr bench` keeps the two probe operations

`parse_all_files` and `resolve_all_files` are not requests any client sends. They stay in the
table anyway, because they are what turns "references is slow" into "parsing is slow" — and
that distinction is the entire content of this ADR. A future wave that finds the number has
grown needs the same split available without rebuilding it.

Their presence is also the answer to the obvious objection: a benchmark that only measures
end-user operations can tell you *that* something is slow and never *what*.

## Consequences

- **ADR-0030's reverse index is closed, not deferred.** It should not reappear on an open list
  unless a measurement shows the traversal itself costing something, which would mean a
  workspace far larger than 302 files or a name occurring far more than 108 000 times.
- **The open-work list gets shorter for the right reason.** §7 had this at the top; it is now
  a recorded decision. That is the opposite of the "handoff rots toward what remains is small"
  pattern — the item is removed because it was answered, not because it was forgotten.
- **`AstIdMap` (ADR-0013) is unaffected and still deferred.** ADR-0033 measured it separately.
- **Parallel parsing is the live lead** if this cost ever matters. Named in §2 with the
  obstacle attached.
- **The numbers are one machine and one synthetic tree.** A 36 000-line workspace is far larger
  than anything Jairs has, and the conclusion is a ratio (1% vs 99%) rather than an absolute,
  which is what makes it robust to the machine.

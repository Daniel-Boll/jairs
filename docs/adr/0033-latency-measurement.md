# ADR-0033: `jr bench`, and why a benchmark harness would have measured the cache

- **Status:** Accepted
- **Date:** 2026-07-27
- **Deciders:** dboll

## Context

Five features and three ADRs now wait on one number that has never been taken.

- **ADR-0013** deferred `AstIdMap` and named its own revisit trigger: "incremental
  re-analysis cost is **measured** and whitespace-edit invalidation is a real cost — not
  assumed to be. The natural trigger is the language server, where keystroke latency is
  directly observable." The language server has existed for four waves.
- **ADR-0030** declined a reverse index for references without measurement.
- **ADR-0031 §5** declined an exported-name index for auto-import for the same reason, and
  noted auto-import is the most keystroke-adjacent claimant: an editor asks for a code action
  whenever the cursor sits on a diagnostic.

`PLAN.md` §7 has carried this under "also open, and smaller" for three waves while the list
of things blocked on it grew. It is not smaller. Every one of those deferrals is defensible
*only* while the measurement is absent — the moment it exists, three decisions become
answerable, and until it exists they are guesses wearing a citation.

Nothing in this repository has ever been benchmarked against anything.

## Decision

### 1. A `jr bench` subcommand, not a `criterion` harness

**Rejected: `criterion` (or `divan`) with `[[bench]]` targets.** The obvious choice, and
wrong here for a specific reason: **a benchmark harness would measure the memo table.**

Criterion's entire method is to run the same closure many times and take a distribution.
Under salsa (ADR-0007) the second call to `hover` on an unedited file does no work at all —
it reads a memoized value. So the reported number would be the cost of a hash lookup, the
variance would be tiny, and the result would look authoritative and be meaningless. Worse,
it would answer ADR-0013's question *backwards*: the invalidation cost that ADR is about is
precisely what a warm cache hides.

Measuring this correctly requires controlling the cache **per iteration** — a fresh database
for a cold number, and a real edit before each warm one. That is not a closure a harness can
time; it is a script. So it is a subcommand:

```
jr bench [--iterations N] [--module-path DIR] FILE
```

The cost of the choice, stated: no statistical machinery, no historical comparison, no
outlier detection. What comes back is min / median / p95 over N iterations, which is enough
to answer "is this milliseconds or hundreds of milliseconds" — the question all three
deferrals actually turn on. If a decision ever hinges on a 5% regression, that is the day
this needs a real harness, and it is not today.

**Rejected: measure inside the running server over the LSP transport.** Closest to what a
user feels, and it would include framing and scheduling. Rejected because it measures three
things at once and cannot separate them, and because the thing under investigation is query
invalidation, not I/O. The handler functions are pure functions of `(&db, params)` by
ADR-0024 §4 precisely so they can be called directly.

### 2. Three regimes per operation, because the difference between them *is* the finding

Each operation is measured in up to three states:

- **cold** — a fresh `JairsDatabase` per iteration, everything computed from scratch. The
  worst case, and what the first request after opening a project pays.
- **warm** — the same database, no edit. This is the memo-hit path, and it is reported not
  because it is interesting but because it is what a naive benchmark would have reported
  *as if* it were the answer. It is the control.
- **after-edit** — the same database, with a **whitespace-only** edit applied before each
  iteration. This is ADR-0013's question stated as an experiment: if HIR embedded stable
  ids instead of absolute spans, this column would collapse toward *warm*; because it embeds
  spans, an edit at the top of the file invalidates every node below it.

A whitespace edit specifically, and at the **top** of the file, because that is the edit
whose semantic effect is zero and whose invalidation is total. Any other edit conflates real
re-analysis with the span churn under test.

### 3. What is measured, and why each one

| Operation | Why it is here |
|---|---|
| `diagnostics` | ADR-0013's named trigger: keystroke to squiggle |
| `hover` | The cheapest real request; the O(nodes) `locate` scan's floor |
| `completion` | Runs on nearly every keystroke in an identifier |
| `code_action` | ADR-0031 §5's claimant — parses discovered modules on request |
| `references` | ADR-0030's claimant — a workspace scan |
| `rename` | The same scan plus edit construction |
| `workspace_load` | The cost ADR-0029 §3 promised would land on the first caller |

`references`, `rename` and `workspace_load` are measured **cold only**. Warming them would
mean holding a database across iterations that the workspace scan has already populated,
which measures nothing: the interesting number is the one the first caller pays, and the ADR
that deferred the index says so.

### 4. The number is printed, never asserted

`jr bench` has **no** pass/fail threshold and no test asserting a duration. A timing
assertion on a shared CI machine is a test that fails for reasons unrelated to the code, and
this project's gates are meant to be believable. What *is* tested is that the subcommand runs
and produces a finite number for every operation — a smoke test, so the harness cannot rot
into something that reports zeros.

Consequently `jr bench` is **not** a seventh gate. It is a tool you run when a decision needs
it, in the same category as `editors/nvim/verify.lua`: verified, not gated.

## Consequences

- ADR-0013, ADR-0030 and ADR-0031 §5 become answerable. Whether the answers change anything
  depends on the numbers, and this ADR deliberately does not predict them — recording a
  prediction here would be the same mistake as the assumption it exists to replace.
- The measurement is only as honest as the file it is given. A three-hundred-line corpus file
  is not a project, so the numbers below are a floor, and `jr bench` takes a path precisely so
  a bigger tree can be pointed at it later.
- `jr bench` is a *sixth* subcommand that loads the workspace, which means it exercises
  ADR-0029's discovery on real trees and would notice a regression there.
- **No dependency added.** `std::time::Instant` is sufficient, and ADR-0009 makes adding a
  dependency a deliberate act rather than a convenience.

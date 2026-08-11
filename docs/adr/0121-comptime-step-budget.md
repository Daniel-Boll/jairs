# ADR-0121 — Compile-time execution has a step budget

**Status:** Accepted
**Date:** 2026-08-07
**Amends:** ADR-0006 (which bounded what compile-time code may *call*, not how long it may *run*).

## Context

The audit at `354d900` ([`docs/assessment-2026-08-07.md`](../assessment-2026-08-07.md), finding F3) found
that the bytecode interpreter had **no step budget, no fuel and no timeout**. The only bound was
`MAX_DEPTH = 256` on recursion, which catches an infinitely *recursive* comptime program and nothing else.

So this hung the compiler:

```jairs
spin :: () -> s64 { n := 0; while true { n = n + 1; } return n; }
HANG :: #run spin();
```

`jr check` never returned. There was no diagnostic and no way out but a signal.

The blast radius is much larger than "the compiler is slow on a silly program". `file_consts` calls the VM
inside a salsa query, and the loop makes no database reads — so **salsa's cancellation can never reach it**.
Under `jr lsp` that hangs the single worker thread (`jr-lsp/src/server.rs:716-729`), and the job channel is
unbounded, so it then grows with every keystroke. The user did not run a compiler: they **opened a file in an
editor**.

That is worth stating plainly because it inverts the usual reading. Compile-time execution is a *feature* —
`#run`, `#insert`, `#modify` predicates and `noted_insert` all execute attacker-authored code by design, and
ADR-0006 already decided what such code may call. What ADR-0006 did not decide is how long it may take, and
"forever" is not a defensible answer for a compiler, still less for a language server.

## Decision

### 1. A step budget, checked in the dispatch loop

`Vm` carries a `fuel` counter, decremented once per instruction in `run_instrs` — the one place every
instruction passes through. Exhaustion is `VmError::Exhausted("steps")`, which `jr-db`'s const-eval already
renders as **E0230**, so no new diagnostic code was needed and the existing message reads correctly:
`compile-time evaluation failed: the compile-time interpreter ran out of steps`.

Counted **per VM rather than per frame**, so a loop that calls a procedure a billion times is bounded too —
a per-frame budget would bound only the shape of the failure, not its cost.

### 2. `MAX_COMPTIME_STEPS = 10_000_000`

Ten million. Far past any constant a real program folds — the entire corpus's compile-time work is orders of
magnitude below it — and well under a second on the interpreter, which is the number that matters for an
editor.

Both halves are pinned by a test. `a_non_terminating_compile_time_loop_is_refused` checks that the budget
*bites*; `a_long_but_terminating_compile_time_loop_still_folds` checks that a hundred thousand iterations
still folds, so the budget cannot be quietly lowered until it breaks legitimate work with nothing noticing.

### 3. Only compile-time execution is metered

Under `Mode::Runtime` the counter starts at `u64::MAX`, which is effectively unmetered.

This is the load-bearing half of the decision. Under `jr run` the interpreter is executing the *user's own
program*, where a long loop is the program working rather than the compiler hanging — metering it would
refuse legitimate work and make `jr run` a worse engine than the native back end for no reason. The two
engines must agree on what a program *computes*; they need not agree on how patient they are.

So the budget bounds **compilation**, which is the thing nobody asked to be unbounded.

Rejected: *a wall-clock timeout.* It would make compilation non-deterministic — the same program could fold
on a fast machine and fail on a slow one, and the two engines' agreement is this project's central invariant
(ADR-0019). A step count is a property of the program.

Rejected: *a budget proportional to program size.* It sounds principled and is not: the relationship between
source size and folding cost is exactly what a loop breaks.

## Consequences

A non-terminating `#run` now reports E0230 in under two seconds instead of hanging, and `jr lsp` cannot be
wedged by opening a file. `jr run` is unchanged.

Test count 988 → 990.

**What this does not fix.** The LSP's job channel is still unbounded
(`jr-lsp/src/server.rs:721`), which was the *other* half of the hang — benign now that the worker cannot
wedge, but a bound there is its own small change. And a `#run` that allocates without looping is still
bounded only by the VM's 1 MiB region, which is a separate limit that already reports.

The budget is also **not** a security boundary in the sense of sandboxing: compile-time code still calls what
ADR-0006 permits, still reads what the pool holds, and — until the `BUILD_OUTPUT` confinement lands — still
chooses where `jr build` writes. This bounds *time*, and says so.

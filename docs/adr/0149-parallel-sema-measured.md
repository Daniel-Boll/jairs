# ADR-0149: Parallel sema, measured and refused — and the pool becomes an `RwLock`

- **Status:** Accepted
- **Date:** 2026-09-01
- **Deciders:** dboll
- **W8 sub-wave 8, and it closes W8.** §2.1 lists "parallel Sema + parallel codegen" in this wave's
  content. This ADR measures it, **declines to ship it**, and names precisely what it is blocked on.
  The one change that does land is the pool becoming an `RwLock` — which did *not* make anything
  faster, and is kept for a different reason recorded in §1.
- No design fork was put to the decider; the recommended approach was taken and the rejected
  alternatives are recorded below, per the session directive.

## Context

### The mechanism was already there, which is why this looked easy

`salsa::Storage` is `Clone`, `JairsDatabase::snapshot` has existed since ADR-0024 for the LSP, and
every field of the database is an `Arc` or a mutex. `jr-base`'s `Interner` has been an
`Arc<ThreadedRodeo>` since the first wave, with a comment saying "because parsing and semantic
analysis are intended to run in parallel". The lock discipline — *never hold a pool guard across a
nested query call* — was already written down and already followed at all sixteen sites.

So this was not an architecture change. A parallel `jr check` is about sixty lines in one driver
function: build the deduped file list, compute `file_diagnostics` across `std::thread::scope` workers
each holding a snapshot, then emit in list order so the output stays byte-identical.

It was written, it worked, its output was byte-identical at 1, 2, 4, 8 and 12 threads — and then it
was measured.

### What the measurements say

All on a 12-core M2 Pro, best of several runs, against this project's own corpus.

**In-process, the phase that was parallelised:**

| threads | wall (119 files) |
|---|---|
| 1 | 74 ms |
| 2 | 64 ms |
| 4 | 56 ms |
| 8 | 54 ms |
| 12 | 53 ms |

1.39x, saturating at four threads. Pre-warming the shared modules first changed nothing, so this is
not the shared-dependency fan-in.

**Why it saturates.** Instrumenting the pool guard: **571 acquisitions** for 119 files — the lock is
coarse, exactly as the discipline requires — holding the pool for **~30 ms of the 74 ms**. So about
**40% of a single-threaded check runs inside the pool's exclusive critical sections**, and Amdahl
bounds any driver-level parallelism at `1 / 0.4` = **2.5x**. The measured 1.39x is the rest of the gap:
salsa blocks one worker while another computes a shared module's queries.

**At the process level, which is what a user experiences:**

| input | 1 thread | 12 threads | speedup |
|---|---|---|---|
| `tests/corpus/valid` (119 files, clean) | 132 ms | 110 ms | **1.20x** |
| `tests/corpus/type-errors` (75 files) | 32 ms | 31 ms | 1.02x |
| both | 179 ms | 177 ms | 1.01x |

Amdahl a second time, and this is the number that decides the wave: the parallelised phase is itself a
*fraction of the command*. Reading 194 files, `load_modules_transitively`, the one-shot `source_map()`
clone, and rendering every diagnostic are all serial, and on a tree with errors the rendering dominates.
The floor is not the compiler at all — the process costs 10 ms to start.

### The build path, and a probe that measured the wrong thing

The first attempt at parallel codegen ran `build_object` for each of the 119 corpus files
concurrently and reported 84% of wall time inside the pool guard. That number is real and the
conclusion drawn from it was wrong: `build_object` compiles *every reachable file* of one root, so
119 roots is 119 whole-program compilations. It was measuring **duplicated work**, not contention —
and `jr build` builds one root.

The honest statement about parallel codegen is therefore not "it is slow" but: **this project has no
program large enough to measure it on.** The biggest is four files. A per-file codegen fan-out over a
four-file program is a thread pool spun up to do three units of work.

## Decision

### 1. The pool becomes an `RwLock`, and this is not a performance change

`Mutex<Pool>` → `RwLock<Pool>`, `lock_pool` returns the write guard, and a new `read_pool` returns the
read guard. Rust identified all six read-only sites inside `jr-db` for free: they were already spelled
`let pool` rather than `let mut pool`, so the read/write split was a fact the code stated and the type
did not. The pool is append-only and idempotent — its own docs say so, as the reason mutating it inside
a tracked query is harmless — which is exactly the property that makes shared reads sound.

**It made nothing measurably faster, and that is stated rather than glossed.** Check's pool use is
dominated by interning, which is a write. A change kept anyway needs a different justification, and
here it is: the conversion turned eight hand-rolled
`db.pool().lock().unwrap_or_else(|e| e.into_inner())` sites in `jr-lsp` into compile errors, and they
are now one `Db::read_pool` with the poison recovery in one place. `run.rs`'s module docs already
described that recovery as deliberately centralised while four files quietly re-implemented it. Two
copies of one rule is this project's standard definition of two chances to disagree.

**Rejected: keeping the `Mutex`.** It would leave the duplication, and it would leave the type unable
to say which sites intern — which is the fact §3's ceiling is about.

### 2. Parallel `jr check` is written, measured, and **not shipped**

The driver change is reverted. The reasons, in the order they carry weight:

- **1.20x at best, 1.01x on a mixed tree.** Against a 2.5x ceiling that a driver cannot lift.
- **It buys a latent failure mode that only appears under threads.** A `std::sync::RwLock` is neither
  reentrant nor upgradable, so any future query that takes a pool guard and then calls a nested query
  deadlocks. That discipline exists and is documented — and `run.rs` carries a comment about the time
  it was broken and "the program hung rather than failing, which is worse". A single-threaded compiler
  makes that mistake into a hang one developer sees immediately; a threaded one makes it into a hang
  that depends on timing and thread count.
- **A flag would be dead code and a default would be a tax.** `--threads` defaulting to 1 is a
  feature nobody runs; defaulting to auto ships thread scheduling into every `jr check` for 4% on the
  input that matters.

ADR-0058 §3's rule — a directive that is silently ignored is worse than one that is rejected — is
about surfaces, and the same reasoning applies to a *capability*: parallelism that is present, on by
default, and worth 1% is a claim the code makes and the measurements do not support.

**Rejected: shipping it anyway because §2.1 lists it.** A plan item is a hypothesis. This one was
tested and did not hold on this architecture, and recording that is the deliverable — the same shape
as ADR-0146 §4 refusing to claim a program-speed number this project cannot measure.

### 3. What would actually lift the ceiling, named as future work

Two things, in order:

1. **Finer-grained interning.** 40% of check inside the pool's exclusive sections is the binding
   constraint. The sections are few (571) and long (~53 µs each), because the pattern is *gather every
   query result, then lock and do all the work* — which the nested-query rule forces. Splitting them
   means interning under short locks at high frequency, or sharding the pool. Both change how
   `PoolId`s are assigned, which is ADR-0015's identity model and ADR-0018 §2's single layout
   computation. **Its own wave, with its own ADR.**
2. **An input large enough to measure on.** Neither parallel codegen nor a serious parallel check can
   be evaluated against 119 files averaging thirty lines, and a benchmark that cannot distinguish the
   change from noise is not evidence. ADR-0146 built the throughput harness; what it lacks is a large
   input, and generating one is a decision about what "representative" means.

Until both exist, parallelism is a change whose benefit cannot be demonstrated — which is the one kind
of change this project's own rules say not to make.

## Consequences

- **W8 closes with seven of eight sub-waves shipped and one measured and refused.** The refusal has
  numbers behind it, which is the difference between a decision and a shrug.
- **`jr-lsp` no longer hand-rolls poison recovery**, and `Db::read_pool` is the one way to read the
  pool from outside `jr-db`.
- **The nested-query lock rule is now load-bearing in the type system**, not just in prose: a site
  that interns needs the write guard and says so.
- **No new dependency.** The parallel driver used `std::thread::scope`; no thread pool was added, and
  none is now needed.
- **The two blockers above are the honest content of a future "performance" wave**, and neither is a
  driver change. Anyone reaching for parallelism again should start by re-running the two measurements
  in this ADR's Context, because the first number to move must be the 40%.

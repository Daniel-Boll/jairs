# ADR-0146: A compile-throughput number, and the faster sort it justifies

- **Status:** Accepted
- **Date:** 2026-09-01
- **Deciders:** dboll
- **W8 sub-wave 5.** §2.1 names "published compile-throughput number" as this wave's content, and
  ADR-0104 §3 chose insertion sort with the words "a faster algorithm is W8's, with a benchmark
  behind it". The two are one sub-wave because the second is the first's first customer.
- No design fork was put to the decider; the recommended approach was taken and the rejected
  alternatives are recorded below, per the session directive.

## Context

### Everything in this wave so far has been a guess about performance

`MAX_INLINE_STATEMENTS = 24` says of itself that it "is a guess and has never been measured".
ADR-0145 added three more (`MAX_INLINE_ROUNDS`, `MAX_INLINED_STATEMENTS`, `MAX_FORWARD_HOPS`), each
labelled a guess for the same reason. ADR-0021 §4 named the missing input precisely: "the performance
number that would justify a real threshold is downstream of the wave that introduced this pass".

That number does not exist yet, and until it does every remaining W8 item — SIMD, parallel sema,
parallel codegen — is a decision about speed taken without a way to tell whether it worked. So this
sub-wave comes before them rather than at the end of the wave, where §2.1 lists it.

### Why `jr bench` cannot answer it as it stands

ADR-0033 built `jr bench` for a different question: per-*request* language-server latency, in three
cache regimes, over one file. Its whole design is about controlling salsa's cache per iteration,
because "the *second* call to `hover` on an unedited file does no work at all".

Compile throughput is the opposite shape. It is one cold pass over *many* files, and there is no warm
regime to control because a compiler run is a process: the second run starts with an empty database
by construction.

### What ADR-0104 §3 promised and what it costs to keep

`modules/Sort` is insertion sort, `O(n²)`, chosen for three stated reasons — stable, no storage, short
enough to read — and one deferral: a faster algorithm needs a benchmark. Keeping that promise means
*measuring* rather than asserting, and it means deciding what happens to stability, which is
observable behaviour and not a quality of implementation.

## Decision

### 1. `jr bench --throughput` — a mode of the existing subcommand

`jr bench --throughput <PATH>…` measures compilation over a file set, where a `PATH` may be a
directory exactly as `jr check`'s may.

**A mode rather than a new subcommand**, because it is the same activity under the same contract:
measure, report, **never judge** (ADR-0033 §4). A second subcommand would be a second place for that
contract to be stated and a second place for someone to add a threshold to it. The existing
`jr bench <FILE>` behaviour is unchanged when the flag is absent.

**Two operations are timed**, and both end where a real command ends:

- **check** — every diagnostic for every reachable file, which is `jr check`'s work.
- **build** — through MIR and a back end into an object, which is `jr build`'s work minus the link.
  The link is excluded because it is `cc`, not this compiler.

**Cold only, and this is stated because the existing bench has three regimes.** There is no warm
throughput: a compiler is a process, so the number a user experiences is always the cold one. A warm
figure would measure a memo table and would be exactly the misleading answer ADR-0033 §1 was written
to avoid.

**Reported as lines per second and bytes per second**, plus the wall time and the file count.

- **Lines**, because that is the unit a person compares compilers in.
- **Bytes as well**, because a line is a formatting artefact — this project's own corpus has files
  whose lines are mostly prose in `//!` comments, and a lines-per-second number over them flatters
  the lexer.
- **Both, rather than picking one**, so that a reader who distrusts either has the other.

**Rejected: a `criterion` benchmark.** ADR-0033 §1's argument applies unchanged, and more strongly:
`criterion` would want to run the closure many times in one process, which measures a warm database.

**Rejected: measuring `jr check` as a subprocess and timing the process.** It includes process
start, argument parsing and diagnostic *rendering*, none of which is compilation, and it cannot
separate check from build.

### 2. The number is published with its machine, and it is not a gate

The figure goes in `README.md` with the machine, the toolchain and the input set named beside it. A
throughput number without a machine is not a number, and one whose input set is unstated cannot be
reproduced.

**It is not a gate**, extending ADR-0033 §4's rule rather than making an exception to it: "a timing
assertion on a shared machine fails for reasons unrelated to the code, and this project's gates are
meant to be believable". The command is documented in `AGENTS.md` beside the Neovim script, under the
same "verified, not gated" heading.

### 3. `Sort` gains `heap_sort`; `sort` keeps its stability

`heap_sort(xs, less)` is added: in place, no allocation, `O(n log n)`, and **unstable**. `sort` stays
insertion sort and stays stable.

**Why not replace `sort`.** Stability is *observable behaviour*, not an implementation quality: with
equal keys, a stable and an unstable sort produce different permutations, and a program that sorts by
one field after sorting by another depends on it. ADR-0104 §3 chose insertion sort partly for
stability, so silently swapping the algorithm would change what an existing program computes — the
class of change this project refuses to make quietly. Two names is the Rust precedent (`sort` and
`sort_unstable`) and it lets a caller state which property they need.

**Why heapsort rather than quicksort or a merge sort.** A merge sort needs scratch storage, and
allocation is the decision ADR-0103 §3 declined to take and this ADR has no reason to reopen.
Quicksort is faster in practice and has a worst case that is `O(n²)` on adversarial input plus a
pivot choice to argue about. Heapsort is `O(n log n)` *always*, needs no storage, and is short —
which are the same three criteria ADR-0104 §3 used, with the asymptotics moved from third place to
first.

**Rejected: a hybrid — insertion below a threshold, heapsort above.** It is what a production sort
does and it makes stability depend on the *input size*, which is the worst of both: a program that
works on small inputs and silently reorders equal keys on large ones.

### 4. The sort comparison is measured in **comparisons**, not in seconds

`heap_sort` and `sort` each take an optional counter — a `*s64` incremented per comparison — and a
corpus program asserts that heapsort performs strictly fewer comparisons than insertion sort on a
reversed input, and that both produce a sorted result.

**This is a better measurement than a timing, for this project specifically.** A comparison count is
deterministic, machine-independent, and identical in all three engines — so it is a *test* that runs
in the differential harness rather than a number that needs a footnote about the machine it was taken
on. And it measures the thing the decision is actually about: the asymptotics, not the constant
factor.

Wall-clock timing of Jairs *programs* is deliberately not attempted. It needs a clock in the
language, which means a `#foreign` binding, a decision about monotonic versus wall time, and a unit
— a real sub-wave, and one nothing else is waiting for.

**Rejected: asserting an absolute comparison count.** It would pin the algorithm's exact schedule, so
any future tuning would fail a test for no reason. The assertion is the *inequality*, which is what
the choice rests on.

**Rejected: counting through a global or the context.** A counter parameter is explicit at the call
site, and the context is a callee facility whose whole point is that a caller need not thread it
(ADR-0057) — using it to return a measurement would invert that.

## Consequences

- **`jr bench` gains a flag and a mode**; its single-file behaviour is untouched, and the "reports,
  never judges" contract is stated in one place still.
- **A published number in `README.md`**, with the machine, the toolchain and the input set. It will
  rot, and it is dated for exactly that reason — a stale number that says when it was taken is
  usable; one that does not is a lie.
- **`modules/Sort` gains `heap_sort` and `heap_sort_ints`**, plus the counter parameter on both
  sorting routines. The wrappers exist for ADR-0104 §5's reason, unchanged: cross-file instantiation
  is still deferred, so an importer can only call a wrapper this module instantiated.
- **Every guessed constant in the mid-end now has a way to be checked**, which is what makes them
  tunable rather than permanent. Tuning them is not this sub-wave: the number has to exist first, and
  a change to a threshold in the same commit as the tool that measures it would have nothing to
  compare against.
- **Deliberately not done**: a clock in the language (§4), wall-clock timing of Jairs programs, a
  throughput *gate*, and any change to the mid-end's constants.

### The number, and two things found while taking it

**Measured on this machine** — Apple M2 Pro, macOS (Darwin 25.6.0), rustc 1.94, a `--release`
compiler — over `tests/corpus/valid` with `modules/` on the search path: **116 files, 9 203 lines,
360 982 bytes**, best of ten:

| operation | best | lines/s | bytes/s |
|---|---|---|---|
| check | 81 ms | 113 103 | 4 436 403 |
| build | 356 ms | 25 864 | 1 014 481 |

A debug compiler (this workspace's `[profile.dev]`, `opt-level = 1`) manages 87 460 and 19 230 —
worth recording because every gate runs the debug binary, so that is the number a contributor sees.

**`build` is 4.4× the cost of `check`**, which is the most useful thing the table says: the front end
is not where the time goes, so the remaining W8 items that would speed *it* up have less to win than
they appeared to.

Two findings, both from writing `heap_sort` and both recorded in `PLAN.md` §7 rather than fixed here:

- **A `$T` template cannot call another `$T` template**, even with the variable already bound:
  `sift_down(xs, …)` where `xs: []T` inside `heap_sort` is **E0268**, "cannot infer every `$T`".
  Heapsort is therefore written as one loop with a single sift site — which is a better shape anyway,
  since the alternative was writing the sift twice — but the limitation is a real one and adjacent to
  ADR-0104 §5's cross-file refusal rather than the same thing.
- **A file-level mutable variable leaks an internal error.** `counter := 0;` at file scope checks
  clean and then fails in lowering with "the compiler could not lower `main` … this compiler has a
  gap — please report it" — the **eighth** occurrence of this project's most-recorded failure shape,
  found by probing for a way to count comparisons without a parameter. It wants either a real
  refusal or mutable static data, and the latter is W6's static-data table, so it is recorded for
  that wave rather than guessed at here.

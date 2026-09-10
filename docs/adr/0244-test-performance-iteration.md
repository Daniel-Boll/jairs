# ADR-0244: Test iteration uses an isolated cache, measured lanes, and parallel Cargo proof

- **Status:** Accepted
- **Date:** 2026-09-10
- **Deciders:** dboll
- **Amends:** ADR-0209's full-lane implementation while preserving its coverage and
  Cargo/libtest authority. The ordinary gate remains every workspace test plus every doctest.

## Context

ADR-0209 made the suite practical by sharding two monolithic properties and adding nextest feedback
lanes. The corpus then grew to 327 files and the warm authoritative gate regressed to **183.07
seconds**, despite compilation taking only **0.59 seconds**. The `jr-cli` differential target alone
took **76.47 seconds**.

The local Cargo cache had also become pathological:

| Target directory | Size | Files in `debug/deps` | Warm nextest discovery |
|---|---:|---:|---|
| shared `target/` | 45 GiB | 494,084 | repeatedly killed after 20–33 s |
| clean measured target | 1.8–2.3 GiB | 3,494 | completes normally |

Changing full debug information to line tables only reduced a clean test target by about 0.5 GiB
and did not materially change warm execution. The large win is therefore avoiding an unbounded
mixed-purpose cache and exposing independent test phases to the machine, not adding another
compiler cache.

The four ordinary differential shards were measured at 44.25–45.83 seconds under nextest. Their
work is already balanced; cost-weighted assignment would add history and invalidation machinery
without addressing the remaining scheduling limit.

## Decision

### 1. Nextest feedback gets a dedicated target directory

`scripts/check fast`, `pre-commit`, and `measure` use `target/nextest` by default, while respecting
an explicit `CARGO_TARGET_DIR`. This keeps test-runner discovery away from stale artefacts produced
by unrelated builds, features, and profiles.

The script exposes cache information and an explicit cache-cleaning operation. Cleanup remains a
deliberate command rather than an automatic side effect: rebuilding a healthy cache on every run
would trade a rare pathology for guaranteed delay.

### 2. Test artefacts retain line tables, not full debug information

The workspace test profile uses `debug = "line-tables-only"`. Failing tests keep file/line
backtraces while test binaries and incremental state carry less debug data.

This is a storage and link-hygiene choice, not the headline run-time speedup. The measured warm
difference was under one second.

### 3. Feedback, measurement, and proof remain distinct

- `fast` skips the differential and compatibility binaries and the exhaustive prefix sweep.
- `pre-commit` skips every exhaustive corpus-wide sweep, including the O0/O1 equivalence sweep
  that ADR-0209's original filter accidentally retained.
- `measure` runs nextest with JUnit output so slow-test decisions can be based on checked timings.
- `full` remains the authoritative Cargo/libtest evidence.

Nextest does not replace the gate. Its process-per-test execution can mask process-global coupling,
and it does not execute doctests.

### 4. The full lane compiles once, then runs normal tests and doctests concurrently

The authoritative runner first performs Cargo's normal `--no-run` compilation. It then runs:

1. `cargo test --workspace --lib --bins --tests --examples`; and
2. `cargo test --workspace --doc`

as concurrent child processes, waits for both, and fails if either fails. Their union is the same
ordinary workspace evidence as `cargo test --workspace`: the runner changes orchestration, not
selection or semantics. Cargo/libtest still owns each half.

This amends ADR-0209's requirement that `scripts/check full` be textually the one Cargo command.
That spelling had made independent doctest compilation/execution extend the critical path after all
normal targets had finished.

### 5. Differential parity remains at four shards

The proposed increase from four to eight deterministic VM/Cranelift shards was measured in the new
full runner and rejected: the differential target regressed from **76.47 seconds to 145.72
seconds**. Cargo/libtest launched eight linker-heavy shards together, creating resource contention
instead of useful parallelism. Both the ordinary and LLVM comparisons therefore remain at four.

### 6. CI caches only where reuse can repay restore cost

Formatting does not restore a Rust cache. Rust commands use the lockfile. The corpus-drift job uses
the repository's pinned npm tree-sitter CLI, caches npm inputs rather than a Rust target, and does
not repeat the Rust corpus test already covered by the full test jobs. CI's test job calls the same
authoritative full runner as local development.

## Rejected alternatives

### Replace the full gate with nextest

Rejected because it changes test process semantics and omits doctests.

### Add sccache

Rejected for the inner loop. Warm compilation is below one second, while sccache disables Rust
incremental compilation for cacheable crates and cannot cache final system-link steps. It may be
reconsidered for measured clean CI builds, not installed speculatively.

### Cache test results

Rejected because filesystem state, subprocesses, generated binaries, environment variables, and
graphics-driver state are test inputs that Cargo does not model as a safe result-cache key.

### Disable or convert doctests

Rejected because documentation examples compiling is part of the gate. Running their Cargo phase
beside normal tests removes critical-path delay without removing evidence.

### Share or cache the entire target directory indefinitely

Rejected by measurement: the shared directory reached 45 GiB and nearly half a million dependency
artefacts, at which point nextest discovery itself stopped completing.

### Increase or cost-weight the differential shards

Rejected because the measured four shards differed by less than four percent, while eight shards
made the target 90% slower. The bottleneck is the resource limit around native compilation and
linking, not uneven assignment.

## Consequences

- Inner-loop nextest discovery has a bounded, purpose-specific cache.
- The pre-commit lane now omits exactly the exhaustive properties it claims to omit.
- JUnit timings make the next optimization evidence-driven.
- The ordinary and LLVM test counts remain unchanged.
- The full gate remains Cargo/libtest plus rustdoc, but can overlap its two independent execution
  phases.
- Cache cleanup is available and explicit rather than becoming recurring hidden work.
- The research and baseline measurements are retained in
  `docs/research/test-performance-primary-sources.md`.

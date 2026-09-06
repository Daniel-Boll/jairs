# ADR-0209: Fast local lanes supplement the full test gate, and exhaustive engine sweeps are sharded

- **Status:** Accepted
- **Date:** 2026-09-06
- **Deciders:** dboll
- **Amends:** the local testing guidance in `README.md`, `CONTRIBUTING.md` and `AGENTS.md`; it does
  not amend the six-gate requirement or replace `cargo test --workspace`.

## Context

The default test gate had become too slow to use as ordinary feedback. A warm-cache measurement on
the development machine separated compilation from execution:

| Command | Scope | Wall time |
|---|---|---:|
| `cargo test --workspace --lib --bins --tests --examples` | 1212 non-doctests | 177.14 s |
| `cargo nextest run --workspace` | the same non-doctest targets | 145.57 s |
| `cargo test --workspace --doc` | four doctests | 45.45 s |

The build itself took less than half a second. `cargo-nextest` therefore helps, but replacing the
runner is not the answer: it saved about 18% on the non-doctest run and does not execute doctests.

The timing report named the real bottlenecks:

1. `every_corpus_program_behaves_identically_in_both_engines` took **131.86 s**. One Rust test
   sequentially ran every executable corpus program through the VM, built it with Cranelift, linked
   it and ran the result.
2. `every_prefix_of_every_corpus_file_round_trips` took **25.23 s**. One Rust test made about
   490,000 parser calls over every character-boundary prefix of every valid and invalid corpus file.

Both tests are valuable. Their problem is shape: an external runner can schedule tests, not work
hidden inside one test.

## Decision

### 1. Three local lanes, one authoritative gate

`scripts/check` exposes three stable commands:

- `scripts/check fast` is the inner loop. It skips the whole differential test target and the
  exhaustive parser-prefix shards.
- `scripts/check pre-commit` keeps the differential target except for the exhaustive all-program
  equivalence shards, and skips the exhaustive parser-prefix shards.
- `scripts/check full` runs `cargo test --workspace` unchanged, including doctests.

The first two use repository-owned `cargo-nextest` profiles. The full lane remains ordinary Cargo
because nextest's process-per-test isolation can hide accidental process-global coupling between
tests in one binary, and because nextest does not run doctests.

**Rejected: replace gate 3 with nextest.** It is faster, but it is not equivalent evidence.

**Rejected: make the fast lane the wave-completion gate.** A feedback loop and a release proof answer
different questions. The fast lane may skip expensive evidence only because the full lane remains
mandatory before a wave is committed.

### 2. Preserve exhaustive coverage and expose parallel work

The VM/Cranelift corpus comparison and the LLVM-only three-engine comparison are each split into
**four deterministic shards**. The parser-prefix test is split into **eight deterministic shards**.
Every shard has a non-empty guard, and the partition is by index over a sorted source list, so their
union is exactly the old test's input: new corpus files remain automatic, no file is duplicated,
and none is sampled away.

**Rejected: sample prefixes or nominate a smaller differential corpus.** That would make the suite
faster by weakening the property. Sharding changes scheduling rather than coverage.

**Rejected: one test per corpus file.** Hundreds of test processes would magnify process-startup
cost and make the test inventory noisy. Four native-link shards and eight CPU-only parser shards
match the development machine's twelve logical CPUs without turning each source file into harness
machinery.

### 3. Bound tests that compete for external resources

The nextest configuration:

- permits at most four differential tests to link/run generated programs concurrently;
- runs the six SDL/OpenGL integration tests one at a time.

These limits apply to nextest lanes only. The ordinary Cargo gate keeps its existing same-process
behaviour.

### 4. Compilation caching is deferred

`sccache`, a faster linker and another Cargo profile are not part of this wave. Warm compilation was
under half a second while execution was measured in minutes, and `target/` was already 30 GiB.
Optimising the part below one percent while adding another cache would be activity rather than a
speedup.

## Consequences

- Routine broad feedback has a seconds-scale command instead of requiring the full gate.
- Pre-commit feedback retains almost every test while omitting only the two corpus-wide exhaustive
  properties.
- The authoritative test command and all seven project gates remain unchanged.
- The Rust test count rises **1216 → 1226** by default and **1222 → 1235** under gate 7 because two
  default tests become twelve and one LLVM-only test becomes four; coverage does not rise or fall.
- The nextest profiles require `cargo-nextest`; `scripts/check full` does not.

The controlled warm-cache result after implementation:

| Lane or target | Before | After |
|---|---:|---:|
| Authoritative Cargo gate, including doctests | 222.59 s | **122.54 s** |
| `jr-cli` differential target | 140.74 s | **62.99 s** |
| Gate 7 `jr-cli` differential target | 169.17 s | **102.59 s** |
| `jr-syntax` robustness target | 24.41 s | **5.96 s** |
| `scripts/check fast` | — | **12.95 s** for 1143 tests |
| `scripts/check pre-commit` | — | **35.81 s** for 1209 tests |

The full gate is about **45% faster**; the fast lane is not counted as that win because it answers a
different, deliberately narrower question.

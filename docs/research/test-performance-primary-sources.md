# Rust test-performance guidance: primary-source findings

Date: 2026-09-10

## Scope and method

This is a research note, not an architecture decision. It compares Jairs' current test setup with
current first-party documentation and source from:

- Cargo and rustdoc;
- cargo-nextest;
- sccache;
- GitHub Actions' cache service.

No secondary performance guide is used as evidence. Repository observations come from `Cargo.toml`,
`.config/nextest.toml`, `scripts/check`, `.github/workflows/ci.yml`, ADR-0209, the current test
harnesses, and the installed tools. Online documentation was read on the date above.

The inspected local toolchain is:

| Tool | Inspected version |
|---|---|
| `rustc` | 1.97.1 (`aarch64-apple-darwin`) |
| Cargo | 1.97.1 |
| cargo-nextest | 0.9.127, matching `.config/nextest.toml` |

`rust-toolchain.toml` selects stable rather than pinning a release, while the manifest's declared
minimum is Rust 1.94. Recommendations below therefore avoid nightly-only mechanisms and distinguish
features available in nextest 0.9.127 from newer documentation.

The implemented decision is recorded in ADR-0244. It keeps Cargo/libtest plus doctests
authoritative, gives nextest an isolated target directory and JUnit measurement lane, and overlaps
the ordinary and doctest phases of the full gate. A four-to-eight VM/Cranelift shard experiment was
rejected after making the differential target 90% slower.

## Findings in brief

1. **The known warm bottleneck is test execution, not Rust compilation.** ADR-0209 measured warm
   compilation below 0.5 seconds, then measured 131.86 seconds in one differential test and
   25.23 seconds in one parser-prefix test. Its sharding work reduced the full gate from 222.59 to
   122.54 seconds and produced 12.95-second and 35.81-second nextest lanes without weakening the
   mandatory gate.
2. **Cargo and nextest schedule at different levels.** Cargo runs each test target serially while
   libtest uses threads inside that target. Nextest schedules individual tests across binaries as
   separate processes. This is the main official mechanism behind nextest's better workspace-wide
   utilization, but it is also a semantic difference.
3. **The current nextest setup already uses the important basic controls:** named profiles,
   filtersets, bounded test groups, a slow timeout, and the exact installed-version requirement.
   The remaining high-value nextest work is measurement, balancing, ordering, and possibly CI
   distribution—not merely replacing `cargo test` text with `cargo nextest run`.
4. **The two exhaustive properties are still manually partitioned by sorted index modulo.** That
   preserves coverage but does not account for input length, compilation/link cost, or observed
   duration. As the corpus has grown from ADR-0209's 283 files to 327, re-measuring shard balance is
   the strongest immediate candidate.
5. **Doctest guidance changed materially with Edition 2024.** Eligible doctests in one crate are
   merged before compilation for performance, though each doctest still runs in its own process.
   Jairs is already Edition 2024 and has only four runnable doctests, so ADR-0209's 45.45-second
   doctest measurement should be repeated before redesigning them.
6. **sccache is not a warm-loop answer for this repository.** It requires Rust incremental
   compilation to be disabled for cacheable crates and cannot cache crates that invoke the system
   linker. It can still be useful for clean builds, branch switching, or CI, but only if measured
   cache hits outweigh hashing, transfer, disk, and lost incremental-rebuild benefits.
7. **Cache size is already a constraint.** The local `target/` directory measured 45 GiB during
   this audit. GitHub's default repository cache allowance is 10 GiB and its documentation warns
   that exceeding the allowance can cause cache thrashing. A full, undifferentiated `target/`
   cache is therefore not a safe default assumption.

## Pre-implementation repository baseline

### Cargo profiles and build state

The workspace root currently sets:

```toml
[profile.dev]
opt-level = 1
debug = true

[profile.dev.package."*"]
opt-level = 2
```

Cargo's built-in `test` profile inherits `dev`, so these settings apply to ordinary test builds
unless a `[profile.test]` override changes them. Dependencies are already optimized at level 2,
while workspace crates use level 1 and full debug information.

The current `target/` directory is 45 GiB. No `sccache` executable or cache was found in the
inspected environment.

### Local lanes before ADR-0244

At the start of the investigation, `scripts/check` exposed:

| Lane | Runner | Scope |
|---|---|---|
| `fast` | nextest | excludes the differential and compatibility binaries and exhaustive prefix shards |
| `pre-commit` | nextest | excludes the exhaustive differential and prefix shards |
| `full` | Cargo | one `cargo test --workspace`, including doctests |

`.config/nextest.toml` pins nextest 0.9.127, disables fail-fast for the default profile, marks tests
slow after 60 seconds and terminates after two periods, limits differential and compatibility tests
to four concurrent members, and serializes the eight named SDL/OpenGL/Game tests.

The resource controls are inherited by the custom profiles through the default-profile overrides.
The fast and pre-commit profiles currently change selection, not the test profile's thread count,
retry policy, priority, output reporting, or build profile.

### Expensive test shape

The two exhaustive tests improved by ADR-0209 are now:

- four VM/Cranelift differential tests, each receiving every fourth sorted executable corpus file;
- four LLVM differential tests with the same assignment, behind the LLVM feature;
- eight parser-prefix tests, each receiving every eighth sorted valid/invalid corpus file.

This assignment is deterministic and exhaustive. It is not cost-aware:

- one differential program can import more modules, compile more code, invoke more external tools,
  or take longer to execute than another;
- parser-prefix work is roughly related to the number of character boundaries and the cost of
  parsing each prefix, not the number of files;
- adding a file can move no existing assignment, but it can enlarge one shard's tail substantially.

There is no checked-in JUnit report, per-test duration history, or timing artifact from which current
balance can be established.

### CI shape before ADR-0244

At the start of the investigation, `.github/workflows/ci.yml` had separate jobs for formatting, clippy, documentation, Jairs
formatting, corpus drift, and tests. The test job runs `cargo test --workspace` once on macOS and
once on Linux. Every Rust-building job restores a cache through `Swatinem/rust-cache@v2`; the
workflow itself does not expose cache size, restore duration, save duration, hit rate, or which
artifacts produced the hit.

The workflow does not currently use nextest, nextest partitions, nextest build archives, JUnit
reports, or uploaded Cargo timing reports.

## Authoritative feature facts

### 1. Cargo compilation caching and profiles

Cargo profiles control optimization, debug information, incremental compilation, codegen units,
and related compiler settings. The official profile reference says:

- higher optimization can improve run time at the cost of compilation time, but the result must be
  measured because a nominally higher level can be slower;
- `"line-tables-only"` retains file/line information for backtraces with much less debug
  information than `debug = true`;
- incremental compilation stores extra state in `target/`, applies only to workspace members and
  path dependencies, and is enabled by default in `dev`;
- more codegen units may reduce compile time through parallelism but can produce slower code;
- `test` inherits `dev`;
- each custom Cargo profile gets its own directory under `target/`.

Source:
[Cargo profiles](https://doc.rust-lang.org/cargo/reference/profiles.html).

Applicability:

- Jairs' test suite executes the compiler, VM, native code generator, linker, and generated
  programs. The speed of workspace test binaries can therefore matter more than in a suite whose
  tests only perform small assertions.
- The current level-1 workspace / level-2 dependency choice is already an explicit
  compile-time-versus-run-time trade. A test-profile experiment must compare both build time and
  execution time.
- `debug = true` plus macOS's debug-info defaults is a plausible contributor to link time and the
  45-GiB target directory. It is not proven to dominate either.
- Adding a custom Cargo profile duplicates artifacts in another `target/<profile>` tree. With the
  current target size, a disposable experiment or an in-place `[profile.test]` comparison is safer
  than permanently accumulating profiles.

Risks and limits:

- Reducing debug information can make native crashes and failing integration tests harder to
  diagnose.
- Increasing optimization can make clean builds slower and may increase artifact size.
- Reducing codegen units can improve generated code but lengthen compilation and reduce compile
  parallelism.
- Disabling incremental compilation can reduce disk use and improve reproducibility of cold
  measurements while making normal edit/rebuild cycles slower.
- Cargo's `--timings` measures compilation, not test execution.

Cargo also documents `--target-dir` and `CARGO_TARGET_DIR`, but changing the directory only changes
where artifacts are stored; it does not itself make compilation faster. Sharing one target
directory can improve reuse only when profile, target, features, flags, toolchain, and relevant
inputs remain compatible.

Sources:

- [Cargo test compilation options and `--timings`](https://doc.rust-lang.org/cargo/commands/cargo-test.html#compilation-options)
- [Cargo configuration: `build.target-dir` and `build.incremental`](https://doc.rust-lang.org/cargo/reference/config.html#build)

### 2. Cargo test and doctest behavior

Cargo's official test command documentation says:

- each normal test target is compiled as a separate test executable;
- targets are executed serially by Cargo;
- libtest runs tests within a target on multiple threads;
- documentation tests are included by default;
- `--jobs` affects compilation, while `--test-threads` affects execution;
- `--no-run` separates compilation from execution for diagnosis;
- doctests are extracted and controlled by rustdoc, and their execution model is not a stable
  contract.

Source:
[Cargo test](https://doc.rust-lang.org/cargo/commands/cargo-test.html).

The current rustdoc book adds the Edition 2024 behavior: compatible doctests in one crate are merged
into one compilation unit because compilation is usually the expensive part. A
`standalone_crate` doctest opts out when source-location or namespace assumptions make merging
incorrect. Merged or not, doctests run in their own processes.

Source:
[rustdoc documentation tests](https://doc.rust-lang.org/rustdoc/write-documentation/documentation-tests.html#attributes).

Applicability:

- Every Jairs crate is Edition 2024, so the merge optimization is already available.
- The repository currently has four runnable Rust doctests, spread across `jr-base`, `jr-diag`,
  and `jr-pool`; none uses `standalone_crate`.
- nextest does not currently run doctests. Any nextest-based authoritative design must preserve a
  separate doctest command or explicitly remove doctests from the contract, which would be a
  coverage decision rather than an optimization.

Source:
[nextest test-coverage integration notes](https://nexte.st/docs/integrations/test-coverage/).

Risks and limits:

- Turning doctests into ordinary tests may make the suite faster but loses the guarantee that the
  rendered documentation example itself compiles.
- Disabling doctests in manifests is a coverage reduction, not a caching technique.
- Old per-code-block descriptions in Cargo's command reference are explicitly non-guaranteed; the
  rustdoc Edition 2024 documentation is the more specific current behavior for this workspace.

### 3. Nextest profiles, scheduling, and resource controls

Nextest supports named profiles with inheritance and per-test overrides selected by filtersets.
Overrides can configure retries, slow timeouts, test groups, required threads, priority, output,
and other behavior. Higher-precedence command-line and environment settings can replace repository
defaults.

Sources:

- [Repository configuration and profile inheritance](https://nexte.st/docs/configuration/)
- [Per-test overrides](https://nexte.st/docs/configuration/per-test-overrides/)
- [Configuration reference](https://nexte.st/docs/configuration/reference)

Nextest schedules tests across binaries. Heavy tests can reserve multiple units of the global test
thread budget with `threads-required`. A separately named test group can impose a maximum
concurrency for tests that share a scarce resource. These solve different problems:
`threads-required` is weighted scheduling; groups are mutual exclusion or rate limiting.

Sources:

- [Heavy tests and `threads-required`](https://nexte.st/docs/configuration/threads-required/)
- [Test groups](https://nexte.st/docs/configuration/test-groups/)

Applicability:

- Jairs already uses test groups correctly for native linking pressure and graphics serialization.
- The four differential shards all count as one thread today, even though each repeatedly runs a
  compiler, linker, and generated executable. Assigning them a larger `threads-required` value is
  a candidate only if measurement shows CPU, memory, or I/O contention; it may make the run slower
  on a lightly loaded machine.
- The current group cap of four matches the four differential shards, so it is a ceiling rather
  than active queueing for the default two-engine sweep. Increasing the number of shards could let
  the same four-worker group absorb skew without increasing concurrent link pressure.

Risks and limits:

- Nextest runs each test in a separate process. That is useful isolation but is not semantically
  identical to libtest's shared process within one test binary.
- More parallelism can worsen wall time if native links, temporary files, memory bandwidth, or
  system libraries contend.
- Less parallelism can leave cores idle. Thread weights should follow an observed timeline.

### 4. Slow tests, reports, priorities, and retries

Nextest's `slow-timeout` marks a test slow after a period and can terminate it after a configured
number of periods. It is a watchdog, not a ranked profiler. Jairs' 60-second threshold can identify
catastrophic long poles but cannot distinguish a 4-second test from a 40-second test.

Source:
[Slow tests and timeouts](https://nexte.st/docs/features/slow-tests/).

Nextest can emit JUnit XML containing suite and test-case durations. Its reporting UI can also show
the status of currently running tests. JUnit is available in the pinned nextest generation and is
enough to establish per-test history without adopting newer recording/replay features.

Sources:

- [JUnit support](https://nexte.st/docs/machine-readable/junit/)
- [Reporting test results](https://nexte.st/docs/reporting/)

Nextest 0.9.127 supports integer priorities from -100 to 100; higher priority tests run first.
Official guidance names high-signal smoke tests as one use. Current nextest documentation says
automatic historical-duration prioritization is future work, so any duration-aware order must be
configured or generated by the project.

Source:
[Test priorities](https://nexte.st/docs/configuration/test-priorities/).

Retries can be global or per-test and can use fixed or exponential backoff. A test that passes on a
retry is marked flaky.

Source:
[Retries and flaky tests](https://nexte.st/docs/features/retries/).

Applicability:

- A measurement profile that writes JUnit is the lowest-risk way to find the current long pole and
  shard skew.
- Starting known long shards early can reduce the tail of a parallel run. Starting high-signal
  smoke tests first improves time-to-failure even when total duration does not change.
- Retries are not a performance optimization. With no demonstrated flaky class, they increase work,
  delay real failures, and can hide nondeterminism.

Version limit:

- The current docs mark `flaky-result` as introduced in nextest 0.9.131. Jairs pins 0.9.127, so a
  proposal using that setting first requires an explicit nextest upgrade and compatibility check.
  The candidates in this note do not depend on it.

### 5. Nextest partitioning and build archives

`--partition` divides tests among separate nextest invocations. Official nextest guidance presents
it as a CI feature for runs that are too long on one machine:

- `slice:m/n` assigns the globally listed tests round-robin and is recommended for even buckets;
- `hash:m/n` is stable when the test set changes but is not guaranteed to balance duration;
- `count:m/n` is documented as worse than both and retained for compatibility.

Source:
[Partitioning and sharding runs](https://nexte.st/docs/ci-features/partitioning/).

By default, each partition job builds its own tests. `cargo nextest archive` can build once, package
the test binaries into a Zstandard-compressed tar archive, transfer it, and run partitions with
`cargo nextest run --archive-file`.

Source:
[Archiving and reusing builds](https://nexte.st/docs/ci-features/archiving/).

Applicability:

- Partitioning does not make a single local invocation faster. Running several local partitions
  would duplicate orchestration and can duplicate builds unless carefully arranged.
- It can reduce CI wall time by fanning one operating system's test archive out to several runner
  jobs. macOS and Linux need separate builds/archives because their test binaries are
  platform-specific.
- Doctests are outside the archive/run path and still need a Cargo/rustdoc step.
- The current CI test matrix is the natural boundary: build one archive for macOS and one for Linux,
  then partition each platform independently if runner cost and artifact transfer justify it.

Risks and limits:

- More CI jobs consume more billed runner time even if wall time falls.
- Archive upload/download and decompression can cost more than rebuilding on a warm cache.
- `slice` balances test counts, not measured durations. With four very heavy differential tests and
  many tiny unit tests, count balance is not necessarily time balance.
- Archives separate build evidence from run evidence; the workflow must preserve the exact
  toolchain, target, feature set, profile, and system-library assumptions.

### 6. sccache

sccache is a compiler wrapper that stores compilation results in local or remote storage. Cargo can
use it through `build.rustc-wrapper` or `RUSTC_WRAPPER`, and `sccache --show-stats` reports cache
statistics.

Source:
[sccache README](https://github.com/mozilla/sccache/blob/main/README.md).

The official Rust caveats are decisive here:

- rustc incremental compilation must be disabled for a crate to be cached;
- crates that invoke the system linker, including binaries, dynamic libraries, and procedural
  macros, cannot be cached;
- absolute source paths must match for shared hits;
- procedural macros that read files may not be tracked safely.

Sources:

- [sccache Rust support](https://github.com/mozilla/sccache/blob/main/docs/Rust.md)
- [sccache known caveats](https://github.com/mozilla/sccache/blob/main/README.md#known-caveats)

Applicability:

- It may improve clean CI builds, switching between branches that invalidate Cargo's local
  artifacts, and rebuilding dependencies across isolated jobs.
- It will not cache the final `jr` binary's linking step, nor the native linking repeatedly done by
  the Jairs differential tests.
- Locally, adopting it means trading away Cargo incremental compilation for workspace/path crates.
  That can be a net loss for ordinary edits even if clean-build cache hits improve.
- ADR-0209's warm measurements leave too little compilation work for sccache to address. The
  43.35-second build encountered during this audit shows cold or invalidated compilation is still a
  separate scenario worth measuring; it does not overturn the measured warm execution bottleneck.

Risks and limits:

- Cache hashing, local daemon startup, remote transfer, and storage are overhead.
- A second cache consumes additional disk beside the already 45-GiB Cargo target.
- Hit rate is sensitive to toolchain, flags, features, target, paths, and source inputs.
- A cache should be accepted only from controlled cold and branch-switch measurements that include
  hit rate and transferred bytes, not from one successful build.

### 7. GitHub Actions cache

GitHub's official cache service uses an immutable key. On a miss it can search ordered restore-key
prefixes and saves a new cache after a successful job. Cache contents cannot be updated in place;
a new key creates a new cache.

The default repository allowance is 10 GiB. Entries not accessed for more than seven days are
removed, and exceeding the allowance evicts least-recently-used caches. GitHub explicitly warns
that frequent creation and eviction can cause cache thrashing.

Sources:

- [GitHub dependency caching reference](https://docs.github.com/en/actions/using-workflows/caching-dependencies-to-speed-up-workflows)
- [`actions/cache` source and README](https://github.com/actions/cache)

Applicability:

- Jairs' local 45-GiB `target/` is larger than the default cache allowance by itself. The current
  third-party action may prune or select files, but the workflow provides no evidence about what is
  actually transferred.
- The six isolated CI jobs restore caches independently. A cache can reduce compilation but does
  not share already-running test processes or avoid test execution.
- Platform, toolchain, Cargo profile, feature set, and lockfile need to participate in safe cache
  identity. Broad restore keys can improve hit rate while restoring artifacts that Cargo must
  validate or rebuild.

Risks and limits:

- Cache restore/save time can exceed the compilation saved, especially for large Rust target trees.
- Separate macOS and Linux artifacts consume independent space.
- Many per-job/per-feature keys can churn the 10-GiB allowance.
- Cache hits should be evaluated from GitHub's cache telemetry and job timings, not inferred from
  the presence of a cache action in YAML.

## Prioritized candidates — not decisions

The ordering below is by expected information gain and likely applicability to the measured Jairs
bottleneck. It deliberately does not choose an architecture.

### P0 — establish a current execution timeline

**Candidate:** add a measurement-only nextest profile or invocation that emits JUnit durations for
all non-doctests, plus separately time `cargo test --workspace --doc` and a no-run Cargo build.

Questions it answers:

- Which test cases and binaries now dominate after the corpus grew to 327 files?
- Are the four differential shards balanced?
- Are the eight prefix shards balanced by actual parser work?
- Is the 2024-edition doctest merge already eliminating most of the old 45.45-second cost?
- How much of a cold, warm, and one-file-edit run is compilation versus execution today?

Minimum comparison:

1. cold/invalidation-oriented `cargo test --workspace --no-run --timings`;
2. immediate warm repeat;
3. one representative Rust source edit and rebuild;
4. nextest full non-doctest execution with JUnit;
5. `cargo test --workspace --doc`;
6. peak target size before and after.

Acceptance evidence:

- machine/toolchain/commit recorded;
- at least three repetitions after one warm-up;
- median and range, not one wall-clock number;
- JUnit or equivalent per-test durations retained for comparison.

Stop condition:

- do not change scheduling, profiles, caching, or coverage until the current long pole is identified.

### P0 — rebalance the exhaustive in-process shards

**Candidate:** use measured costs to increase and/or rebalance the four differential and eight
prefix shards while preserving the exact union, non-empty guards, and deterministic discovery.

Promising variants to measure:

- create more differential shards than the four-worker group limit, so nextest has queued work to
  absorb an expensive shard without increasing link concurrency;
- assign parser files by a static estimate such as character-boundary count rather than file count;
- assign differential files with a measured or reproducible cost estimate rather than sorted index;
- give known long shards higher nextest priority so they begin early.

Why it ranks first:

- ADR-0209 already proved that exposing hidden work to the scheduler cut the authoritative gate by
  about 45%;
- the corpus has grown since that measurement;
- this attacks execution directly and does not add a compilation cache.

Risks:

- too many shards add test-process and setup overhead;
- a checked-in historical duration table can rot or vary by platform;
- generated assignments add maintenance machinery;
- changing only nextest priority cannot improve ordinary Cargo's serial target scheduling.

Stop condition:

- reject a variant if it weakens coverage, duplicates inputs, makes assignments non-deterministic,
  or fails to improve controlled full-lane wall time beyond normal run variance.

### P0 — isolate repeated native-build costs inside the differential harness

**Candidate:** measure one differential program as parse/sema/MIR, VM execution, Cranelift object
generation, external linking, and generated-process execution rather than treating `jr build` as one
opaque duration.

Why:

- nextest can schedule only the four outer Rust tests; it cannot optimize hundreds of sequential
  compiler/linker/process operations hidden inside them;
- sccache cannot cache the final linker work;
- the test's contract requires a real process and exit status, so replacing the subprocess with an
  in-process call would change evidence.

Possible follow-up questions, not proposals:

- Is linker startup the dominant repeated cost?
- Can invariant module discovery or fixture preparation be reused without reusing compiler results
  that the test is meant to verify?
- Is there duplicated compilation between dedicated tests and the corpus sweep?
- Would a larger pool of independent shard tests reduce the tail without changing behavior?

Stop condition:

- preserve the real native binary execution, stdout, stderr, and exit-status comparison described by
  the harness; an in-process shortcut is not equivalent evidence.

### P1 — evaluate a nextest non-doctest gate with explicit semantic compensation

**Candidate:** measure a design where nextest runs ordinary tests, Cargo/rustdoc runs doctests, and a
small explicit Cargo/libtest compatibility lane retains tests that intentionally depend on
same-process behavior.

Why it may help:

- nextest schedules tests across binaries, while Cargo runs test targets serially;
- ADR-0209 measured nextest about 18% faster for the same non-doctest targets before test-shape
  improvements.

Why it is not P0:

- ADR-0209 deliberately retained Cargo as authoritative because process-per-test isolation can hide
  accidental process-global coupling;
- changing gate semantics is an architecture/coverage decision, not a runner toggle;
- doctests remain separate.

Required evidence before a decision:

- enumerate tests that use process-global state, current working directory, environment mutation,
  graphics/runtime singleton state, or shared native libraries;
- compare failures and total test inventory between Cargo and nextest;
- retain a documented compatibility command for the semantics nextest does not exercise.

Stop condition:

- do not call nextest equivalent to gate 3 unless every lost Cargo/libtest behavior has explicit
  replacement evidence.

### P1 — benchmark the existing Cargo test profile

**Candidate matrix:** compare the current inherited test profile with narrowly scoped alternatives:

- `debug = "line-tables-only"` versus full debug;
- workspace `opt-level = 1` versus 2;
- selected codegen-unit counts;
- incremental on versus off for cold/CI and edit/rebuild scenarios.

Measure:

- clean/invalidation compile time;
- one-file edit/rebuild time;
- non-doctest execution time;
- doctest time;
- link time where visible;
- target-directory growth;
- quality of a representative panic backtrace.

Why:

- the compiler itself is the workload under test, so optimized test binaries may materially reduce
  execution;
- full debug information and incremental state can contribute substantially to a 45-GiB target;
- Cargo explicitly recommends experimentation rather than assuming higher optimization is better.

Risks:

- a custom profile duplicates artifacts;
- reduced debug information may hurt failure diagnosis;
- more optimization can erase compile-time gains;
- disabling incremental can make the normal local loop worse.

Stop condition:

- no profile wins unless it improves the intended lane end-to-end, not merely one compilation or
  one microbenchmark.

### P1 — partition CI tests using one nextest archive per platform

**Candidate:** on each OS, build and archive once, then fan test execution out with
`--partition slice:m/n`; keep doctests and any Cargo-compatibility evidence separate.

Why:

- this is the official nextest use case for partitioning and archives;
- it avoids rebuilding once per test partition;
- macOS and Linux can choose independent partition counts from their own timing data.

Required measurements:

- archive build time and size;
- upload/download/decompression time;
- per-partition duration and skew;
- total billed runner-minutes;
- wall-clock improvement;
- comparison with the current cache restore/build path.

Risks:

- `slice` balances counts rather than durations;
- runner cost can rise;
- artifact transfer can dominate;
- archives are platform- and configuration-specific.

Stop condition:

- reject if wall time does not improve materially or if total cost/complexity is disproportionate.

### P2 — trial sccache for cold builds and CI only

**Candidate:** run an isolated experiment with incremental compilation disabled, record
`sccache --show-stats`, and compare clean, warm, branch-switch, and one-file-edit cases.

Why it is lower priority:

- the measured warm loop is execution-bound;
- linker invocations are not cacheable;
- local incremental compilation must be surrendered for cacheable workspace crates.

Useful cases:

- repeated clean CI builds with stable absolute paths and toolchain;
- branch switching that invalidates Cargo's local artifacts but leaves identical compiler inputs;
- sharing dependency compilation across isolated jobs, if cache transfer is smaller than the work
  avoided.

Stop condition:

- reject if hit rate, bytes transferred, or end-to-end wall time do not beat ordinary Cargo
  incremental builds and the current CI cache path.

### P2 — audit GitHub cache effectiveness and size

**Candidate:** collect cache restore/save durations, exact hit/miss status, compressed size, key
churn, and compilation time for each CI job before altering the cache strategy.

Questions:

- Are fmt and docs restoring a large cache to save only a small compilation?
- Do clippy, test, Jairs fmt, and corpus drift share useful artifacts or rebuild due to different
  targets/flags?
- Is the repository near the 10-GiB default allowance and churning entries?
- Is restoring a target cache faster than building with a nextest archive or no cache?

Stop condition:

- do not cache the full 45-GiB local target tree by analogy; use actual CI cache contents and
  telemetry.

## Things that do not make the local full run faster by themselves

- **Nextest `--partition`**: distributes work across separate invocations; it is not extra local
  scheduling inside one invocation.
- **Nextest archives**: avoid rebuilding when transferring tests; they do not reduce test execution.
- **Retries**: add attempts and can mask nondeterminism.
- **A lower slow timeout**: finds or terminates long tests; it does not optimize them.
- **A different target directory**: changes artifact placement, not compiler work.
- **Caching Cargo's registry alone**: avoids downloads but not workspace compilation or test
  execution.
- **sccache on the existing warm lane**: cannot address the hidden compiler/linker/process work
  inside the differential tests.
- **More threads without measurement**: can increase CPU, memory, filesystem, linker, or graphics
  contention.

## Proposed execution sequence for a later implementation wave

This sequence is deliberately reversible and evidence-first:

1. capture current cold, warm, edited, doctest, and per-test timings;
2. rebalance/increase exhaustive shards and retime the unchanged full gate;
3. inspect the differential harness's internal phase costs;
4. remeasure the four Edition-2024 doctests;
5. benchmark test-profile variants without retaining extra target trees;
6. evaluate nextest as a non-doctest gate only with explicit Cargo semantic compensation;
7. evaluate CI archive partitioning per platform;
8. evaluate sccache and cache-key changes only for cold/CI scenarios.

Each step should land only if its own before/after evidence shows an end-to-end improvement and the
test inventory, corpus coverage, failure semantics, and mandatory platform evidence remain intact.

## Source inventory

| Owner | Source | Used for |
|---|---|---|
| Cargo | [Profiles](https://doc.rust-lang.org/cargo/reference/profiles.html) | test inheritance, optimization, debug info, incremental compilation, codegen units, custom-profile artifact directories |
| Cargo | [`cargo test`](https://doc.rust-lang.org/cargo/commands/cargo-test.html) | test-target scheduling, libtest threads, doctests, `--no-run`, `--timings`, target directory |
| Cargo | [Configuration](https://doc.rust-lang.org/cargo/reference/config.html#build) | target directory and incremental configuration |
| rustdoc | [Documentation tests](https://doc.rust-lang.org/rustdoc/write-documentation/documentation-tests.html) | Edition-2024 merging, per-process execution, `standalone_crate` |
| nextest | [Repository configuration](https://nexte.st/docs/configuration/) | profiles and inheritance |
| nextest | [Per-test overrides](https://nexte.st/docs/configuration/per-test-overrides/) | filtersets, precedence, supported settings and version annotations |
| nextest | [Configuration reference](https://nexte.st/docs/configuration/reference) | profile and override schema |
| nextest | [`threads-required`](https://nexte.st/docs/configuration/threads-required/) | weighted scheduling |
| nextest | [Test groups](https://nexte.st/docs/configuration/test-groups/) | mutual exclusion and rate limiting |
| nextest | [Slow tests](https://nexte.st/docs/features/slow-tests/) | slow reporting and termination |
| nextest | [Reporting](https://nexte.st/docs/reporting/) | live status and output behavior |
| nextest | [JUnit](https://nexte.st/docs/machine-readable/junit/) | machine-readable test durations |
| nextest | [Priorities](https://nexte.st/docs/configuration/test-priorities/) | explicit run order and absence of automatic historical prioritization |
| nextest | [Retries](https://nexte.st/docs/features/retries/) | retry semantics and backoff |
| nextest | [Partitioning](https://nexte.st/docs/ci-features/partitioning/) | slice/hash/count behavior and build reuse |
| nextest | [Archiving](https://nexte.st/docs/ci-features/archiving/) | build-once/run-elsewhere workflow |
| nextest | [Coverage integration](https://nexte.st/docs/integrations/test-coverage/) | current doctest limitation |
| sccache | [README](https://github.com/mozilla/sccache/blob/main/README.md) | wrapper setup, storage, stats, caveats |
| sccache | [Rust support](https://github.com/mozilla/sccache/blob/main/docs/Rust.md) | incremental and linker limitations |
| GitHub | [Dependency caching reference](https://docs.github.com/en/actions/using-workflows/caching-dependencies-to-speed-up-workflows) | keys, restore keys, immutability, limits and eviction |
| GitHub | [`actions/cache`](https://github.com/actions/cache) | official action interface and cache limits |

## Research stop conditions

- No code, manifest, nextest configuration, script, workflow, ADR, plan, or status document was
  changed.
- No recommendation is presented as an architectural decision.
- No nextest feature introduced after the pinned 0.9.127 is required by the prioritized candidates.
- No claim is made that a cache is faster without measured hit rate, transfer size, and end-to-end
  timing.
- No proposed speedup is allowed to weaken corpus coverage, doctest coverage, process-behavior
  evidence, or the real native-executable differential contract.

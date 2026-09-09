# ADR-0219: Compatibility claims are executable, chapter-complete probes

- **Status:** Accepted
- **Date:** 2026-09-09
- **Deciders:** dboll
- **Reference:** `references/The_Way_to_Jai` at
  `19cb4b7acb0de2798c769f9ad73313a4d15f4056`

## Context

ADR-0217 pinned `The_Way_to_Jai` as a local research source and produced a
chapter-by-chapter compatibility inventory. That inventory is broad enough to
order work, but it is prose. This repository has repeatedly paid for prose that
continued to call a shipped feature absent, and the compatibility inventory
itself corrected several such rows.

The pinned guide contains 315 `.jai` files in 35 example-bearing top-level
chapters:

```text
03–31, 33–35, 50–52
```

Six representative examples already pass `jr check` unchanged at the pinned
revision: chapters 03, 05, 07, 11, 19 and 26. Others are source-portable with
local spelling or library changes, blocked by a concrete compiler gap, or
intentionally divergent.

Three scopes were considered:

1. one representative probe for every example-bearing chapter;
2. a thinner cross-chapter pilot;
3. all 315 examples immediately.

The decider chose the first. A pilot cannot support a chapter-complete
compatibility statement, while importing every example would initially repeat
the same prerequisite failures and turn an expectation file into the work.

## Decision

### §1. One typed manifest owns all 35 chapter probes

`tests/compatibility/probes.toml` is the authoritative executable inventory. It
records:

- the upstream repository and pinned revision;
- at least one representative upstream path for each of the 35 chapters;
- a repository-owned Jairs probe path;
- one status: `source-compatible`, `ported`, `blocked`, or `divergent`;
- the earliest useful classification: lexical, syntactic, semantic, library,
  toolchain, platform, runtime, or intentional divergence;
- a concise reason;
- the command mode and exact observable expectation.

The first manifest has exactly one row per chapter. The schema is deserialized
with `deny_unknown_fields`. IDs, upstream paths and Jairs paths are unique;
absolute paths and `..` are refused. The runner owns the approved 35-chapter
set, so deleting a chapter's last row cannot make the manifest
self-consistently smaller.

The first manifest is deliberately chapter-complete rather than
feature-complete. A chapter may contain several independent gaps; its
representative records the highest-value current one, and later probes can be
added without changing the minimum.

### §2. Only repository-owned probes execute in ordinary tests

Every executable path is below `tests/compatibility/probes/`. The test runner
never reads, checks, imports or executes `references/The_Way_to_Jai`.

The upstream path is provenance: it says which pinned source motivated the
probe. The Jairs file is the executable evidence. For a `source-compatible`
entry, the upstream file was separately checked unchanged while preparing the
manifest, but CI still executes the owned probe so a checkout with no
submodules remains complete.

Each probe is copied with its sibling fixtures to a fresh temporary directory
before invocation. File, build and process probes can therefore create output
without mutating the worktree or sharing current-directory state with another
test.

This preserves ADR-0217 §1: the guide remains a research input, not a build,
module, package, install or test dependency.

### §3. The harness tests the real command-line boundary

`crates/jr-cli/tests/compatibility.rs` is a `libtest-mimic` test binary. It
turns every manifest row into a named test and invokes the built `jr` binary
with colour and summaries disabled.

The supported modes are:

- `check` — expect acceptance or exact diagnostic codes;
- `run` — execute in the VM and compare exit status and output;
- `build` — exercise native compilation or a build script in the isolated
  directory;
- `build-run` — build a native artefact, execute it, and compare the result.

Successful probes pin an exit status and, where output is part of the example,
exact stdout and stderr. Refusal probes pin exact diagnostic codes rather than
diagnostic prose. Stable message fragments are used only where no code exists,
such as a runtime trap or external toolchain failure.

The manifest's classification is not guessed from diagnostic number ranges:
codes have moved between crates, and “library” or “platform” is a statement
about the missing capability rather than whichever phase first notices its
name.

### §4. Compatibility probes are not the language corpus

`tests/corpus/` is simultaneously the language specification, parser corpus,
formatter corpus and engine differential input. Compatibility probes have a
different contract:

- a blocked source may intentionally fail parsing or checking;
- a port may use Jairs-specific spelling;
- a platform probe may check rather than execute;
- a divergence is evidence, not a proposed language rule.

They therefore stay under `tests/compatibility/`. Putting them in the corpus
would either weaken the corpus contracts or silently turn a secondary guide
into the language specification.

The compatibility binary is omitted from `scripts/check fast`, included in
`pre-commit` and the authoritative full gate, and assigned bounded concurrency
because native builds and process probes can be expensive.

### §5. A status change is a reviewed compatibility event

When a language or library feature ships, the corresponding probe changes from
`blocked` to `ported` or `source-compatible` in the same wave. The executable
expectation changes with it. Current-facing prose is then updated from the
manifest rather than from memory.

No percentage is published in this wave. Thirty-five representatives are
enough to make chapter coverage executable, not enough to claim that 315
examples or unpublished Jai are supported.

## Rejected alternatives

- **A 16-entry pilot.** Faster to write, but leaves nineteen example-bearing
  chapters represented only by prose and still cannot support chapter-complete
  claims.
- **All 315 guide examples.** Most failures would repeat one missing feature,
  and maintaining hundreds of secondary-source expectations would obscure
  rather than prioritize parity work.
- **Execute directly from the submodule.** Violates ADR-0217, makes normal CI
  depend on submodule initialization, and still cannot establish Jai behavior
  without the closed compiler and modules.
- **Copy the guide examples into Jairs.** Duplicates copyrighted upstream text
  and creates an untracked fork. Owned probes are small statements of the
  compatibility property rather than copies of tutorials.
- **Put the probes in `tests/corpus`.** Conflates compatibility evidence with
  the language specification and breaks the corpus's parse/format/differential
  invariants.
- **Use snapshots instead of a manifest.** A snapshot records bytes but does
  not model provenance, status, phase, platform, or the relationship between an
  upstream example and its owned probe.

## Consequences

Every example-bearing guide chapter gains one named, executable compatibility
claim. Six begin source-compatible, a larger group begin as runnable ports, and
the remaining gaps are exact refusals rather than unchecked table cells.

The probe suite becomes the first gate that turns compatibility progress into
a required repository update. General procedure overloading remains the first
high-leverage language project after this P0 foundation; inferred aggregate
literals remain the smaller alternative, and both require their own design
decisions before implementation.

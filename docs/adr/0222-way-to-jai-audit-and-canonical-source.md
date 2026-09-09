# ADR-0222: The Way to Jai audit separates evidence from compatibility, and owned source is canonical

- **Status:** Accepted
- **Date:** 2026-09-09
- **Deciders:** dboll
- **Amends:** ADR-0221 §Decision.
- **Reference:** `references/The_Way_to_Jai` at
  `19cb4b7acb0de2798c769f9ad73313a4d15f4056`

## Context

ADR-0217 pinned *The Way to Jai* as a research source and ADR-0219 made one representative from
each of 35 example-bearing top-level groups executable. That was a useful baseline and not a
full-book assessment: the reference has 42 numbered groups, 60 Markdown chapters, one ASCII PDF and
315 examples. Broad chapters such as procedures, arrays, polymorphism and metaprogramming contain
several independent feature families behind one representative probe, while tooling, processes and
in-language testing were not represented.

The audit also found current-facing repository claims that had expired: CI described itself as a
skeleton after both parsers existed, W12 still said no DWARF existed, module headers repeated lifted
cross-file-generic blockers, and active examples taught `print_int`/`print_line` after `print` became
the general output API.

Finally, ADR-0221 deliberately left `jr_fmt::Config::default()` at four spaces. That meant direct
formatter users and the repository's canonical sources disagreed with generated projects,
manifest-free CLI/LSP formatting and `.editorconfig`, all of which selected two.

## Decision

### 1. The assessment has two explicitly different evidence layers

The chapter matrix is the full-book feature-family assessment. It has 59 rows covering all 60
Markdown chapters (`01A`/`01B` share one row), classified as present, partial, absent or
intentionally divergent.

The executable manifest remains a representative baseline: 35 example-bearing groups, currently
six source-compatible, eighteen ported, nine blocked and two divergent. It does not justify a
percentage over 315 examples or a claim about unpublished Jai.

The next evidence wave covers the missing CLI, process, plugin and testing groups, then splits the
broadest chapters into several family probes. The implementation order after that is general
procedure overloading; inferred aggregate literals and evaluated lengths; module/conditional
composition; cross-file template instantiation; first-class `Code`; systems breadth; ecosystem
breadth. Assembly, plugins and Windows hosting remain separate strategic projects rather than
incidental tutorial fixes.

### 2. Current-facing inventories are corrected; historical records stay historical

`README.md`, `PLAN.md` §1.5/§7, `docs/capabilities.md`, the documentation site and live module
headers must describe the current repository. Accepted ADR bodies and old wave narratives are not
rewritten to pretend their original facts were always current; a later correction or disposition
banner records what superseded them.

`docs/compatibility-plan.md` therefore keeps its historical reasoning and gains a prominent current
disposition. The research matrix and roadmap own present compatibility ordering.

### 3. Two spaces is the formatter's one default, including direct callers

`jr_fmt::Config::default().indent_width` is two. `jr_manifest::default_fmt_config()` delegates to
that shared default. Generated manifests still write the choice explicitly, and an explicit
`[fmt] indent_width` or tab style still wins.

Every parseable repository-owned `.jr` source under examples, modules, compatibility probes, the
language corpus and fixtures is reformatted to that canonical default. This amends ADR-0221's
decision to preserve a four-space direct-library default and canonical corpus.

### 4. `print` is the teaching API; old names remain compatibility wrappers

Active examples, compatibility probes, corpus programs and current teaching material use `print`,
including explicit `\n` and `%` placeholders. `print_line` and `print_int` remain exported as thin
wrappers so existing downstream source does not break. Historical discussion may name them when the
history is the subject.

Tests whose purpose was optimization or symbol plumbing no longer depend on an incidental standard
library helper name. The integer-output differential case keeps its stable filename for ADR links but
tests canonical formatting behavior.

### 5. The CI drift job is the real local gate

The stale “skeleton” guards are removed. CI generates the tracked tree-sitter parser, parses every
well-formed corpus/module/fixture tree, validates the Neovim and Zed queries, regenerates the derived
Zed highlights query, and fails if either generated tracked artefact drifts. The compiler corpus test
runs unconditionally because it exists.

## Rejected alternatives

- **Call the 35 probes “full compatibility” or publish a percentage.** One representative does not
  measure all independent families in a broad chapter, much less 315 examples.
- **Treat the guide as the Jairs specification.** It is secondary evidence about a closed language;
  executable Jairs source, tests and accepted Jairs decisions remain authoritative.
- **Delete `print_int` and `print_line`.** It cleans the namespace by breaking downstream source.
  Wrappers cost almost nothing and let teaching material converge without a flag day.
- **Keep direct formatter callers and the corpus at four spaces.** That preserves two defaults and
  makes canonical style depend on which entry point invoked the same formatter.
- **Rewrite old ADRs and wave narratives in place.** That destroys the evidence of why earlier
  decisions were reasonable and makes later corrections impossible to audit.

## Consequences

- The compatibility document answers both “what is missing?” and “how strong is the evidence?”.
- The roadmap starts with missing contracts, then orders implementation by dependency and source
  multiplier rather than chapter number.
- The large source diff is intentionally mechanical indentation; explicit formatter configurations
  remain stable.
- Current code and teaching material present one output API while old programs continue to compile.
- The ordinary six gates cover the change. Gate 7 is not required because no MIR, layout or backend
  behavior changes.
- Workspace tests remain 1333 by default and 1344 under gate 7; the corpus remains 291 files outside
  `tests/corpus/modules`; the ADR count becomes 222.

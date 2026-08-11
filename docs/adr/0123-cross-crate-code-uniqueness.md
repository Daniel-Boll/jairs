# ADR-0123 — Diagnostic-code uniqueness is enforced across crates, by a test

**Status:** Accepted
**Date:** 2026-08-07
**Amends:** `AGENTS.md`'s "there is no central registry; each crate has a `code.rs`", which described
an arrangement that was neither complete nor checked.

## Context

Diagnostic codes are per-crate by convention: each crate owns a numeric range and keeps its codes in
a `code.rs`. The convention exists because of a real bug — `jr-syntax` had no `code.rs`, its codes
were inline `&str` literals, and its parser emitted **E0200, E0201 and E0202**, which are `jr-hir`'s
"duplicate declaration", "unresolved name" and "use before declaration". A `&str` cannot collide at
compile time, so it stood for *waves*, behind a note in `AGENTS.md` telling people not to filter
tests by those codes.

The fix at the time gave `jr-syntax` a `code.rs` with two tests: no code used twice, every code
inside a range this crate owns. The audit at `354d900`
([`docs/assessment-2026-08-07.md`](../assessment-2026-08-07.md), finding F7 — raised independently by
two assessors) found that fix **cannot catch the bug it was written for**, and the file says so
itself:

> The tests below check what this crate *owns*; they cannot check a claim about somebody else's
> range, so the claim is a comment and the comment is a liability.

Both halves had come true:

- **Two crates still had no `code.rs` at all** — `jr-hir` and `jr-db`, which between them hold every
  exception in the range table.
- **The range table was hand-copied into three files** and had drifted three ways.
  `jr-syntax/src/code.rs` claimed "E0258 is the first free code overall, and E0131 the first free
  parser code" while E0131 was already in use, listed in that same file's own test data twenty lines
  below.

So the convention was being maintained by prose, in triplicate, and the prose was wrong.

## Decision

### 1. One test reads the union

`crates/jr-cli/tests/codes.rs` walks `crates/*/src/**/*.rs`, collects every
`const NAME: &str = "EXXXX";`, and checks four things:

- **No two crates declare the same code.** This is the invariant no per-crate test can state, and
  the one the original bug violated.
- **A constant named after a code binds that code**, so `const E0231: &str = "E0232";` is caught.
  That is not hypothetical tidiness — it compiles, passes every per-crate test, and reports the
  wrong code forever.
- **`AGENTS.md`'s "first free code" sentence is true.** ADR-0047 already found that sentence stale
  once; the audit found it stale in two more places. It is checkable, so it is checked.
- **The walk still finds things** — a floor on the count and a per-crate presence check, because a
  scraping test that silently stopped matching would pass forever and check nothing.

Deliberately keyed on the **value** rather than the constant's name, because `jr-mir` names its codes
*semantically* (`const USE_OF_UNINITIALISED: &str = "E0227"`) while the others name them after the
code. Both are legitimate, and the code is the identity a user sees.

Teeth-checked: pointing `jr-db`'s E0230 at `"E0201"` fails both the collision test and the
name/value test.

### 2. It reads source text rather than an exported registry

Rejected: *a `pub const CODES: &[(&str, &str)]` in each crate.* It is type-safe and not fragile, and
it widens five crates' public API for a test's convenience — against `AGENTS.md`'s "private `mod`
plus a curated `pub use`" rule, which exists precisely to stop that. It would also not catch the
name/value disagreement, since an exported list would be built from the same constants.

The cost is a dependency on how a code is declared. That is bounded by making the "the walk still
finds things" check part of the test, which is the pattern
`differential.rs::the_corpus_has_executable_programs` established for exactly this hazard.

### 3. `AGENTS.md` holds the one authoritative table; the copies become pointers

The tables in `jr-syntax/src/code.rs` and `jr-db/src/imports.rs` are replaced by a sentence naming
`AGENTS.md` and this test. Each crate still documents *its own* range, which it can keep true.

`AGENTS.md` now also says plainly that **`jr-hir` and `jr-db` have no `code.rs`**, contradicting its
own opening sentence, rather than leaving a reader to discover it. The cross-crate test closes the
*collision* risk those files carried — which was the entire reason for the rule — so consolidating
them is now tidiness rather than a defect, and it is recorded as owed instead of being done in a
wave that would have touched two 1,000-line files for no behavioural gain.

## Consequences

The collision that stood for waves is now caught at the boundary it actually crossed. Two of three
drifting tables are gone, and the surviving one has a test attached to its most rot-prone sentence.

Test count 1001 → 1005.

**What this does not do.** It does not check that a code's *meaning* is consistent, which no test
can — `SHARED` lists E0211 as deliberately raised by two crates with the reason, and adding to that
list is the moment to ask whether two uses are really one diagnostic. It does not check ranges across
crates, because the ranges are now fragmented enough (E0250 and E0253 in `jr-hir`, E0271 and E0275 in
`jr-db`, the rest of the 250s and 270s in `jr-sema`) that a range table would be a second thing to
keep true rather than a check. Uniqueness plus the first-free ceiling covers what the fragmentation
actually risks.

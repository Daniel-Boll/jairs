# ADR-0125 — `print_int` is executed, and the documents match the code

**Status:** Accepted
**Date:** 2026-08-07
**Amends:** nothing behavioural. One new corpus file, one new test, and a reconciliation of `README.md`,
`PLAN.md` and `AGENTS.md` against the tree.

## Context

Two findings from the audit at `354d900`
([`docs/assessment-2026-08-07.md`](../assessment-2026-08-07.md), F5 and F6), grouped because both are
about the gap between what this project says and what it does.

### `print_int` was executed by nothing

`README.md`'s capability table leads with "Print a number — `print_int(n)` from `modules/Basic`". A grep
over every `.jr` file in the tree found `print_int` and `print_error` **only in their own definitions
and in comments**. No program called either.

So both engines could have broken the advertised capability with all six gates green. This is the
project's own named failure mode, recurring almost verbatim — `AGENTS.md` records that
"`modules/Basic` hid a bug for a whole wave because it is not in `tests/corpus/valid/` and `file_mir`
is per file, so its bodies never appeared in a snapshot."

Also unexecuted, and recorded rather than fixed here: all thirteen files in
`tests/corpus/imports/valid/` are checked, resolved and MIR-snapshotted but never *run* in either
engine, and `Sort.is_sorted`/`less_int` are dead stdlib surface an importer cannot reach at all while
cross-file `$T` instantiation is refused (E0268).

### The documents had drifted, and not only in numbers

Fourteen places. The stale counts were the least of it — the serious ones were **capability claims that
were false**:

- `README.md` said "Linux x86-64 is kept green in CI as a sanity oracle." **No CI run has ever happened
  on this repository**; `main` has never been pushed. The same file said "configured in CI but never
  run" two hundred lines earlier, and `PLAN.md` said "Configured, never run" — three statements, one
  tree, mutually exclusive.
- The **Absent** column of "The language today" listed `type_info()`, `Any` and `#code` as missing, on
  the same page whose *Works* rows document all three.
- A bullet said "a cross-file `#run` does not work" when only the *imported-constant* half is refused —
  the callable half shipped in ADR-0069, ten ADRs before anyone corrected the sentence.

And "Open, and honest about it" was frozen around W4 sub-wave 5. The audit classified every entry:
seven had **shipped** and were never struck; five had a **stated reason that had expired**; none was
secretly broken.

That last class is the valuable one, and the pattern has a precedent: ADR-0109 revisited a refusal of a
view's `.data` whose two stated grounds "are now both false." An expired justification is worse than a
missing entry, because it reads as a considered decision.

## Decision

### 1. `valid/101-print-int.jr`, asserting output rather than an exit code

Zero (the recursion's base case, reached without recursing), one digit, two, a value crossing several
levels, both signs, and `s64` max — which is `print_digits`' deepest recursion at twenty frames. Plus
`print_error`, so a swapped file descriptor is caught rather than washing out into stdout.

The differential asserts **stdout and stderr** verbatim. That is where the teeth are: a recursion
emitting digits in the wrong order, an off-by-one in the `+ 48` byte arithmetic, or a lost sign are all
invisible to an exit code. The exit code is a checksum of the same values, so a wrong digit fails twice.

Deliberately **not** the most negative `s64`: `print_int` negates a negative and
`-(-9223372036854775808)` overflows, which ADR-0002 makes a trap. `modules/Basic` documents that, and a
file proving it traps belongs with the trap tests.

**It was correct.** No bug was found, in either engine, at any value. That is the good outcome and not
an argument against having looked — the point of the coverage is that nothing was *checking*, and the
next change to `print_digits` would have had no witness.

### 2. The documents are reconciled, and the false claims are corrected as claims

`README.md` now says plainly that no CI run has ever happened, that the six gates are green *locally*,
and that Linux is therefore entirely unverified — and it draws the consequence the audit drew, that the
tree-sitter corpus job, the only check able to detect a **wrong parse tree** rather than an error
count, has never run either.

The five expired reasons are rewritten to say what is *actually* still missing rather than being
deleted, because "why this is not here" is the most useful part of such a list when it is true.
`talloc`'s entry now says its `*u8` **is** storable at a wider type through `typed(T, p)` and that
aligned `talloc` is what remains; `T == U`'s says the design question was answered by ADR-0077's stable
`id` and that what is left is sugar nobody has argued for — a much smaller claim than the one it
replaced.

### 3. The counts stay prose, and the tree says so

Only one number is enforced: the first-free-code sentence, by ADR-0123's test. The test count and
corpus count had each drifted in three places, so `AGENTS.md` now records the progression through these
audit sub-waves *and* tells a reader to trust `PLAN.md` §7 over any count found elsewhere.

Rejected: *a test asserting the workspace test count.* It would fail on every wave that adds a test,
which is every wave — a gate that must be edited to pass is a gate people learn to edit. The counts are
a *handoff* aid, and the honest fix is to name one authoritative location rather than to pretend prose
can be enforced.

Rejected: *deleting the counts.* They caught real regressions in coverage before, which is why
`AGENTS.md` asks for them.

## Consequences

The README's flagship capability has a test. 1007 → 1008 tests, 213 → 214 corpus files.

`README.md`, `PLAN.md` and `AGENTS.md` now agree with each other and with the tree on: the test count,
the corpus count, the Neovim check count (166, having been variously 23, 67 and 151), the diagnostic-code
count (115, having been 95), the ADR range, and whether CI has ever run.

**What this does not fix.** `tests/corpus/imports/valid/`'s thirteen files are still never executed in
either engine, and extending the differential to a directory whose files mostly lack `main` is a harness
change rather than a doc fix. `Sort`'s generic surface is still unreachable across a module boundary. And
the audit's whole *security* scope remains unexamined, because the assessor responsible for it failed
twice — that is recorded in `PLAN.md` §7 and in the assessment's coverage section, and a second pass is
owed rather than quietly dropped.

# ADR-0108: A program's diagnostics are every reachable file's, not only the root's

- **Status:** Accepted
- **Date:** 2026-08-04
- **Deciders:** dboll
- **W7 sub-wave 6.** ADR-0107 §5 found this and deliberately left it alone, because fixing it changes what
  `jr check`, `jr run` and `jr build` all report. This is that change.

## Context

`file_diagnostics(root)` reports **one file**. So a root whose *imported module* was broken passed every gate —
`jr check` printed "0 errors" — and then failed inside an engine with `no routine for file 2 proc 0`: a signature
had crossed the module boundary while a body had not.

**Resolution was never the bug.** Checking the module on its own has always reported `unresolved name malloc`,
correctly. Nothing asked it. That is a *reporting* gap, and it was found by writing the `List` module, which
called `malloc` without importing `Basic`.

This is the **fifth** leaked internal error this project has turned into a real diagnostic, and the **second**
that was a cross-file body which never got compiled.

## Decision

### 1. The CLI reports every reachable file, each attributed to itself

`jr check`, `jr run` and `jr build` walk `reachable_files` — the set they *already* use to assemble MIR — and
report each file's diagnostics. `reachable_files` becomes public for this; it was crate-private while its only
use was inside `jr-db`.

**Each diagnostic keeps its own file and span**, so a reader is told the module's line, which is where the fix
goes. Attributing it to the `#import` line in the root was considered and rejected: it reads better for someone
using a module they cannot edit, and it discards the only thing that locates the bug — ADR-0043's lesson about a
diagnostic that is true and useless. For the standard library, the person who can fix the module *is* us.

**Reported by the CLI rather than by a new query.** A `program_diagnostics` query in `jr-db` would be the right
shape eventually, and today its only consumers are the three commands that already hold the reachable set — and
it would blur `file_diagnostics`'s meaning ("this file") by proximity. The three commands each gained one loop.

**No new diagnostic code.** The module's own error *is* the diagnostic; a second one saying "that module had an
error" would be noise about something already stated.

### 2. Deduplicated across roots, and a warning stays a warning

`jr check a.jr b.jr` may reach one module from both, and reporting its errors twice would make a shared module
look worse the more files import it. Each reachable set is already distinct — the seen-set is what makes a legal
import cycle (ADR-0014 §4) terminate — so the only duplication possible is across roots, and that is where the
dedupe lives.

**A module's diagnostics are reported as they are, not re-graded by distance.** An unused import in a module
(E0231) stays a warning. Re-grading would mean one code meant different things depending on which file you
compiled, which is exactly the property a diagnostic code should not have.

### 3. `jr run` and `jr build` now refuse programs they previously ran

That is the *point*, and it is worth saying plainly: this makes the compiler reject programs it used to accept.
Every one of them was going to fail — at run time, from inside an engine, with a message naming a `FileId` — so
the change is from "fails late and incomprehensibly" to "fails early with a line number". No program that
*worked* stops working.

## Consequences

- **A broken module is a compile error.** The `jr-cli` test writes a clean root against a deliberately broken
  fixture module and asserts the check fails.
- **The broken fixture lives in `tests/fixtures/broken-modules/`, not `tests/corpus/modules/`.** That directory
  has an invariant worth keeping — `fixture_modules_check_cleanly` asserts every module in it type-checks
  silently, because a *fixture* module is scenery and a broken one makes every test importing it ambiguous.
  Giving the broken module its own home is cheaper than weakening the invariant, and the invariant is what caught
  the first attempt at this test.
- **Deferred with a reason: ordering.** Diagnostics come out root-first, then per reachable file in discovery
  order — deterministic, but not source-ordered across files. Sorting by file and line would be nicer and needs a
  decision about whether the root's own errors should still lead, which they probably should.

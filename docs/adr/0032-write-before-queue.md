# ADR-0032: every write before the snapshot, and a cancelled publish must be re-queued

- **Status:** Accepted
- **Date:** 2026-07-27
- **Deciders:** dboll
- **Amends:** ADR-0024 §2, which stated the obligation salsa's cancellation imposes on the
  *worker* and not the one it imposes on the *main thread*.

## Context

ADR-0024 §2 states one half of salsa's bargain: a writer blocks until the snapshot count
drops to one, so a worker must take its snapshot per request and drop it on unwind. That is
the obligation on the reader.

There is a second obligation, on the writer, and it was not written down. A snapshot is bound
to the revision it was taken in; the *next* write cancels every reader still holding an older
one. So the order in which the main thread interleaves **writes** and **job dispatch** is
load-bearing, and `run_stdio`'s notification arm had it wrong:

```rust
if let Some(file) = apply(&mut db, &notification) {
    if adopt_root(&mut db, &mut roots, file) { db.set_workspace_roots(&roots); }
    let _ = jobs.send(Job::Diagnostics { db: Box::new(db.snapshot()), file });  // queued here
}
if !watching && matches!(method, "didOpen" | "didSave") {
    db.set_workspace_roots(&roots);          // …and cancelled here
}
```

`Job::Diagnostics` answers a cancellation by publishing nothing, with this reasoning attached:
"the write that cancelled it will queue another one, so there is nothing to report and nothing
to apologise for." That reasoning is true of `set_file_text` — a keystroke queues a fresh
diagnostics pass — and **false of `set_workspace_roots`**, which changes the file *list*.
Nothing re-queues, and the file the user just opened gets no diagnostics at all.

This presented for several waves as a flaky test. `opening_a_broken_file_publishes_diagnostics`
hung intermittently under `cargo test --workspace`, and the previous wave's handoff recorded it
as unexplained, bounded only by a watchdog. It is not a test artifact: **a client with no file
watcher silently loses diagnostics on open**, which is every plain `nvim` and every one of this
project's own stdio tests.

Measured, with the fix reverted and the machine loaded: **11 hangs in 16**. Idle: **0 in 16**.
The race window is the directory walk between the snapshot and the set, which is why it needed
contention to appear and why it read as flakiness rather than as a defect.

## Decision

### 1. Every write for a notification happens before the snapshot that answers it

The notification arm applies the text change, then `adopt_root`'s write, then the no-watcher
re-walk — and **only then** snapshots and queues the job. One snapshot, taken when no write
remains to invalidate it.

**Rejected: re-queue the diagnostics job after `set_workspace_roots`.** It would work, and it
is what the cancelled-branch comment already assumed someone would do. Rejected because it
publishes twice for one `didOpen` in the common case, and because it treats the symptom: the
job would still be *racing* a write, and the next writer added to this loop would have to
remember the same thing again.

**Rejected: have `Job::Diagnostics` retry on cancellation.** A retry inside the worker needs a
fresh snapshot, which the worker cannot take — it holds no writer database — so this is not
available without inverting the ownership ADR-0024 §2 fixed.

### 2. A cancelled read may publish nothing only when a *re-queueing* writer cancelled it

The rule the old comment needed. `set_file_text` re-queues; `set_workspace_roots` does not.
Silence is only correct downstream of the first kind, and any new writer added to the main loop
must either re-queue or run before the dispatch.

Stated as a rule rather than fixed by a mechanism because there is no type that distinguishes
the two writers, and inventing one for two call sites would be more machinery than the loop
deserves. §1's ordering makes the question moot for every writer that exists today.

## Consequences

- **The flaky hang is closed.** 0 failures in 16 loaded runs with the fix, 11 in 16 without.
- **A regression test that actually reproduces the race.** `didopen_publishes_diagnostics_even_though_it_rewalks_the_workspace`
  makes its own CPU contention and repeats the attempt 24 times: 6/6 detection against the
  reverted fix, 6/6 pass with it. Three earlier drafts detected the defect **0 times in 6, 0
  times in 6, and 2 times in 8** — one waited for the `initialize` reply (which lets the
  startup walk finish and closes the window), one padded the walked tree to 1 600 files (tree
  size turned out not to be the variable), and one ran a single attempt. A single-attempt
  version of this test is worse than no test: it passes on broken code and reports the defect
  fixed.
- **The watchdog ADR-0031's wave added is what made this findable.** It converted an
  indefinite wait into a named failure; without it the run simply sat there and the fault
  looked like slowness.
- **A test with no timeout cannot fail, only wait** — restated here because that property is
  what let a real user-facing bug live several waves behind the word "flaky".

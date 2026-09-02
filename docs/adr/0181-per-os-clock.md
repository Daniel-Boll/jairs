# ADR-0181: A per-OS value in the standard library — `CLOCK_MONOTONIC`

- **Status:** Accepted
- **Date:** 2026-09-02
- **Deciders:** dboll
- **Group C of the Simp-shaped-graphics plan**, and deliberately **one item**: it is the end-to-end proof
  that ADR-0180 works, applied to the one place in the library that already documented the problem.

## Context

`modules/Time/module.jr` carried this, and it is the clearest statement of the gap ADR-0180 closed:

```
/// `CLOCK_MONOTONIC` on both supported targets.
///
/// The number differs between platforms in general; it is 6 on macOS and 1 on Linux. **This is a real
/// portability gap and it is named rather than hidden**: the value below is macOS's, because that is the
/// only target this project has ever run on. A Linux build needs 1, and the day the CI matrix runs is the
/// day this needs a `#if`-shaped answer — which this language does not have either.
CLOCK_MONOTONIC :: 6;
```

A wrong clock id does not fail loudly. `clock_gettime` returns `-1`, `Time.monotonic` returns 0, and a
program measuring an interval gets two zeroes and a duration of nothing. So the Linux build would have
compiled, linked, run, and silently measured no time at all.

## Decision

### §1 — Selected by a procedure a `#run` evaluates, not by a conditional

```jai
monotonic_clock_id :: () -> s64 {
    if os() == Operating_System.MACOS { return 6; }
    if os() == Operating_System.LINUX { return 1; }
    return 0;
}

CLOCK_MONOTONIC :: #run monotonic_clock_id();
```

**The `#if` the old comment asked for was not built, and is not needed.** `os()` is a compile-time *value*
(ADR-0180 §2), so the selection is an ordinary procedure — and `#run <callee with ifs>` at file scope is the
established idiom, already used by `modules/Random`'s `GOLDEN :: #run golden_seed();`. A construct that
reshapes the item tree, added for a case that only needs a number, would be the largest possible answer to
the smallest question (ADR-0180 §"Why not conditional compilation").

`CLOCK_MONOTONIC :: os_dependent_number()` **without** the `#run` would also work now, since ADR-0180 §3
made a folded intrinsic reach a file-scope constant. The `#run` is kept because the callee's `if`s are not
folded — only `os()` is — so the *procedure call* needs comptime evaluation regardless. Writing it without
`#run` would be relying on the constant-evaluation path finding the call anyway, which is a different
mechanism reaching the same answer; naming the one that is asked for is clearer.

### §2 — Windows gets 0, and that is a decision rather than a placeholder

Windows has **no `clock_gettime`**. There is no correct number, so the question is what a wrong one costs.
`0` is `CLOCK_REALTIME`'s value: if the binding somehow resolved, a caller gets a real (non-monotonic)
reading rather than a failure, and if it does not, `clock_gettime`'s `-1` is the failure `Time` already
reports and `monotonic` already turns into 0.

Rejected: inventing a plausible-looking constant for a call that cannot happen. A number that looks
deliberate and is not is exactly what the old `6` was, one platform over.

## Consequences

- `modules/Time` no longer contains a per-platform lie, and the module's docs say why rather than
  apologising.
- `CLOCK_REALTIME` is untouched at `0`: it was portable before and had no reason to move. Only the value
  that actually differs is selected, which keeps the mechanism's footprint the size of the problem.
- Windows remains **unrun**. This makes `Time` source-portable and nothing more; the module's failure path
  is what reports the absence.

## Verification

- **`tests/corpus/valid/135-per-os-clock.jr` exits 42**, both engines. It asserts the constant against
  `os()` rather than against a literal, so the file is true on all three hosts — the same discipline
  ADR-0180's own corpus file uses.
- **The reading itself is checked, not just the constant.** A wrong clock id makes `clock_gettime` fail and
  `monotonic` return **0 twice**, which a "second is not before the first" comparison would happily accept.
  So the file exits 3 and 4 on a *zero* reading, which is the failure this wave could have introduced. That
  is the difference between testing the number and testing the clock.
- `cargo test --workspace` covers `Time`'s existing tests, unchanged.

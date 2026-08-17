# ADR-0135: iterating a range with an index — closing ADR-0133 §2

- **Status:** Accepted
- **Date:** 2026-08-17
- **Deciders:** dboll
- **Follow-up wave.** This is not one of the eight ADR-0127 waves; it closes the deferred half of
  wave 4 (ADR-0133 §2). That §2 recorded that `for x, i: a..b` and `for a..b { it_index }` both
  failed for the same MIR reason — a range's counter *is* its value in MIR, so a named or
  injected index went unwritten — and named the fix to whichever wave settled that gap. It has.
- No design fork was owed to the decider. §1 records one small decision (the definition of the
  index as `value - start`) that was not called out by ADR-0133.

## Context

### The bug ADR-0133 §2 named

The MIR `for_stmt` chose one induction variable per loop:

```rust
counter = match (index, has_element) {
    (None, true) => synthetic,           // for x: xs     — synthetic counter, x = xs[counter]
    (Some(i), _) => Counter::Local(i),   // for x, i: xs  — i is the counter; x = xs[i]
                                         // for x, i: a..b — i is the counter; x is UNWRITTEN
    (None, false) => Counter::Local(value), // for it: a..b — value IS the counter
};
```

For a *range* with an *index*, the counter was `i`, which was correctly written each iteration —
but the range's *value* `x` was never written. The body's read of `x` was uninitialised, so sema
reported E0227. The nameless-range case `for a..b { it_index }` was the same gap wearing a
different mask: ADR-0133 declined to inject `it_index` for a range because there was no MIR path
to give it a value distinct from the loop counter, so the name was simply absent (E0201).

### Why the fix belongs here rather than in ADR-0133

Adding a second counter to MIR is not part of the `it`/`it_index` surface. Landing it there would
have widened that wave's blast radius from parser+HIR into MIR, and a regression in either would
be attributable to only one place if the two changes stayed separate. ADR-0133 said as much. This
wave keeps the split: MIR gets one focused change, and the HIR-side injection of `it_index` for
ranges (which ADR-0133 elided) rides on it.

## Decision

### 1. For a range, `value` is the counter and `index = value - range.start`

The MIR chooses:

- **Counter is always `value`** for a range, regardless of whether an index name is present. `value`
  runs `range.start..range.end` — the shape ADR-0133 already used for the nameless-range case.
- **If an index local exists** (named or injected `it_index`), the top of the body writes
  `index = value - range.start`. That runs `0..(end - start)`, which is the definition — the
  0-based iteration count, distinguishable from `value` whenever `start != 0`.

`for x, i: 5..10` binds `x` to 5,6,7,8,9 and `i` to 0,1,2,3,4. `for 0..3 { it_index }` binds
`it_index` to 0,1,2, coinciding with `it` (a consequence of the zero start rather than a special
case). `for 5..5` never enters the body, so the index write never runs — an empty range has no
iteration to index.

**Rejected: two independent counters** (one for value, one for a zero-based index). The
subtraction at the top of the body is one instruction that const-prop deletes when `start == 0`
(the common case, `for 0..N`), so the notional cost is smaller than a whole extra induction
variable's SSA phi and its back-edge write. And the definition ("index is offset from start")
matches how a reader who reaches for `it_index` inside a range would already reason about it.

**Rejected: refuse the shape** with a specific error. It would keep MIR simpler and shift the
work to the caller, but the plan's decision (PLAN §7's it/it_index row) already declined that
path — nameless-for was decided *because* `it`/`it_index` are the ergonomic ask, and refusing
`it_index` for a range would carry the "one construct half-worked" shape ADR-0058 §3 refused
seven times.

### 2. `it_index` is injected unconditionally for a nameless `for`

ADR-0133 §1 originally injected `it_index` only for sequences, to avoid the MIR gap this wave
closes. The gap is closed, so the "sequence only" restriction is lifted: every nameless `for` now
gets `it` and `it_index` as ordinary injected locals.

The corpus file `valid/106-it-and-it-index.jr` — which used to state "`it_index` is not available
for a range" — stays in place, and this wave lands a *second* corpus file (`valid/108`) that pins
the newly working shape. Two files bracketing one behavioural change reads more clearly than
retitling the first.

## Consequences

- **`for x, i: a..b` and `for a..b { it_index }` both work.** Both cases were E0227 or E0201
  before this wave, and both are pinned by `valid/108`.
- **1010 tests unchanged; 221 → 222 corpus files.** The MIR snapshot moved for existing files
  that iterate over ranges — the injected `it_index` adds a `value - start` step to each body
  and one local, which is the whole shape of the MIR change.
- **The ADR-0127 programme's owed-count is unchanged**: this wave discharges a deferral from
  wave 4 but is not itself one of the eight. Waves 6–8 (`[..]T` dynamic arrays, `$$T`,
  `print(fmt, ..Any)`) remain, and none of them depend on this fix.
- The ADR-0133 §2 note "`for x, i: a..b` is also an uninitialised-value bug today for the same
  reason" is now closed. The residue is only the note pointing at this ADR.

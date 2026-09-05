# ADR-0204: Line wrapping, scoped by measuring the corpus rather than by taste

- **Status:** Accepted
- **Date:** 2026-09-05
- **Deciders:** dboll
- **Amends:** ADR-0202 §2, which deleted `max_width` as a dead setting and recorded line wrapping as
  unimplemented. It is implemented now, for two constructs, and the setting is back with its scope
  stated.

## Context

ADR-0202 §2 found `jr_fmt::Config::max_width` declared, defaulted to 100, and **never copied into
the `Formatter`** — dead from the day it was written. Offering it in `jairs.toml` would have been a
setting that appears to work, so it was deleted and the gap recorded as `PLAN.md` §7's first item.

That item named a fork it called expensive: keep the single-pass emitter, or move to a
Wadler-style document. **The fork dissolved under measurement**, and how it dissolved is the useful
part of this ADR.

## 1. The scope came from counting, and it is nothing like what the plan implied

Every `.jr` file gate 5 covers — 285 files, 25,865 lines:

| | |
|---|---|
| Lines over 100 columns | **3472** (13.4%) |
| …that are **comments** | **3436** (99%) |
| …that are **code** | **25** (0.097%) |
| …that are a string literal crossing column 100 | 11 |
| Widest line in the tree | 338 columns |

And the 25 code lines are three shapes: **11 call argument lists, 10 procedure parameter lists, 4
boolean chains.**

**A Wadler-style document engine cannot be justified for three constructs.** That is the whole
answer to the fork, and it took one measurement rather than a design argument. The plan had framed
the choice as architectural because nobody had counted what needed wrapping.

## 2. What is wrapped, and what is refused

**Wrapped:** a call's argument list and a procedure's parameter list, one item per line, trailing
comma, closer back at the construct's own indentation. **The trailing comma was verified to parse
before it was emitted** — a formatter whose output the compiler rejects is worse than one that never
wraps, and a grammar is entitled to refuse `f(a, b,)`.

**Refused, each for its own reason and each asserted by a test:**

**A comment is never reflowed.** It would touch 3436 lines here, and this repository's comments
carry tables, code samples and deliberate alignment that a reflow destroys. `rustfmt` takes the same
position — its `wrap_comments` is off by default and still unstable — which is the strongest
available evidence that this is not timidity.

**A boolean chain is not broken.** `a || b || c` is a *nested* left-recursive `BINARY_EXPR`, so
breaking it needs same-precedence chain flattening plus a "do not re-decide" flag threaded through
every inner node. Four lines here, the widest four columns over, against a real risk of
non-idempotent output — which would break invariant 2. The test asserts the chain stays on one line,
so whoever builds it inverts a test rather than discovering an omission.

**A long string literal is not broken.** A Jairs `string` is `{data, count}` with no continuation
syntax (ADR-0004), so a break would change the program rather than its formatting.

So **a formatted file may still contain lines over the width**, and `max_width`'s documentation says
so in all three places it appears — the field, the manifest key, and the scaffolded `jairs.toml`.
That is the difference between a scoped setting and the dead one ADR-0202 deleted.

## 3. Measure by rendering, not by predicting

The decision to break needs a width, and a width needs the construct rendered. Two ways to get one:
walk the CST computing what the output *would* be, or render into a scratch buffer and measure that.

**Rendering wins, and not on effort.** A CST-walking width calculator is a second implementation of
the emitter, and the two can disagree about what a construct looks like — which is this project's
most frequently recorded failure mode, from `#insert`'s expired comment to `type_names` keyed per
file. `Formatter::measure` takes the output buffer, runs the real emitter into it, and puts the
buffer back. There is exactly one thing that knows how a parameter list is spelled.

It restores `indent` as well as `out`, because a body that breaks increases it and an abandoned
measurement must not leak that.

## 4. The defect this wave produced, and what it teaches

The first implementation **broke every two-parameter signature in the corpus** — 41 files instead of
10, including `clamp_low :: (value: s64, floor: s64) -> s64 {` at 43 columns.

The cause: a parameter list's budget must reserve room for what follows it on the same line, so
`format_proc` renders the tail to measure it. But the function it rendered, `format_proc_tail`,
**also emitted the body** — so `reserve` was the character count of the entire procedure, and
nothing ever fit.

Two things worth keeping from it:

**The bug was invisible in isolation.** `jr fmt --stdin` on the signature alone did not break it,
because a one-line body is short. It appeared only in a file, which is why it was found by running
gate 5 over the corpus rather than by a unit test.

**And the diff display sent me to the wrong line first.** `jr fmt --check`'s rendering showed
`clamp_low` as the first change when the real edit was elsewhere on screen, so the first hypothesis
was about `clamp_low` specifically. Reading `diff -u` of the real output corrected it in one step.
**When a tool's own diff and the file disagree, believe `diff`.**

The fix splits the tail at the boundary that matters — `format_proc_signature_tail` for what lands
on the same line, `format_proc_body` for what does not — and takes `reserve` from the **first line
only**, so a `#modify { … }` attribute carrying a block cannot poison the budget either.

## 5. Two owed items closed, one of them by measurement alone

**`run_script` read the root file a second time.** ADR-0202's "Owed" list named it. `ScriptRequest`
now carries `source`, supplied by the caller that already read it for build-script detection. A
field rather than an `Option`, so the exhaustive-initialiser rule forces every construction site to
provide it and no caller can quietly fall back to a read that no longer happens.

**A faster linker is refused, and now for a measured reason rather than an absent tool.** ADR-0202
said `lld` was not installed so the option could not be measured. It still is not — it is a separate
homebrew formula, and installing software to answer a question is not this project's habit — but the
question is answerable without it:

| | |
|---|---|
| `/usr/bin/true` (process floor) | 5.3 ms |
| `ld -v` (the linker's own startup) | **39.3 ms** |
| `ld <obj> -o out -lSystem …` | 72.2 ms |
| → real link work | **~33 ms** |

**39 of those 72 ms are the linker starting up**, which any replacement binary also pays. So the
ceiling for *any* faster linker is ~33 ms of `jr build`'s 114, and the realistic gain is a fraction
of that. Recorded as a bound rather than left as an open question.

A second finding falls out: `cc <obj> -o out` is **76.2 ms** against `ld`'s own 72.2 — so the C
driver costs essentially nothing over invoking `ld` directly. ADR-0019 §2 chose `cc` because "the C
driver knows all of that and `ld` does not", and it turns out that knowledge is free.

## Consequences

`max_width` is a real setting in `jairs.toml`, honoured by `jr fmt` and over LSP, with the same
default of 100 it used to have while doing nothing. Ten corpus files reformatted once; the widest
line in the tree is now a comment rather than a 338-column call.

**On the counts.** This wave measured 1193 in isolation (1199 under gate 7), against a `main` at 1181.
It landed together with ADR-0203, which added 11 of its own, so the tree is at **1204** (1210) —
1181 + 11 + 12. Both ADRs state their own wave's contribution; `PLAN.md` §7 carries the running total,
and the arithmetic is written there so a reader who finds three different numbers can reconcile them.

### Owed

- **A boolean chain is still not broken.** Four lines, asserted as a boundary. It needs chain
  flattening; the test to invert is named above.
- **Comment reflow is not planned.** If it is ever wanted it is a wave of its own, and it needs a
  rule for tables, code samples and alignment before it needs an implementation.
- **A struct literal and an array literal are not wrapped.** Neither exceeded the width anywhere in
  this corpus, so there was nothing to measure and nothing to test against; they are the next
  candidates if one ever does.

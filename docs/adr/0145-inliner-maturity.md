# ADR-0145: Inliner maturity — a non-leaf callee, and forwarding across a straight line

- **Status:** Accepted
- **Date:** 2026-09-01
- **Deciders:** dboll
- **W8 sub-wave 4.** §2.1 lists "inliner maturity" in this wave, and `PLAN.md` §1.5 itemises what
  is missing: "**cross-block forwarding** and **SROA**". ADR-0021 §4 rejected a general cost model
  because "the performance number that would justify a real threshold is downstream of the wave
  that introduced this pass" — that wave is this one.
- No design fork was put to the decider; the recommended approach was taken and the rejected
  alternatives are recorded below, per the session directive.

## Context

### The two limits, and why each was chosen rather than overlooked

**The inliner takes leaf callees only.** `is_inlinable` refuses a callee containing any call, and
ADR-0021 §4 is explicit that this single condition is the *whole* termination argument: "a recursive
procedure calls something, so it is not a leaf, so it is never inlined, and neither is any member of
a mutual-recursion cycle. There is deliberately no depth counter and no recursion check in this
module, because no code path needs one."

That is a good decision to have shipped and a bad one to keep. A two-level wrapper — the shape a
standard library is full of, where `sort_ints` calls `sort` calls `less_int` — is inlined at exactly
one level, and the middle procedure stops the chain for every caller above it.

**Store-to-load forwarding is one walk per block.** ADR-0023 §1 deferred a dataflow analysis
deliberately, on the correct observation that every store/load pair in the slice's exit criterion
sits in one block. Since then MIR has grown constructs that split a straight line into several
blocks — `if` with no `else`, `&&`/`||`, a `for` step block, a `defer` — so a store and its load are
routinely one block apart with nothing in between.

### What a differential-tested mid-end change costs now, versus a wave ago

Two things landed in this wave before this sub-wave, and both are safety nets for exactly this work:

- **ADR-0142's equivalence sweep**: every corpus program's whole observable behaviour is asserted
  identical at `-O0` and `-O1`. A mid-end pass that changes an answer now fails a test rather than
  being noticed later.
- **ADR-0143's third engine**: VM ≡ Cranelift ≡ LLVM. A pass that produces MIR one back end reads
  differently has two witnesses.

That is why this sub-wave is a reasonable risk to take and would not have been one a wave ago.

## Decision

### 1. A non-leaf callee is inlinable; a *recursive* one is refused, and the reason is backtraces

`is_inlinable` drops the no-call condition and gains two others: the size limit it already had,
and **no cycle** — a callee that can reach itself through the bodies available for inlining is
refused. `inline_body` then runs at most `MAX_INLINE_ROUNDS = 3` rounds, each splicing only the call
sites that existed when it began.

**The draft of this ADR unrolled recursion instead, and the corpus caught it.** Three rounds of
unrolling is a legitimate optimisation and it left a real call at the bottom, so it was correct —
and it broke two differential tests, one of which is
`a_recursive_trap_reports_every_live_frame_in_both_engines`. That test asserts a four-frame
`countdown` chain, byte for byte, from two very different mechanisms. Unrolling flattened three of
those frames.

That is not a test to update. An inlined callee has **no frame** (ADR-0021 §3), and ADR-0066 §4
defers inline-provenance backtraces — so every flattened frame is a frame permanently missing from a
diagnostic. In a recursive trap the *depth* is the message: a chain of four reported as one is a
backtrace that lies about what happened. So the case where flattening costs the most turned out to
be the case whose benefit had never been measured, and it is refused instead. The unrolling was a
plausible optimisation traded against a documented promise, which is exactly the shape this
project's ADRs exist to catch.

**The cycle check walks `callees`, not a program call graph**, and that is the right scope rather
than a compromise: a cycle whose members are not all available for inlining cannot be spliced
through anyway, because the unavailable call is not a site. It is a depth-first walk with a visited
set, so a diamond is walked once and a cycle *not* through the callee terminates.

**Mutual recursion is caught too**, and deliberately by the same check rather than by a cheaper
self-call test. A self-call test would have flattened `ping`/`pong` while reporting the direct case
correctly — an inconsistency no reader could predict — so both are refused and a test pins each.

**Why rounds as well as the cycle check.** They bound different things. The cycle check makes
termination structural again; the round count bounds the *nesting depth*, so a ten-deep wrapper
chain costs three levels of splicing rather than ten. A depth counter per site would need provenance
on a statement, which MIR does not carry; rounds get the same bound from the structure that is
already there, because a splice copies the callee's calls in and refusing to visit them until the
next round makes the round number the depth.

**A total size budget on the caller**, `MAX_INLINED_STATEMENTS = 256`, stops a fan-out of medium
callees from exploding one body — something the leaf rule used to make unlikely by refusing most
callees. Checked before each splice, so a body over budget takes no further splice and the pass
stops for that body only.

**These numbers are guesses and are said to be guesses**, exactly as `MAX_INLINE_STATEMENTS = 24`
says of itself. What is *pinned* is the behaviour they bound: one test asserts that a chain at the
round limit collapses completely and one past it keeps a call, another that the size budget stops a
fan-out, and two that a recursive callee — direct or mutual — is refused.

**Rejected: a call-graph SCC analysis.** The precise answer to "is this a cycle", and it needs a
call graph a body-level pass has no access to. The `callees` walk answers the same question over
exactly the bodies that matter.

**Rejected: keeping the leaf rule and adding an "inline through a wrapper" special case.** Two
eligibility rules that must agree about what a wrapper is.

### 2. Forwarding follows a single-predecessor chain across blocks

When the backward walk reaches the start of a block without finding a store or a kill, it continues
into that block's **unique** predecessor, and so on for at most `MAX_FORWARD_HOPS = 8` blocks.

**Why a single predecessor is sound, stated as the argument rather than asserted.** A block with
exactly one predecessor is entered only from it, so every statement in that predecessor executed
before the load. By induction the whole chain executed, in order. And each block in such a chain
*dominates* the load's block — which is the second thing needed, because the operand being forwarded
is a `ValueId` and using it at the load requires its definition to dominate the use. A block with
two predecessors ends the chain, so a loop header ends it: a store before a loop is still not
forwarded into the body, and §3 says why that is left.

**A terminator cannot store**, so nothing on the way back through the chain needs examining beyond
the statements — which is why this is a walk extension and not a new analysis.

**Rejected: a full available-expressions dataflow with a meet at joins.** It is the real
cross-block forwarding and it is a bigger change than this sub-wave should carry alongside a change
to the inliner: the two would land together and a divergence found by the corpus sweep would have
two candidate causes. The single-predecessor chain is a strict subset of what the dataflow would
find, so it is not a design that has to be undone — the dataflow replaces the hop loop and keeps
everything around it.

**Rejected: chaining through a block with several predecessors when all of them agree.** That *is*
the meet, written informally, and writing it informally is how it comes to be subtly wrong.

### 3. SROA stays deferred, and the reason is sharper than "not yet"

`PLAN.md` §1.5 names SROA beside cross-block forwarding, and probing found they are not comparable
tasks.

The case SROA is wanted for is the one §1.5 itself names: `modules/Basic`'s `print` does
`store s0 <- v0` (a whole aggregate, from a parameter) and then `load s0.data`. Forwarding refuses
that pair on purpose, because "MIR has no rvalue that extracts a field from a *value*" — and that is
the actual blocker. Splitting the slot into one slot per field does not help, because the *store* is
whole-slot: there is nothing to split it into without a field-extract.

So SROA proper needs **a new `Rvalue` that projects a field out of an operand**, which reaches the
VM, both back ends and the verifier. That is its own sub-wave with its own ADR, and it is now
recorded as a MIR change rather than as a pass.

**Rejected: slot splitting for the field-wise-only case** (a slot touched only by `Field(i)`
projections). It is expressible today and it buys almost nothing: forwarding and DCE already reason
per projection path, so splitting gives them no fact they did not have. Recorded because it is the
obvious thing to do and doing it would have looked like progress.

**And this wave's own output is the evidence.** `024-hello.jr`'s optimized MIR now contains, three
times, exactly the pattern SROA is wanted for — a whole-slot `store s1 <- "hello from Jairs\n"`
followed by `load s1.data` and `load s1.count`, left behind by inlining `print`. Making the inliner
better made the missing pass *more* visible, which is a better argument for it than the prose one
this ADR started with.

### 4. Both improvements are `-O1`; no `-O2` is justified

ADR-0142 §1 refused to invent a `-O2` and said the first pass whose cost justifies opting in would
get one. Neither of these is that pass: both are bounded by construction — three rounds, 256
statements, eight hops — so there is no cost to opt out of. Saying so keeps that promise honest
rather than quietly leaving `-O2` unexplained.

## Consequences

- **The optimized-MIR snapshot moves**, and it should: `print` is now inlined into `main`, and
  `print_line` is inlined *two levels* — `print_line` → `print` → `write` — which is the wrapper
  chain the leaf rule refused and the whole reason for the change. The unoptimized corpus snapshot
  grows by exactly the new corpus file.
- **One test's assertion became a property instead of a number.**
  `print_line_loses_the_spill_slot_it_never_reads` asserted `slot_count() == 0`, which was the same
  thing while `print_line`'s only slot was its own write-only spill. It now absorbs `print` twice and
  each copy brings a `string` temporary that *is* read, so the body legitimately has slots and what
  must still hold is that **none of them is dead**. Asserting the property is strictly stronger than
  the number was.
- **The equivalence sweep and the three-way differential are the real check.** Nothing else in the
  suite would notice a mid-end pass that changes an answer, and both existed before this sub-wave
  precisely so that this sub-wave could be attempted.
- **ADR-0021 §4's termination note is superseded**, not amended: "no depth counter and no recursion
  check … because no code path needs one" was true of a leaf-only inliner and is false now. This ADR
  is the new statement.
- **`MAX_INLINE_ROUNDS`, `MAX_INLINED_STATEMENTS` and `MAX_FORWARD_HOPS` are guesses**, each said to
  be one where it is declared, each with the property it bounds pinned by a test.
- **`valid/116` exits 112** and is run at both optimisation levels by ADR-0142's sweep and in all
  three engines by ADR-0143's. Every construct in it is one the old pipeline could not flatten — a
  three-level wrapper chain, a store and load one block apart across an `if` with no `else`, and a
  recursive procedure that must still be refused — so a wrong answer after this wave is attributable
  to it.
- **1027 → 1030 tests** (1031 under gate 7), 231 → 232 corpus files.
- **Deferred, with a sharper reason than before**: SROA, which is a MIR change (§3); a full
  cross-block dataflow (§2); compacting the SSA value arena, which is unrelated to either and stays
  ADR-0022's follow-on work.

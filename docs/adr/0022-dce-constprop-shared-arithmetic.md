# ADR-0022: DCE and const-prop, one shared arithmetic, and a bounded fixed point

- **Status:** Accepted
- **Date:** 2026-07-27
- **Deciders:** dboll

## Context

ADR-0021 gave `jr-mir` its first optimisation pass. `PLAN.md` §7 names the next two —
dead-code elimination and constant propagation — and then the first published
performance number, which §1.3's estimate has been waiting for since the plan was
written.

Five things about the existing code decide this wave's shape, and all five were read
rather than assumed.

**ADR-0002's arithmetic already exists twice.** `jr-vm`'s interpreter does it in `i128`
through `IntKind::check`, which reads signedness and width from `Item::IntType` and
range-checks before normalising. `jr-codegen-clif` does it with Cranelift's
`sadd_overflow` family plus `trap_if`. Nothing structural keeps the two equal;
`crates/jr-cli/tests/differential.rs` does, by comparing a trapping program's stderr.

**Cranelift cannot share an evaluator.** It emits code; it never evaluates. So
extracting one shared implementation takes ADR-0002's arithmetic from three
implementations to **two**, not to one, and the remaining pair is still held equal only
by test. That is worth stating plainly, because "one shared computation" is what
ADR-0018 §2 achieved for layout and this is deliberately weaker.

**A dead assignment can still trap, and the code already says so.**
`crates/jr-codegen-clif/src/body.rs:266` reads: *"A discarded rvalue is still evaluated,
deliberately: an ADR-0002 overflow in an expression whose result nobody wants still
traps."* A `Statement::Assign` whose destination is never read is semantically that same
case. DCE therefore cannot be "remove assignments nobody reads" without deleting
observable behaviour that both engines currently implement and one of them comments on.

**Fold-only const-prop would be a no-op on the case that motivates the pass.** After
ADR-0021's splice, an inlined call's arguments arrive as *edge arguments* to the copied
entry block. For `caller :: () -> s64 { return add(2, 3); }` that is

```text
    goto bb2(2_s64, 3_s64)
  bb2(v3: s64, v4: s64):
    v5: s64 = v3 + v4
```

A pass that folds only when both operands are already `Operand::Constant` sees two
`Operand::Value`s and declines — so §7's reason for ordering const-prop after the
inliner would go unrealised. Collapsing a parameter every predecessor agrees on is what
turns that into something foldable.

**A correction to this ADR's own first draft, recorded rather than folded in.** The
draft used `024-hello.jr` as the example above, and that was wrong. Its `main` reads

```text
    store s0.0 <- 4_s64
    store s0.1 <- 5_s64
    v0: s64 = load s0.0
    v1: s64 = load s0.1
    goto bb12(v0, v1)
```

— the edge carries two *loads*, not two constants, because `p` is a `struct` and so
lives in a slot rather than in SSA. Nothing in this wave folds through memory, so
`024-hello.jr` gains a deleted `nop` and nothing else. The transformation is still
worth having, on the shape shown first; the mistake was reading a `store` of a literal
as a constant operand. Store-to-load forwarding for a slot whose address is never taken
is the pass that would close it, and it is named in the follow-on work rather than
smuggled into this one.

**There is a local precedent for a bounded fixpoint.** `file_consts` iterates
lower-then-evaluate under `MAX_ROUNDS = 16`, and its docs give the reason: a bound
rather than "until stable" so that a bug in the progress check is a diagnosable stop
instead of a hang. This pipeline runs inside a salsa query too, so a hang is a hung
editor.

## Decision

### 1. Both passes land in one wave, and the arithmetic extraction comes with them

The alternative considered first was DCE alone, deferring const-prop to a wave that
could put the extraction's own forks up separately. It was rejected: §7 already
sequences the two, the extraction is the *only* hard part of const-prop, and splitting
would mean a wave whose deliverable is "DCE plus a `nop` remover" while the
opportunity the inliner created sits untaken for another wave.

The cost is named rather than discovered: this wave touches `jr-pool`, `jr-vm`,
`jr-mir` and `jr-db`, and three of those currently agree about arithmetic only because
a test says so. §7's third item — the performance number — stays out, because it needs
a benchmark harness, programs larger than the corpus's 43-line maximum, and a stable
place to report, none of which is MIR work.

### 2. ADR-0002's integer arithmetic moves into `jr-pool`, behind its own operator enum

`IntKind` moves from `jr-vm` to `jr-pool`, together with the checked operations, as
`jr_pool::{IntKind, IntOp, IntCmp, IntTrap}`. `IntKind::of` already takes a `&Pool` and
reads `Item::IntType { signed, bits }`: it is pool knowledge that happened to live in
the interpreter. `jr-vm` re-exports `IntKind` so no consumer breaks, and maps
`IntTrap` onto its own `Trap` — `IntTrap::Overflow { what }` carries the same
`&'static str` the message is built from, so the mapping cannot introduce wording drift.

`jr-pool` gets **its own** operator enums rather than `jr-mir`'s. The translation lives
in `jr-mir`, as `BinOp::as_int_op` and `BinOp::as_int_cmp`, so there is one mapping and
both consumers use it — `jr-vm` depends on `jr-mir` already.

**Rejected: move `jr_mir::BinOp` down into `jr-pool`.** One operator enum, no mapping,
no drift. ADR-0017 argues at length that MIR *owning* its operator set is what makes
`&&` unrepresentable as an `Rvalue::Binary` — the set is HIR's minus `And` and `Or`,
and an exhaustive match at the translation makes a new HIR operator a compile error.
Moving the type turns that into an argument about a type MIR no longer owns, and gives
`jr-pool` — whose job is identity and layout — a notion of "operator" it has none of.
The mapping functions preserve the protection: they are exhaustive, so a new MIR
operator is a compile error at both call sites.

**Rejected: a new `jr-arith` crate.** `IntKind::of` needs `Pool`, so the crate would
sit *above* `jr-pool`, which is where the arithmetic effectively already is. It buys a
`Cargo.toml`.

**Rejected: fold in `jr-mir` as a third implementation.** The cheapest option, and the
reason it is wrong is specific rather than aesthetic. Const-prop runs at *compile* time
and bakes its answer into a `PoolId` that both engines then consume. A fold that
disagrees with the interpreter does not produce two engines disagreeing — it produces
two engines agreeing on the wrong constant, which `differential.rs` cannot see. That is
the exact failure shape §3.1's invariant exists to prevent, and `jr-mir`'s own crate
docs already say "a second evaluator is exactly what §3.1's invariant forbids" about
`ConstValues`.

### 3. The pipeline is a `jr_mir::optimize` façade; the query keeps only the policy

`optimized_file_mir` calls one function. `jr-mir` owns which passes run and in what
order; `jr-db` continues to own ADR-0021 §2's decision about *which bodies* may be
rewritten at all. Each pass stays `pub` so a test can drive it alone, which
`tests/inlining.rs` already does.

The split is the point: the frozen-set check and the pass ordering are different kinds
of decision, and the one that matters for correctness — that no pass touches a body
comptime executes — should not be one item in a growing list of calls.

**Rejected: keep the query calling each pass.** Greppable, and the order sits beside
the frozen-set decision it must respect. Rejected because `jr-db` would own a
sequencing decision that is `jr-mir`'s, and every future pass would edit a query —
which is how a pass eventually gets added *before* the frozen-set check rather than
after it.

**Rejected: a `Pass` trait and a vector.** rustc's shape, and rustc has sixty passes.
With three, the indirection makes the single thing that matters here harder to see
rather than easier.

### 4. DCE removes unreachable blocks, `Nop`s, provably pure dead assignments, and unused slots

The purity predicate is an exhaustive match over `Rvalue`, and it **admits** `Use`,
`Address`, comparisons, the wrapping operators, `Not` and `Undef`. It **refuses**:

- trapping arithmetic (`Add`, `Sub`, `Mul`, `Div`, `Rem`, `Neg`) — ADR-0002, and
  `body.rs:266`'s existing commitment;
- `Call`, which can do anything, including `exit`;
- `Load`, and any place whose base is a `Deref`, because a read through a dangling
  pointer faults — `Trap::BadAddress` is reachable from a valid program, as
  `jr-vm`'s own docs note.

Exhaustiveness is load-bearing: a future `Rvalue` variant is a compile error here
rather than a variant silently classified as pure by a `_` arm. That is the same reason
ADR-0017 required exhaustive matches throughout.

A slot is removed when nothing stores to it, loads from it, or takes its address —
**and a store to a slot that is never loaded and never address-taken is itself
removed first**, because otherwise the rule above achieves nothing.

That second clause was added after the first draft, and the correction is worth
recording rather than quietly folding in. §7 names the symptom this decision exists to
fix: `print_line` in `modules/Basic` keeps a spill slot it never reads. Its MIR is

```text
  bb0(v0: string):
    store s0 <- v0
    discard call proc2(v0)
```

— the slot is kept alive *by the dead store that fills it*, so a rule phrased only as
"remove slots nothing mentions" would have left the exact case it was written for. The
draft was wrong in the same shape ADR-0021 §3's draft was: a decision that sounded
sufficient and was checked against the code only afterwards.

Dropping the store is sound precisely because the address was never taken: nothing can
alias the slot, so nothing can observe the write. A store through a `PlaceBase::Deref`
is never dropped, because what it aliases is unknown.

Removing a slot requires renumbering every surviving `SlotId`, which is accepted for
the same reason.

**Rejected: unreachable blocks and `Nop`s only.** Trivially sound, needs no predicate,
and would land a wave that does not address its own stated motivation.

**Rejected: remove any dead assignment, traps included.** What a C compiler does with
dead undefined behaviour. It contradicts ADR-0002 in terms — "overflow always traps,
never differs between debug and release" — and contradicts a decision both engines
already implement. Recorded because it is what a first implementation does by default.

### 5. Const-prop folds, substitutes, collapses single-valued block parameters, and folds a constant branch

Four transformations, in one pass because they feed each other within a single walk:

1. An `Rvalue::Binary`/`Unary` whose operands are all `Operand::Constant` folds to
   `Rvalue::Use(Operand::Constant(_))`, through §2's evaluator.
2. A value defined by `Use(Constant)` is substituted at its uses.
3. A block parameter whose every predecessor supplies the *same* constant becomes that
   constant: the parameter is dropped, every incoming `Target::args` entry is dropped
   in step, and uses are substituted.
4. A `Terminator::Branch` on a constant condition becomes a `Goto` to the taken arm.

(3) is what makes the pass worth running after the inliner — a call with literal
arguments folds all the way to a constant, which `inlining_a_literal_call_folds_all_the_way_to_a_constant`
asserts — and (4) is what lets DCE then delete the arm that cannot run. (3) is also the
most intricate code in the wave: the parameter list and every predecessor's argument
list must move together, which is precisely what the verifier's edge-arity check exists
to catch.

**A fold that would trap is not performed.** The statement is left exactly as it was,
so the trap happens at run time with the location ADR-0020 gives it. Folding it into a
compile-time diagnostic is a *language* decision about whether `1/0` in unreachable-ish
code is an error, and nothing in Jairs-0 has decided that; silently folding it to some
value would be a miscompile.

**Rejected: local folding only.** Simple and obviously correct, and a no-op on
`024-hello.jr` for the reason the Context gives.

**Rejected: full SCCP with a lattice and a reachability worklist.** The textbook
version, and strictly more powerful. Rejected because its extra power over (1)–(4) is
that it discovers unreachability *itself*, which means it and DCE both delete blocks —
and two passes editing the same structure is how an ordering bug becomes a miscompile
rather than a missed optimisation. Reachable later by promotion, once DCE and this pass
have a fixed point that is understood.

### 6. The passes iterate to a bounded fixed point

`optimize` runs inline, then const-prop, then DCE, and repeats while any pass reports a
change, capped by `MAX_OPT_ROUNDS`. Each pass returns whether it changed the body.

The cascade is real: (4) lets DCE delete a block, which can leave a surviving block
parameter with one predecessor, which (3) then collapses. A single pass over the three
would leave that on the floor.

The cap follows `file_consts`' precedent verbatim, including its argument: a bound
rather than "until stable" means a bug in a pass's change-reporting is a diagnosable
stop rather than a hang, and this loop runs inside a salsa query where a hang is a hung
editor. Reporting a change that did not happen wastes a round; failing to report one
loses an optimisation. Neither is a wrong answer.

**Rejected: one hand-picked schedule.** The number of repetitions would be a guess
tuned by whoever next noticed a missed fold — the same objection that rejected a
general inlining cost model in ADR-0021 §4.

**Rejected: iterate until stable, uncapped.** Termination would rest entirely on every
pass reporting change accurately, forever, and the failure mode is the compiler hanging
rather than returning something visibly wrong.

### 7. Each dangerous shape gets its own differential case

DCE is the first pass that can *remove* observable behaviour rather than rearrange it,
so the wave adds targeted cases for exactly what §4's predicate must get right: a dead
expression that overflows still traps in both engines, a dead call still runs, and a
dead load is not removed.

**Rejected: lean on the corpus differential and the snapshot.** No corpus program
contains a dead trapping expression, so the dangerous case would be untested, and a
snapshot proves only that the output changed. This is the "a plausible argument stood in
for a check" pattern behind both of this project's silent miscompiles.

**Rejected: a property test over generated programs.** The strongest guarantee
available, and a program generator is a wave of its own. It would also be built before
there is any evidence the targeted cases are insufficient.

## Consequences

### Positive

- ADR-0002's arithmetic has one implementation for both places that *evaluate* it, and
  the remaining divergence is confined to the one place that cannot share — the code
  generator — where `differential.rs` is already the guard.
- The inliner's output is finally consumed by something: constants that arrive as edge
  arguments get folded, and the branch they feed collapses.
- `jr-mir` owns its own pass ordering, so a future pass is a change in one crate.
- The frozen-set check stops being one call in a list and becomes the query's only
  optimisation decision.
- `#run` and native still execute identical MIR for every body comptime touches:
  ADR-0021 §2's exclusion is upstream of the whole pipeline, not per pass.

### Negative

- The wave touches four crates, and moving `IntKind` moves a type `jr-vm` currently
  exports. A re-export keeps it compiling and is a small piece of debt.
- `jr-pool` acquires two operator enums it has no other use for, plus the translation
  cost of an exhaustive match at each call site.
- Slot renumbering means a `SlotId` in a MIR dump is no longer stable across an
  optimisation change, so the optimized snapshot will churn more than the built one.
  Acceptable: the built snapshot is the stable one, and it is the one that describes the
  program the user wrote.
- Const-prop does not fold a trapping operation, so `MAX + 1` in dead code survives to
  run time. Correct rather than optimal, and a language decision has to be taken before
  that changes.
- Still no published performance number. §1.3 waits one more wave.

### Follow-on work this forces

- **Into this wave:** the three differential cases; the exhaustive purity predicate;
  the block-parameter collapse rewriting every predecessor in step.
- **Into the next mid-end wave: store-to-load forwarding.** Nothing in this wave sees
  through memory, so a `struct` whose fields are written from literals and read back —
  which is `024-hello.jr`'s `main` — folds nothing. A slot whose address is never taken
  cannot be aliased, so forwarding a store to a later load of the same place is sound
  and would connect const-prop to the aggregate code that currently defeats it.
- **Into the performance wave:** a benchmark harness, programs large enough to measure,
  and a place in `PLAN.md` §1.3 to report a number honestly. ADR-0019 §6's expiry
  condition is finally satisfiable.
- **Into wave W8:** whether `jr-codegen-clif` should *verify* itself against
  `jr_pool`'s evaluator on constant operands, which would close the last gap in
  ADR-0002 having one meaning. Cheap to do, and it is a testing decision rather than a
  design one.
- **Into whichever wave decides it:** whether a provably-trapping operation in dead code
  is a compile-time error. §5 deliberately declines to decide it.
- **Left undone on purpose:** the SSA value arena is never compacted, so a value whose
  defining statement was removed keeps its `ValueData` and the VM still sizes a frame
  for it. Unlike a slot, a value is named by block parameters as well as by statements,
  so compaction is a wider rewrite; it is worth doing when a register budget is a
  measured problem rather than a suspected one.

## Alternatives considered

Each fork's rejected alternatives are argued at its own point of decision. One
alternative spans the whole ADR.

**Skip the mid-end and do the editor packaging instead.** §1.4's three open boxes are
VS Code packaging, Neovim packaging and a Linux CI run, and closing them would finish
the slice on paper. Rejected for the reason ADR-0021's equivalent alternative was
rejected: every wave that passes without a mid-end is a wave whose performance claims
cannot be made, and §1.3's estimate has been waiting on a number since the plan was
written. The packaging is neither blocked by this wave nor made harder by it.

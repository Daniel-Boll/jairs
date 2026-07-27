# ADR-0023: Store-to-load forwarding, block-local and without a layout

- **Status:** Accepted
- **Date:** 2026-07-27
- **Deciders:** dboll

## Context

ADR-0022 gave `jr-mir` const-prop and DCE, and its own Context records the correction
that motivates this ADR: `024-hello.jr` folds **nothing**, because `p` is a `struct` and
so lives in a stack slot, and nothing in the mid-end sees through memory. The optimized
dump of `PLAN.md` §1.4's exit criterion gained a deleted `nop` and no more.

The shape of the missed opportunity is worth stating exactly, because it decides the
scope:

```text
  bb0():
    store s0.0 <- 4_s64
    store s0.1 <- 5_s64
    v0: s64 = load s0.0
    v1: s64 = load s0.1
    goto bb12(v0, v1)
```

Every store and its matching load sit in **one block**, with nothing between them. So
does `sum`'s pair, four blocks later:

```text
  bb11(v2: s64):
    store s1 <- v2
    v3: s64 = load s1
    v4: bool = v3 > 5_s64
    branch v4 ? bb1() : bb2()
```

Three further facts constrain the pass, and all three were read rather than assumed.

**`sum`'s address is taken — but later.** `v8: *s64 = addr s1` appears in `bb7`, well
after the store and load above. A rule that refused any slot whose address is ever taken
would decline `s1`, and with it the `9 > 5` fold and the branch collapse that make the
whole cascade visible.

**A whole-slot store cannot feed a field load.** `modules/Basic`'s `print` is
`store s0 <- v0` followed by `v1: *u8 = load s0.data`. The store supplies the whole
aggregate; the load wants one field of it. MIR has no rvalue that extracts a field from
a *value* — `Projection::Field` applies to a `Place`, never to an `Operand` — so there
is nothing to forward. This is why the rule below is *identical paths* and not
*overlapping paths*.

**`jr-mir` may not compute a layout.** ADR-0017 §5 keeps sizes, alignments and offsets
out of the crate entirely, and `PLAN.md` §7 lists computing one as the first entry under
Traps because a second layout computation is a silent comptime/runtime divergence. So
disjointness has to be decided from *indices*, not from byte ranges.

## Decision

### 1. Forwarding is block-local and flow-sensitive

One forward walk per basic block. A `Rvalue::Load(P)` becomes `Rvalue::Use(v)` when an
earlier `Statement::Store { place: P, value: v }` in the *same* block has nothing between
it and the load that could disturb `P` (§2). There is no lattice, no join over
predecessors, and no back-edge reasoning, so termination is the walk finishing.

That is enough for the whole of the Context's cascade, which is the test of whether the
scope was chosen for the problem or for its own elegance: both pairs are intra-block, so
forwarding them lets ADR-0022's block-parameter collapse see constants on the edge into
`bb12`, which lets the fold produce `9`, which lets the branch collapse, which lets DCE
delete the untaken arm and then both slots.

The cost is stated: a value written before a loop and read inside it stays in memory,
because the store and the load are in different blocks.

**Rejected: cross-block available-stores dataflow.** Strictly more powerful, and the
natural next step. Rejected for this wave because it needs a lattice, a fixpoint over the
CFG and correct treatment of loop back-edges — where a store on the back-edge path must
kill a value that looked available on entry. That is the class of pass whose subtle bug
is a miscompile rather than a missed optimisation, and it would be built before
block-local forwarding has shown what it actually leaves behind.

**Rejected: promote whole slots to SSA (scalar replacement of aggregates).** The real
fix, and it subsumes forwarding. Rejected because it is a wave rather than a pass: a
field's address may be taken, an aggregate may be passed or returned whole, and
`string`'s `.data`/`.count` are pseudo-fields ADR-0004 has only ever specified in prose
(ADR-0017 §5 records that gap as deliberately open). It also needs ADR-0017 §2's claim
that there will never be a `mem2reg` re-argued rather than quietly contradicted: that
claim is about *locals*, which Braun's construction already promotes, and an aggregate
slot is a different case — but the distinction should be written down by the wave that
relies on it.

### 2. Only slot-local places participate, and the kill set is explicit

A place **participates** only if its base is a `PlaceBase::Slot` and its projection
contains no `Projection::Deref`. Anything reached through a pointer names memory this
pass cannot reason about, and a `Deref` step in the middle of a projection means the
same thing as one at the base.

A store at index *i* forwards to a load at index *j > i* in the same block when:

- both places participate, and their **projection paths are identical** — see §3;
- no statement in *(i, j)* stores to a place on the same slot whose path is identical to,
  or a prefix of, or prefixed by, the load's path;
- no statement in *(i, j)* takes that slot's `Address`;
- and, **only when the slot's address is taken somewhere in the body**, no statement in
  *(i, j)* performs a call or stores through a `Deref`.

The last clause is what makes `s1` work in the exit criterion's file. A slot whose
address is never taken cannot be reached indirectly at all, so neither a call nor an
indirect store can touch it — the same argument ADR-0022 §4 already uses to justify
dropping a dead store, and the two passes share the predicate rather than each having
their own.

For a slot whose address *is* taken, the guard is deliberately coarse: any intervening
call or indirect store kills forwarding, whether or not the pointer could actually reach
this slot. Being precise about that needs alias analysis, and there is none.

**Rejected: refuse any slot whose address is ever taken in the body.** Much less to
reason about and obviously sound. Rejected because it declines `s1` — whose address is
taken several blocks *after* the pair being forwarded — so the `9 > 5` fold and the
branch collapse would not happen and the wave would land having half-done its motivating
example. That is the failure mode ADR-0022 §4's first draft had, and it is not worth
repeating one ADR later.

**Rejected: decide overlap from a computed layout.** Comparing byte ranges would settle
partial overlap exactly instead of conservatively. It contradicts ADR-0017 §5 in terms
and is the first entry under `PLAN.md` §7's Traps.

### 3. Two distinct projection steps are disjoint storage, which is not a layout claim

`s0.0` and `s0.1` do not overlap, and neither do `s0.data` and `s0.count`. That is a
statement about a struct having distinct fields, not about where they sit: nothing here
asks how large a field is or where it starts. Paths are compared step by step, and the
first differing step means disjoint.

Where the paths do relate, forwarding is refused rather than attempted:

- **identical** — forward;
- **one a strict prefix of the other** — refuse, and kill. `store s0 <- v` followed by
  `load s0.data` is the `print` case from the Context: the value is there but MIR cannot
  extract a field from it. A partial store (`store s0.0`) followed by a whole load
  (`load s0`) is the mirror case and is refused for the same reason.

Refusing the prefix case rather than treating it as disjoint is the load-bearing half. A
prefix relation means the two *do* share storage, so treating them as unrelated would
forward a stale value.

**Rejected: synthesise the extraction.** A `Store` of an aggregate operand could be
turned into per-field stores, after which the field load forwards. That is scalar
replacement arriving through the back door, it needs the field list and therefore the
`string` layout ADR-0004 leaves in prose, and it belongs to §1's rejected SROA wave with
its arguments made properly.

## Consequences

### Positive

- `024-hello.jr` finally optimises, which makes the three passes of ADR-0021 and
  ADR-0022 observable on the file the whole slice is measured against rather than only
  on hand-written examples.
- The pass is a single forward walk, so it cannot fail to terminate and it has no
  ordering relationship with itself.
- `jr-mir` still knows no size, alignment or offset. ADR-0017 §5 holds.
- ADR-0022 §4's "the address was never taken, so nothing can alias it" argument becomes a
  shared predicate rather than a claim made twice.

### Negative

- A store and a load in different blocks are not connected, which is most loops.
- For an address-taken slot, any intervening call kills forwarding regardless of whether
  the call could reach it. Imprecise, and the fix is alias analysis.
- A whole-slot store never feeds a field load, so `modules/Basic`'s `print` and
  `print_error` keep their slots and their loads. That is the majority of aggregate code
  in the standard library today.
- The optimized MIR snapshot changes substantially, so the diff a reviewer reads is
  large. The *built* snapshot is untouched, which is why ADR-0021 §1 kept two.

### Follow-on work this forces

- **Into the next mid-end wave:** cross-block forwarding, or SROA, with §1's rejected
  arguments answered. SROA is the one that would fix `print`.
- **Into the performance wave:** unchanged, and now the last mid-end excuse is gone.
  ADR-0019 §6's expiry condition is satisfiable and the number has not been taken.
- **Into wave W1:** a benchmark that is not an integer loop. Jairs-0 has no arrays, no
  `for`, no floats and no way to print an integer, so the only expressible workload is
  arithmetic in a `while` — which measures the register allocator more than the mid-end.
  This is the reason this wave does not publish a runtime number, and it is a language
  gap rather than a tooling one.

## Alternatives considered

Each fork's rejected alternatives are argued at its own point of decision. One
alternative spans the whole ADR.

**Publish the performance number now and leave forwarding.** §7 has listed the number as
"next" for two waves. It was rejected because the number is infrastructure rather than a
MIR change — a harness, machine-noise handling, generated source larger than the
corpus's 43-line maximum, and a committed place in §1.3 to report it — and bundling it
with a pass makes the wave's gate "the number looked plausible", which is not a gate.
Forwarding also changes what any number would say, so taking one first would mean taking
it twice.

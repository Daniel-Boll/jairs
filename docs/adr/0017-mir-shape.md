# ADR-0017: MIR shape — block parameters, SSA at construction, poison refused

- **Status:** Accepted
- **Date:** 2026-07-26
- **Deciders:** dboll

## Context

`jr-mir` is being built. Everything upstream of it exists: the lexer, the
error-recovering parser, the lossless CST, HIR with name resolution, the module
loader, the `InternPool`, and `jr-sema`. `PLAN.md` §3.1 names the IR "typed SSA,
monomorphized" and states the invariant the whole design hangs off:

> **The load-bearing invariant:** comptime and runtime execute *the same* MIR. The
> VM consumes bytecode lowered from the identical MIR that Cranelift consumes.
> Any other arrangement guarantees `#run` and runtime silently disagree.

"Typed SSA" is three words and at least four unmade decisions. This ADR makes
them, because each shapes a data representation and each is expensive to undo:

1. How basic blocks and their contents are represented.
2. How a mutable local becomes an SSA value.
3. Whether one MIR unit covers a file or a procedure — which fixes the salsa
   query's granularity, and so cannot be deferred.
4. What lowering does with a body that failed to type-check.

Two facts about the existing code narrow the space more than any preference
would, and both were verified rather than assumed:

**The CFGs are reducible by construction.** The HIR's entire control flow is
`Stmt::Block`, `Stmt::If`, `Stmt::While`, `Stmt::Return`, `Stmt::Break` and
`Stmt::Continue` (`crates/jr-hir/src/hir.rs`). There is no `for`, no `defer`, no
labelled break, no `goto` and no expression-valued block; `for` and `defer` are
lexed and then rejected by the parser as "arrives in wave W2". Reducibility is
not a property we are hoping for, it is a property the grammar cannot express the
absence of.

**Sema populates the `TypeMap` for broken bodies.** A body whose `Proc` has no
`ProcSig` is still walked in full, with `params = Vec::new()` and
`ret = PoolId::ERROR` (`crates/jr-sema/src/check.rs`). The result is a *populated*
map full of `PoolId::ERROR`, not an empty one, and `expr_type` returning `None`
means "not visited" rather than "untyped" (`crates/jr-sema/src/map.rs`). So
absence is not a usable error signal, and any gate has to test for `ERROR`
explicitly.

## Decision

### 1. Blocks are an `IndexVec`, and SSA edges are block parameters

```rust
pub struct MirBody {
    pub blocks: IndexVec<BlockId, BlockData>,
    pub values: IndexVec<ValueId, ValueData>,
    pub slots:  IndexVec<SlotId, SlotData>,
    pub params: Vec<ValueId>,
    pub ret:    PoolId,
    pub entry:  BlockId,
}

pub struct BlockData {
    pub params: Vec<ValueId>,
    pub stmts:  Vec<Statement>,
    pub term:   Terminator,
}
```

A block owns its statements in a `Vec` and ends in exactly one `Terminator`. This
is rustc's shape, and it is chosen for the same reasons: passes mutate a body in
place, `BlockId` is an index and never a reference, and the whole structure is
`Clone` and cheap to share behind an `Arc` — which matters because MIR is a
memoized query result.

The one deliberate divergence from rustc is that **a phi is a block parameter,
not a statement.** Cranelift's IR reference is explicit that it "does not have phi
instructions but uses BB parameters instead", and Swift SIL uses basic-block
arguments for the same reason. Three consequences follow, and together they are
the argument:

- Braun's algorithm (see §2) creates an *incomplete* phi whenever it reads a
  variable in a block whose predecessors are not all known yet. With block
  parameters that is "push onto `BlockData::params`". With phi statements it is
  "prepend to `BlockData::stmts`", which is the only mutation pattern a
  `Vec`-of-statements is bad at, and it invalidates every cached
  `(block, statement_index)` in the body.
- The Cranelift lowering becomes a 1:1 mapping onto `append_block_param`. Phi
  statements would oblige us to write an unphi pass whose only purpose is to
  undo a representation choice.
- No pass ever needs to prepend to a block, so `Vec<Statement>`'s O(n)
  mid-block insert is confined to genuinely rare splices.

**Rejected: intrusive linked lists** (Cranelift's own `Layout`, LLVM). Cranelift
keeps entities in an arena and their *order* in a separate doubly-linked
`SecondaryMap`, which buys O(1) splicing with ids that survive movement. That is
a strictly better structure for a mature optimiser, and it stays available: it
is a change of order representation over the same ids, so adopting it later
breaks nothing. It is not worth its complexity for a mid-end that does not exist
yet.

**Rejected: a flat instruction array with structured control flow** (Zig AIR,
which is one `MultiArrayList` plus an `extra` array where `block`/`loop`/`cond_br`
are instructions whose bodies live in `extra`). Zig can afford this because AIR
has essentially no mid-end — the only AIR passes are `Liveness` and `Legalize`.
We have committed to an inliner, DCE and const-prop over a real CFG, and this
representation fights all three.

**A cached CFG, invalidated on mutable access.** `MirBody` computes predecessors
and reverse postorder lazily and caches them behind a `OnceLock` inside an `Arc`,
with mutable access to the blocks clearing the cache — rustc's `BasicBlocks`
design, taken deliberately. The bytecode lowering needs a block order and the
future mid-end needs predecessors; recomputing either per pass is waste, and
recomputing neither is a bug.

**No critical edges.** Every CFG edge has either a single predecessor or a single
successor, checked by the verifier, and passes that break it must split. This is
SIL's invariant, and it is what makes placing parallel copies on edges when
lowering block parameters to bytecode trivially correct rather than subtly wrong.

### 2. SSA is built during lowering; escaped locals stay in memory

Locals are classified before lowering. A local is **promotable** unless it is the
operand of `UnOp::AddrOf` anywhere in the body, or its type is not
register-representable. Promotable locals become SSA values via the algorithm of
Braun et al., *Simple and Efficient Construction of Static Single Assignment
Form* (CC 2013): a `current_def` table keyed on `(BlockId, LocalId)`,
`read_variable`/`write_variable`, incomplete block parameters for unsealed
blocks, and `try_remove_trivial_phi`. Everything else gets a `SlotId` and is
touched only through `Statement::Store`, `Rvalue::Load` and `Rvalue::SlotAddr`.

This is exactly what `cranelift-frontend` does — its `ssa.rs` cites Braun by
name, and the IR reference says stack slots "can have their address taken with
`stack_addr`, which supports C-like programming languages where local variables
can have their address taken". Go reached the same design independently:
`canSSAName` bails on `Addrtaken()`.

Three reasons this is right *here* specifically:

- **Braun needs no dominance analysis at all.** No dominator tree, no dominance
  frontiers. Its minimality result holds for reducible CFGs, and §Context
  establishes ours are reducible by construction — so we get pruned, minimal SSA
  for free and never implement the SCC-based path for irreducible graphs.
- **The memory form is unavoidable either way.** Jairs has prefix `*` for
  address-of and postfix `.*` for dereference (ADR-0011), so `alloc`/`load`/
  `store` must exist for escaped locals under *any* choice. Memory-first
  therefore buys no uniformity; it buys the memory form we already needed, plus
  a stack of passes to undo it on the ~90% of locals that never escape.
- **It puts the complexity in lowering rather than the mid-end.** Both choices
  have a correctness hazard. Braun's is a wrong escape classification, which is
  local, testable, and mitigated by defaulting to memory and promoting only on
  proof. `mem2reg`'s is a mis-analysis of aliasing, which surfaces as a
  miscompile far from its cause.

**Rejected: memory-first plus our own `mem2reg`.** This is rustc's shape — places
and locals, not SSA — but rustc is non-SSA *because of borrowck*, a flow-sensitive
place-based analysis we do not have, and it delegates promotion to LLVM, which
the Cranelift path does not have either. Writing it ourselves means a dominator
tree, dominance frontiers, phi insertion, renaming, and then SROA, because a
promoted `alloc` of a struct is useless until it is split into scalars. Swift
pays precisely this bill: `AllocBoxToStack`, `DefiniteInitialization`,
`EarlySROA`, `SROA`, `SROABBArgs`, and two separate redundant-load-elimination
passes. That is four-plus passes and a dominance library to reach the state
Braun reaches during a walk we are already doing.

**Rejected: neither** (Zig AIR's opportunistic promotion inside Sema, with
`alloc`/`load`/`store` surviving into codegen). It costs code quality at the
backend and gives the mid-end nothing to work with.

The classification is conservative by construction: the default is memory, and a
local is promoted only when a full walk of the body proves its address is never
taken.

### 3. One MIR unit per procedure body

The salsa query is per body, not per file. Monomorphized instances, when they
arrive, become a *separate query keyed on `(body, substitutions)`* rather than
entries in a table.

Zig's `Air.zig` states the split we are copying in one sentence: "Unlike ZIR where
there is one instance for an entire source file, each function gets its own `Air`
instance." The line lands in the same place for us — the pre-typing IR (HIR) is
per file, the post-typing IR (MIR) is per body. rustc is per-`DefId` with a
staged query chain; rust-analyzer, which is the closest analogue since it is
rustc-shaped MIR inside salsa with an interpreter consumer, has per-body
`mir_body_query` plus a distinct `monomorphized_mir_body_query`.

**Rejected: per file or per module.** Four things break. Any edit to any body
invalidates the MIR of every body in the file, putting the salsa firewall at the
wrong grain. Monomorphized bodies belong to no file, so they would need a
synthetic unit anyway — meaning per-body granularity for exactly the bodies where
recompute is most expensive. Memoized values grow, and salsa's per-revision
comparison cost grows with them. And the inliner acquires two code paths,
same-unit and cross-unit, for no benefit.

**The consequence, stated so it is not a surprise:** once the inliner exists,
every caller takes a dependency on every callee's MIR, so editing a widely
inlined leaf invalidates its whole fan-in. This is inherent to inlining, and the
mitigation is structural: the *built* MIR query must have **no** cross-body
dependencies, and only a later *optimized* MIR query may read callee bodies. That
is rustc's `mir_built` → … → `optimized_mir` staging, and it is why the query
this wave adds is the unstaged one. Cross-body reads, when they come, go through
the callee's own query — never a side table, which would defeat salsa's
invalidation entirely.

### 4. A poisoned body is refused, not lowered

Lowering returns `Result<MirBody, Poisoned>`. It refuses a body that contains an
`Expr::Error`, or any expression or local whose type is `PoolId::ERROR`, or whose
`Proc` has no `ProcSig`. `Poisoned` distinguishes two cases:

```rust
pub enum Poisoned {
    Here(&'static str),   // this body is broken
    Transitive(ProcId),   // a body this one needs is broken
}
```

`Result` in the query's return type is the point: no consumer — not the future
VM, not Cranelift — can be handed poisoned MIR, because there is nothing to hand
them. This is rust-analyzer's guard verbatim: `if infer.has_type_mismatches() ||
infer.is_erroneous() { return Err(MirLowerError::HasErrors) }`. The
`Here`/`Transitive` split is Zig's, which keys `failed_analysis` and
`transitive_failed_analysis` separately so that dependents neither receive junk
nor re-report someone else's error.

**Refusing emits no diagnostic.** A poisoned body is poisoned because sema
already reported the cause; a second message about the same line is noise. This
extends the poison discipline `jr-sema` established, where `PoolId::ERROR` flows
silently and the invalid corpus produces zero sema diagnostics.

**A verifier makes the failure loud.** Debug builds assert that no `PoolId::ERROR`
appears in any value, statement, terminator, slot or block parameter of a
constructed body, that every block ends in a terminator, that no edge is
critical, and that every `ValueId` is defined before use. Because types are
interned, the error check is one index comparison per occurrence. The failure
mode this ADR exists to prevent — MIR silently built from poison — becomes a
crash in CI rather than a wrong binary.

**One thing refusal cannot cover, so the caller must.** The gate can only see the
error signal `jr-sema` leaves behind, which is `PoolId::ERROR` in the `TypeMap`.
Not every reported error poisons a type: `x: u8 = 300;` raises E0204 and then
type-checks as `u8`, so nothing in the types distinguishes it from a correct
program. `jr-mir` is a pure function over HIR plus types and is handed no
diagnostics, so it cannot close that hole from the inside.

Therefore **no caller may request the MIR of a file whose `file_diagnostics`
reports errors.** This is the one respect in which the "require the caller to
check for errors first" option — rejected above as the *general* policy, because a
check every caller must remember is a check some caller will forget — remains
load-bearing. It is narrower here than it would have been as the whole design:
there is exactly one caller, the `jr-db` query, and the obligation is discharged
in one place rather than at every consumer. `jr-mir`'s `tests/lowering.rs` pins
the resulting behaviour with a test asserting that such a body *does* lower, so
that a future reader finds the division of responsibility documented rather than
mistaking it for an oversight.

**Rejected: lower it and carry a taint flag.** This is rustc's other half:
`body.tainted_by_errors`, one guard at the pass-manager entry, and
`span_delayed_bug` to ICE if poison was reached while no diagnostic was emitted.
It is the right choice when a *diagnostic* needs the MIR of a broken body, and it
is worth revisiting if one ever does. It was rejected because a boolean is
something every future consumer can forget to check, whereas a `Result` is
something the compiler will not let them forget. Swift moved from an implicit
notion to a structural `sil_stage raw`/`canonical` marker for the same reason.

**Rejected: lower to a trap body.** rustc's `construct_error` fabricates a
signature-correct body terminated by `Unreachable`. As a general policy it is
worse for comptime specifically: a trap turns one type error into an arbitrary
compile-time value, and the VM's job is to produce values other bodies depend on.
A refusal at a `#run` site is a diagnostic; a trap is a silent wrong answer. The
narrow case it is right for — a body that must physically exist because its
address was taken, or keep-going object emission — has no consumer in this slice,
and is a helper to add when one appears.

### 5. Layout is not MIR's

MIR is typed but not laid out. A field access is a place projection carrying a
field index, never a byte offset; an aggregate is a value or a slot with no
computed size. Nothing in this crate knows a size or an alignment.

The Pool stores a struct's identity as a bare `DeclId` with source field order
and no layout (ADR-0015), and sema hardcodes `string`'s `.data`/`.count` as
pseudo-fields, so ADR-0004's `{data: *u8, count: s64}` remains prose. That gap is
real and is deliberately left open here.

Layout belongs where the target ABI does, which is `jr-codegen-clif`. The reason
is §3.1's invariant rather than tidiness: the VM and Cranelift must agree on
layout exactly, so it wants to be *one* shared computation — most likely a Pool
query added when the second consumer appears and can constrain its shape.
Computing it now, in a crate with no target knowledge and one consumer, would
bake in assumptions with nothing to check them against.

**Rejected: compute it in `jr-pool` now.** It front-loads work both backends
need and makes ADR-0004 executable, but it puts target-dependent knowledge into a
crate that currently has none, on a wave whose job is lowering.

**Rejected: compute it in `jr-mir`.** Then the VM depends on `jr-mir` for layout
or duplicates it, and duplicated layout is exactly the class of divergence the
same-MIR invariant exists to prevent.

## Consequences

### Positive

- The mid-end starts from real SSA with block parameters, so DCE and const-prop
  are straightforward and `mem2reg` is never written at all.
- The Cranelift lowering is close to mechanical: blocks map 1:1, block
  parameters map onto `append_block_param`, and slots map onto stack slots with
  `stack_addr`.
- MIR is `Clone`, has no interior references and no spans-as-offsets, so it sits
  in salsa behind an `Arc` without ceremony. There is no need for rustc's `Steal`,
  whose own documentation calls the mechanism "a bit dodgy".
- The CFG that §1 requires is exactly what the two diagnostics sema deferred —
  definite assignment and missing `return` — need, so they land in the same wave
  as the structure they depend on. Stray `break`/`continue`, which no pass
  currently checks at all, comes free with it.
- `Result` at the query boundary means a miscompile from poison is not
  expressible.

### Negative

- Braun's sealed/filled bookkeeping lives inside lowering, so lowering is more
  intricate than a straight syntax walk. A wrong escape classification is a
  correctness bug rather than a performance one.
- Once the inliner exists, per-body granularity means a widely inlined leaf's
  fan-in is invalidated on every edit. Mitigated, not removed, by keeping the
  built-MIR query free of cross-body dependencies.
- Deferring layout means field access stays symbolic through the whole mid-end,
  so no pass in this wave can reason about offsets or aliasing between fields.
- The no-critical-edges invariant is an obligation on every future pass, not
  just on lowering. It is verifier-enforced, which converts forgetting it into a
  test failure rather than a wrong answer.

### Follow-on work this forces

- A shared layout computation before *either* backend emits code, positioned so
  the VM and Cranelift cannot disagree (§5).
- `Statement`/`Terminator` must be lowerable to bytecode as well as to Cranelift
  IR. The bytecode path additionally needs an RPO linearization and parallel
  copies on edges to eliminate block parameters — which the no-critical-edges
  invariant is what makes correct.
- `Poisoned::Transitive` has no producer until something reads across bodies.
  The inliner is the first, and it must propagate rather than re-report.
- MIR stores HIR ids, not text offsets, and resolves spans only when rendering a
  diagnostic. ADR-0013 already deferred `AstIdMap` for HIR; MIR must not deepen
  that debt by copying byte ranges into a memoized structure.

## Alternatives considered

The three representation alternatives (§1), the two SSA strategies (§2), the
per-file unit (§3), the two poison policies (§4) and the two layout homes (§5)
are each argued at their point of decision above, with the project that chose
them named, rather than being restated here.

One cross-cutting alternative deserves recording: **optimising generic MIR once
and instantiating afterwards**, which is rustc's arrangement and which the MIR
optimisation guide argues for directly — "since MIR is generic (not monomorphized
yet), these optimizations are particularly effective; we can optimize the generic
version, so all of the monomorphizations are cheaper". `PLAN.md` §3.1 already
committed to monomorphized MIR, following Zig, for the sake of a single execution
semantics shared by the VM and the backend. The cost is that the mid-end runs
once per instantiation. If that cost bites, the escape hatch is a two-level
split — a generic body optimised once, then a cheap per-instantiation query — and
it does not require revisiting anything in this ADR.

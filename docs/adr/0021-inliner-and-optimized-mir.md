# ADR-0021: The inliner, a staged optimized-MIR query, and the `#run` closure it must not touch

- **Status:** Accepted
- **Date:** 2026-07-27
- **Deciders:** dboll

## Context

ADR-0009's follow-on work says a real inliner must live in `jr-mir` *before any backend
consumes MIR*. ADR-0019 §6 deliberately relaxed that ordering and named the two
conditions that end the deferral: the first `#expand`, or the first performance number
`PLAN.md` proposes to publish. Neither has arrived on its own, but §1.4's exit criterion
is now met and `PLAN.md` §1.3's estimate is waiting on a number that cannot honestly be
taken without a mid-end. The deferral expires here.

Four things about the existing code decide the shape, and all four were read rather
than assumed.

**A call is an rvalue, not a terminator.** `Rvalue::Call` appears inside
`Statement::Assign` or `Statement::Discard`, in the middle of a block. rustc's
`TerminatorKind::Call` splits the block for free; ours does not, so inlining must split
a block at the call statement itself. This is the single largest piece of work in the
wave and it is a consequence of ADR-0017 §1, not a defect in it — a call that cannot
unwind has no second edge to justify a terminator.

**ADR-0017 §3 already names the resolution direction.** It states that the *built* MIR
query must have no cross-body dependencies and that only a later *optimized* query may
read callee bodies, citing rustc's `mir_built` → `optimized_mir` staging, and it accepts
the fan-in invalidation cost in principle. So the collision `PLAN.md` §7 flags is not a
question of whether, only of which query and at what granularity.

**Comptime does not go through `file_mir` at all.** `file_consts` calls
`jr_mir::lower_file` *directly*, and its own docs say why: `file_mir` consumes
`file_consts`, so calling it from there would be a salsa cycle. §3.1's invariant is
therefore satisfied today by both engines running the same *function*, not by their
sharing a memoized value. Any query that inlines sits strictly downstream of
`file_consts` and can never be what comptime executes. The cycle is real in the exit
criterion's own file: `024-hello.jr` has `COMPUTED :: #run add(2, 3)` calling a body in
that same file.

**A cross-file `#run` does not work today.** `file_consts` lowers only its own file's
HIR, so a `Callee::Direct` naming another file has no body in the map it hands the VM
and evaluation fails with E0230. `PLAN.md` §7 lists this as open. That accident is what
makes §3 below sound, which is why §3 pins it with a test rather than relying on it
quietly.

## Decision

### 1. A new per-file `optimized_file_mir` query does the inlining

`file_mir` is untouched and keeps its name, its shape and its role as ADR-0017 §3's
unstaged query. A new tracked query

```rust
optimized_file_mir(db, file, search_paths) -> MirResult
```

reads `file_mir(file)` plus `file_mir(callee_file)` for each distinct cross-file callee,
and is what `jr run` and `jr build` consume. Diagnostics
(`module_loader`'s use at the `file_diagnostics` seam) and `dump_mir` stay on built MIR.

Granularity stays per file because `jr-db`'s whole query surface is, and because — as
`crates/jr-db/src/mir.rs` already argues — a `ProcId` is an index into one file's HIR
and so is not a salsa key on its own. The cost is stated rather than discovered: editing
`modules/Basic` invalidates the optimized MIR of every importer wholesale, where a
finer key would invalidate only the bodies that actually inlined something from it. The
fan-in invalidation itself is inherent to inlining and ADR-0017 §3 accepted it; only its
coarseness is new.

**Rejected: introduce the interned `(file, proc)` key now.** This is the full rustc
shape — `mir_built(body)` / `optimized_mir(body)` — and it is where this eventually
goes. It was rejected for this wave because it is a new salsa key type plus a rewiring
of every consumer that currently takes a whole `FileMir` (the VM's `add_file`, the back
end's driver, the dump, the corpus tests), in a wave whose subject is the inliner. It
also buys *only* granularity: it does not help with §2, because the `#run` cycle is
between a body and itself in the same file and body-grain keys reproduce it exactly.
`mir.rs`'s existing note — that the split worth making first is `mir_built` versus
`optimized_mir` — is the split this ADR makes; the key type follows when a consumer
needs body grain, which is most likely monomorphization.

**Rejected: inline inside the existing `file_mir`.** No new query, least code. It
directly violates ADR-0017 §3: every caller's *built* MIR would depend on every
callee's. Worse, the unoptimised MIR would stop existing, and that is the
representation that corresponds to the source a user is looking at — the MIR dump and
the LSP would both start describing code the programmer did not write.

### 2. The inliner refuses to modify any body the `#run` closure reaches

`optimized_file_mir` computes the transitive set of procedures reachable from any
`#run` or file-level constant initialiser in the file, and leaves every body in that
set byte-identical to its built form. It may still inline *into* any body outside the
set, and it may inline a set member *as a callee* — copying a body does not change it.

The consequence is that for every body comptime executes, comptime's MIR and runtime's
MIR are the same MIR, by construction; and for every body comptime never executes there
is nothing that could diverge. §3.1's invariant survives **unamended**. In
`024-hello.jr` the closure is `{add}`, so `add` itself is untouched while the `add` call
inside `main` is inlined — the deferral in ADR-0019 §6 genuinely ends, in the exit
criterion's own file.

The closure is computed from HIR plus `imported_procs` and callee bodies, never from
const *values*, so it introduces no dependency on `file_consts` and no cycle.

**This is sound only because a cross-file `#run` is refused.** A `#run` in file *G*
calling a body in file *F* would have `optimized_file_mir(F)` optimising a body that
*G*'s comptime executes unoptimised, and `optimized_file_mir(F)` cannot see *G* — salsa
has no reverse dependencies, and a reverse import graph is not something a per-file
query can ask for. Since `file_consts` lowers only its own file, that program does not
exist today. A test asserts the refusal, so that whoever implements a cross-file `#run`
fails *here*, loudly, instead of shipping a comptime/runtime divergence. That is the
whole reason the guard is a test and not a comment.

**Rejected: let comptime run unoptimised and runtime run inlined, and restate §3.1.**
Much less machinery: no closure, no exclusion, and the honest framing that an
optimisation is semantics-preserving so the two need only *agree*, with
`differential.rs` as the enforcement. Rejected because it converts an inliner bug into a
comptime/runtime divergence, which is precisely the failure class §3.1 exists to make
impossible — and the differential only sees programs the corpus actually runs, which is
two of fifteen that print anything at all. This project's two silent miscompiles were
both cases where something that should have been compared was not.

**Rejected: inline in the back end.** Relocates the same divergence instead of removing
it, and contradicts ADR-0009's core decision — untouched by ADR-0019 §6 — that the
inliner is ours and lives in MIR. Cranelift cannot inline, so a future `#expand` would
have nowhere to expand.

### 3. An inlined body's spans become the call site's span

Every `MirSpan` copied out of a callee — on a statement, a `ValueData` or a `SlotData` —
is replaced by the span of the call that pulled it in. One rule, applied at the splice.

This is not only about diagnostic quality. `MirSpan::Expr(ExprScope, ExprId)`,
`MirSpan::Local`, `MirSpan::Stmt` and `MirSpan::Param` all name arenas belonging to the
*callee's* file, and `resolve_span` is handed the *caller's* `FileHir`. Keeping a
callee's span in a caller's body would index the wrong file's arenas — a bare `ExprId`
collision of exactly the kind `ExprScope` was introduced to prevent, and one that
resolves to a plausible wrong span rather than to nothing. The rewrite makes a body
contain only spans of its own file.

That property is guaranteed **structurally rather than by a verifier check**, and the
distinction is worth stating because the obvious reading is the other one. A `MirSpan`
names an `ExprId`, a `BodyId` or a `ProcId`; none of them carries a `FileId`, and a
`MirBody` does not store the `BodyId` it was lowered from — so a verifier cannot tell a
foreign `Expr` span from a native one. What it *can* check is the one variant that names
a procedure, and it now reports a `MirSpan::Param` naming any procedure other than the
body's own. The real guarantee is that every span the splice writes goes through **one
function that returns the call site's span and takes no other input**, which is the same
choke-point shape ADR-0020 §4 used to stop a bytecode instruction from being emitted
without a span. A copy site cannot pass a callee span through it because there is no
parameter to pass one in.

ADR-0020 §1/§2's two-line message shape and its single formatter are untouched, so
`differential.rs`'s asserted bytes stay stable and both engines keep rendering from the
same data.

**Rejected: carry an inline stack per span.** rustc's shape: `MirSpan` grows a chain of
call sites and a trap prints a frame per level. Strictly better diagnostics, and it is
the eventual answer. Rejected for this wave because it changes the rendered bytes, so
`trap_message`, both engines and every differential expectation move in lockstep in a
wave whose subject is the inliner — and ADR-0020 §2's "one formatter decides" becomes a
formatter with a loop in it. It wants its own decision when `#expand` arrives, because
there an inline stack is a semantic and not a nicety: a user writing a macro needs to
know which expansion trapped.

**Rejected: leave the callee's spans alone.** Known-wrong output — a trap naming a file
the program never mentions — and unsound resolution besides. Listed only to be rejected
explicitly, because "spans just come along with the statements" is what a first
implementation does by default.

### 4. A callee is inlined when it is a leaf under a small statement threshold

The predicate, in full: the callee is `Callee::Direct`; its body is present and `Ok`;
it contains no `Rvalue::Call` of its own; its statement count is under a named
constant; and the caller is outside §2's closure.

"No call of its own" is doing two jobs. It bounds the work — one splice per call site,
no iteration to a fixed point — and it makes termination structural rather than
enforced by a depth counter: a recursive procedure calls something, so it is not a leaf,
so it is never inlined, and neither is any member of a mutual-recursion cycle. There is
no recursion check in the code because there is no code path that needs one.

The threshold is an unmeasured guess and carries a doc comment saying so. That is
tolerable precisely because §1.3's performance number — the thing that would justify a
real number — is downstream of this wave.

**Rejected: a general cost model.** Statement-count cost, single-caller bonus, depth
limit, recursion cut-off: the shape every production inliner converges on. Rejected
because it is several tuning knobs with no benchmark to tune them against, so they would
be set by taste and then become hard to change once corpus snapshots depend on them.
The leaf rule is a strict subset of it and upgrading is additive.

**Rejected: annotation-driven only.** Inline where a directive asks. Jairs-0 has no
`#inline` and no `#expand`, so nothing would inline and ADR-0019 §6's deferral would be
renamed rather than ended.

## Consequences

### Positive

- ADR-0017 §3's staging exists rather than being described, and the crate's own docs
  stop saying "the mid-end is a later wave".
- §3.1's invariant is preserved *structurally* rather than by trusting that an
  optimisation is semantics-preserving. The bodies comptime runs are bit-identical in
  both engines.
- The built MIR remains available, so the dump, the corpus snapshots and the LSP keep
  describing the program the user wrote.
- `Statement::Nop` and `Poisoned::Transitive`, both declared for the inliner and unused
  since ADR-0017, acquire their first producer — a splice leaves the call statement as a
  `Nop`, and a refused callee propagates rather than re-reports.
- A trap in an inlined body names the call, and a copied span cannot name the callee's
  arenas because the splice writes every span through one nullary choke point.

### Negative

- Optimized-MIR invalidation is at file grain, so editing a widely imported leaf
  invalidates more than it needs to. §1's rejected alternative is the fix and it is
  additive.
- Two MIRs now exist for one file, roughly doubling memoized MIR memory for any file
  whose bodies are consumed. Accepted: it is what having a mid-end costs, and the
  built copy is the one diagnostics and the editor need anyway.
- Comptime-heavy code is the one place that stays uninlined, which is the opposite of
  what ADR-0009's ordering wanted. It is the price of §2 and it disappears when a
  cross-file `#run` and a body-grain key make a shared optimized query possible.
- Trap precision inside a callee is lost: two overflow sites in one inlined body both
  report the call. §3's rejected alternative is the fix.
- The threshold is arbitrary until a benchmark exists.

### Follow-on work this forces

- **Into this wave:** the closure computation and its refusal test; the span choke point
  and the verifier's `MirSpan::Param` check; `differential.rs` must stay green,
  including its asserted trap location.
- **Into the next mid-end wave:** DCE and const-prop, both of which the absence of a
  mid-end makes visible — a MIR dump still shows unreachable blocks, and `print_line` in
  `modules/Basic` keeps a spill slot it never reads. Then the first honest performance
  number, which is what §1.3 has been waiting for.
- **Into whichever wave enables a cross-file `#run`:** §2's soundness argument. The test
  that pins the refusal is the tripwire; the fix is either a cross-file closure or the
  body-grain key that lets comptime and runtime share one optimized query.
- **Into wave W1 or wherever `#expand` lands:** §3's inline stack, which a macro makes a
  semantic requirement rather than a diagnostic improvement.

## Alternatives considered

Each fork's rejected alternatives are argued at its own point of decision. One
alternative spans the whole ADR.

**Do the packaging instead and leave the mid-end for later.** §1.4's three open boxes
are VS Code packaging, Neovim packaging and a Linux CI run, and none of them is
compiler work, so this wave could have been skipped in favour of closing the slice on
paper. It is rejected because ADR-0019 §6 named an expiry rather than a vague "later"
precisely so that the deferral could not be renewed by whichever task looked closer to
done — and because every wave that passes without a mid-end is a wave whose performance
claims cannot be made. The packaging is not blocked by this and is not made harder by
it.

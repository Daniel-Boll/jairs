# ADR-0018: VM shape — register bytecode, layout in the pool, const-eval as a query

- **Status:** Accepted
- **Date:** 2026-07-26
- **Deciders:** dboll

## Context

`jr-vm` is being built. Everything upstream of it exists: the lexer, the
error-recovering parser, the lossless CST, HIR with name resolution, the module
loader, the `InternPool`, `jr-sema`, and — as of ADR-0017 — typed SSA MIR.

`PLAN.md` §3.1 states the invariant the VM exists to serve:

> **The load-bearing invariant:** comptime and runtime execute *the same* MIR. The
> VM consumes bytecode lowered from the identical MIR that Cranelift consumes.
> Any other arrangement guarantees `#run` and runtime silently disagree.

That sentence fixes two things and leaves four open. It fixes that there *is* a
bytecode, and that the bytecode is lowered from MIR rather than from anything
earlier. It leaves open:

1. What the bytecode's instructions address — registers, a stack, or nothing at
   all.
2. Where the one shared size/alignment/offset computation lives. ADR-0017 §5
   deliberately deferred this, on the grounds that the VM and Cranelift must
   agree exactly and the second consumer should get to constrain the shape. The
   VM is that second consumer, so the deferral expires here.
3. Where a `#run` expression is actually evaluated. ADR-0016 §4 says sema does
   not fold it and PLAN.md's pipeline diagram draws const-eval as `SEMA <--> VM`,
   but taken literally that is a crate cycle — see §3.
4. How the VM calls a `#foreign` procedure. This is not optional and not
   deferrable: `PLAN.md` §1.4's exit criterion is
   `tests/corpus/valid/024-hello.jr` producing output, that file calls `print`,
   and `print` is `write :: (fd: s64, buf: *u8, count: s64) -> s64 #foreign libc
   "write"`. A VM that cannot make that call cannot run the slice's one program.
5. Whether a cross-file call is representable. The same exit criterion forces
   this too, and it was found only while implementing §3 — see §5.

Three facts about the existing code narrow the space, and all three were verified
rather than assumed:

**MIR is already in register form.** `MirBody` hands out dense `ValueId`s, each
defined exactly once and verified to be defined before use, plus
`reverse_postorder()` — documented in `mir.rs` as "the order the bytecode
lowering will linearise in". Whatever the bytecode addresses, the information
needed to address registers is already computed.

**Nothing in the workspace computes a byte.** `Projection::Field` carries an index
into `Pool::struct_fields`; `Projection::StringData` and `Projection::StringCount`
are symbolic precisely so that no crate asserts a layout for `string`; and
ADR-0004's `string = {data: *u8, count: s64}` exists only as prose, with
`jr-sema` hardcoding the two names as pseudo-fields. There is no prior art in the
tree to be consistent with, and no second implementation to diverge from yet.

**`jr-mir` depends on `jr-sema`.** `crates/jr-mir/Cargo.toml` lists
`jr-sema.workspace = true`, and `jr-vm` must depend on `jr-mir` to consume MIR.
So `jr-sema` calling the VM would close a cycle `jr-sema → jr-vm → jr-mir →
jr-sema`, which Cargo rejects outright. This is why §3 is a decision and not an
implementation detail.

## Decision

### 1. The bytecode is a register machine addressed by `ValueId`

A frame is a flat array of runtime values indexed by `ValueId`, sized once from
`MirBody::value_count()`. Every instruction names its destination register and
its operand registers. There is no operand stack and no `dup`/`swap`.

```text
%3 = add %1, %2        ; not: push %1; push %2; add
```

This follows from MIR rather than being imposed on it. MIR is SSA with dense,
single-assignment `ValueId`s, so "which register holds this operand" is already
answered by the IR; a stack machine would have to *discard* that answer and
recover an evaluation order to replace it.

Three consequences, and together they are the argument:

- **Lowering is a transliteration.** Each `Statement::Assign` becomes one
  instruction whose destination is `dest`. `Rvalue`'s operands are already
  `Operand::{Value, Constant}`, which map onto a register index and a constant
  index respectively. There is no scheduling pass, because there is nothing to
  schedule.
- **Block parameters become parallel copies on edges, and that placement is
  unambiguous.** ADR-0017 §1's no-critical-edges invariant guarantees every edge
  has either one predecessor or one successor, so the copies for an edge always
  have exactly one place to go and never need an edge split at lowering time.
  The copies are *parallel*: a swap between two block parameters must not be
  serialised naively, so lowering emits a temporary when the copy graph has a
  cycle.
- **A bytecode dump reads like the MIR beside it**, because both name `%n`. When
  the VM and Cranelift eventually disagree about a program, that diff is the
  first tool anyone reaches for, and it is worth designing for.

The cost, stated plainly: instructions are wider than a stack machine's, because
each carries two or three register indices instead of an implicit stack position.
No measurement in this project says that matters, and the comptime workload is
`#run add(2, 3)`.

**Rejected: a stack machine.** Smaller encoding and a tighter dispatch loop, both
real. It was rejected because it needs a pass to linearise each SSA definition
tree onto a stack, plus `dup` traffic wherever a value has more than one use —
and that pass is pure overhead here, since it exists only to rediscover the
operand order MIR already states. Every widely-cited reason to prefer a stack
machine (compact code, easy verification, no register allocator) is a reason that
applies when the *source* IR is a tree. Ours is not.

**Rejected: no bytecode, interpret `MirBody` directly.** This is the cheapest
thing to write, and rust-analyzer's MIR interpreter does exactly it. It was
rejected on the invariant: PLAN.md §3.1 commits to "bytecode lowered from the
identical MIR", and walking `MirBody` while Cranelift lowers it means two
independent readings of the same structure — which is the divergence the
same-MIR invariant exists to prevent, relocated rather than removed. It also
gives up the one artefact that makes a comptime-versus-runtime disagreement
debuggable, namely a printable instruction stream.

### 2. Layout is a `jr-pool` module, with the target passed in

`jr-pool` gains a `layout` module computing size, alignment and field offsets over
a `PoolId`. It is *not* implicitly host-targeted: the caller passes a
`TargetLayout` describing pointer width and alignment, and the module is a pure
function of `(Pool, TargetLayout, PoolId)`.

This is what ADR-0017 §5 predicted in as many words — "one shared computation …
most likely a Pool query added when the second consumer appears and can constrain
its shape". The second consumer has appeared, and it constrains the shape as
follows: the VM needs offsets to implement `Projection::Field`, sizes to allocate
`SlotData`, and a committed answer for `string` in order to hand `s.data` and
`s.count` to `write`.

It goes in the pool because the pool already owns every input. Struct identity is
a `DeclId`, the field list is `Pool::struct_fields`, and pointer types are
structural and nested — so layout is a fold over data `jr-pool` holds and nobody
else does. Putting it anywhere else means re-exposing that data.

**This makes ADR-0004 executable.** `string` becomes a real two-field layout —
`data` at offset 0 with pointer size, `count` at the pointer-aligned offset after
it with size 8 — and `Projection::StringData`/`StringCount` become offsets rather
than symbols. ADR-0004's representation stops being prose. It remains *not* the
struct type of that shape, exactly as ADR-0015 §2 requires: the layout is shared,
the identity is not.

**Rejected: a new `jr-layout` crate.** It would keep `jr-pool` free of any notion
of a target, which is a real gain in separation. Rejected because the crate would
have to re-expose enough of the `Pool` to walk struct fields and pointees, so the
coupling survives the split and only the crate count changes. It stays available:
moving a module to its own crate later breaks no caller that used the re-export.

**Rejected: `jr-codegen-clif`, which is what ADR-0017 §5 literally names.** That
sentence said layout "belongs where the target ABI does", and it is right about
where target *ABI* belongs — argument passing, register classes, struct return
conventions. It is wrong about size and offset for one practical reason:
`jr-codegen-clif` is an empty crate, and making `jr-vm` depend on the *backend*
in order to run a program inverts the dependency the pipeline actually has. The
alternative, the VM computing its own, is the duplicated layout ADR-0017 §5
rejected twice.

**Rejected: ad-hoc constants inside `jr-vm`, unified when Cranelift lands.**
Fastest to green, and precisely the failure ADR-0017 §5 deferred the decision to
avoid. A layout the VM invents privately is a layout Cranelift will disagree with
silently, and the symptom is `#run` and runtime computing different field values.

### 3. Const evaluation is a `jr-db` query, not a fold inside `jr-sema`

A new tracked query sits between `checked` and `file_mir`. For each `#run`
expression and each file-level constant initialiser, it lowers a **synthetic MIR
thunk**, runs it in the VM, and interns the result in the pool. `lower_file` gains
a const-value map input, exactly like the `types` map it already takes, and
ADR-0017's two standing refusals — `#run has no value until jr-vm` and `a file-level
item has no value until jr-vm` — become lookups in it.

`jr-sema`'s dependency list does not change. PLAN.md's `SEMA <--> VM` arrow
survives as a description of the *pipeline*, where sema's answer is consumed by an
evaluator and the evaluator's answer flows back into lowering; it does not survive
as a description of the crate graph, where it is a cycle (see §Context).

Three reasons this is the right home:

- **Evaluation is genuinely ordered, and `jr-db` is the only layer that already
  orders things.** In `024-hello.jr`, `main` needs `COMPUTED`'s value, `COMPUTED`
  is `#run add(2, 3)` and needs `add`'s MIR, and `add` needs nothing. That is a
  dependency graph over declarations, it can be cyclic in a program that deserves
  an error, and it therefore needs a traversal with an in-progress set. Putting
  that traversal in the crate whose entire job is query wiring puts it next to the
  module loader's cycle tolerance, which solved the same shape of problem.
- **It keeps `jr-mir` a pure fold.** `lower_file`'s contract is a function of HIR
  plus types; adding a third input map keeps it one, whereas handing it a callback
  that re-enters lowering would not.
- **The gate stays in one place.** `file_mir` already discharges ADR-0017 §4's
  caller obligation by refusing to lower a file with errors. The const query sits
  behind the same gate, so nothing evaluates a thunk built from poison.

**Rejected: dependency-inject an evaluator into `jr-sema`.** A `ConstEval` trait
declared in sema and implemented in `jr-db` over `jr-vm` breaks the cycle and lets
sema literally fold `#run`, which is the most faithful reading of ADR-0016 §4.
Rejected because it makes the type checker re-entrant into MIR lowering in the
middle of a check — sema would have to lower a thunk, which needs the types sema
is still computing — and it puts the evaluation order and its cycle guard inside
the one pass that has no notion of order.

**Rejected: a callback passed into `lower_file`.** Fewest moving parts, no new
query, and only `jr-mir` needs the values so only `jr-mir` would ask. Rejected
because it hides an ordered, cycle-prone traversal inside a function whose module
docs promise a pure fold, and because a callback's results are invisible to salsa:
the same thunk would be re-evaluated once per lowering rather than memoized once.

### 4. Foreign calls go through libffi; the comptime gate is unchanged

`jr-vm` carries the libffi bridge ADR-0006 anticipated, and resolves a
`#foreign` procedure's symbol at runtime. Two things follow, and they must not be
conflated:

- **A foreign call while *running* a program (`jr run`) is ungated.** It is the
  program doing what it says it does. `024-hello.jr` printing is this case.
- **A foreign call while *evaluating comptime code* (`#run`) stays refused**, per
  ADR-0006, until wave W6 introduces `#foreign_at_comptime`. The bridge exists;
  the allowance does not. The VM therefore carries an execution *mode*, and the
  comptime mode rejects a foreign call with a diagnostic rather than performing
  it.

This is the distinction ADR-0006 draws but had no code to draw it in. It is worth
naming explicitly because the two paths share one bridge, and a bridge with no
mode would silently grant comptime FFI years early.

**Symbol resolution has to be redone here.** `ForeignInfo::library` is still an
unresolved `Option<Symbol>` in the HIR: `jr-sema` checks that it names a library
(E0225) and records nothing, so the VM resolves `libc :: #system_library "c"` to a
loaded library itself. This is not new debt — ADR-0006's follow-on work assigned
FFI resolution to the consumer — but it is the second pass over the same
declaration, and the day a third appears the answer belongs in the pool next to
`ForeignLibraryValue`.

**This adds one dependency.** `libffi` was already pinned in
`[workspace.dependencies]` for exactly this ADR-0006 obligation, but it performs
dynamic *calls*, not dynamic *symbol lookup*. Portable `dlopen`/`dlsym` is added
as `libloading`, under the workspace's caret-range convention rather than the `=`
pin ADR-0009 reserves for `cranelift-*` and `salsa`. Hand-rolling `extern "C" { fn
dlsym(...) }` was considered and rejected: `RTLD_DEFAULT` is a null handle on
glibc and `-2` on Darwin, and a per-platform constant transcribed by hand into a
compiler is a footgun with no upside.

**Rejected: a native intrinsic table for `write` and `exit` only.** The VM would
match a `#foreign` symbol against a handful of Rust implementations and error on
anything else, which runs `024-hello.jr` this wave with no C toolchain dependency
and keeps libffi for its own wave. Rejected because the table is a second
implementation of the FFI boundary that has to be deleted later, and because it
answers the easy half: the moment a program declares a `#foreign` function outside
the table, the failure is "this compiler does not support your program" rather
than a missing symbol. The bridge is the thing that generalises.

**Rejected: `jr run` supports only programs with no foreign calls.** Honest, and
it leaves the wave without its headline evidence — `jr run 024-hello.jr` would
execute and print nothing, deferring §1.4's exit criterion and reducing this
wave's end-to-end proof to `#run` folding.

### 5. `Callee::Direct` names `(FileId, ProcId)` — an amendment to ADR-0017

ADR-0017 left `Callee::Direct(ProcId)`, and `ProcId` indexes *one* file's
`FileHir::procs`, so a call into an imported module had no representable callee
and was refused. That refusal is amended here: `Callee::Direct` carries a
`ProcRef { file: FileId, proc: ProcId }`, and `jr-db` passes `lower_file` a map
resolving each imported callee to one — the same shape as the const-value map of
§3.

This is forced rather than chosen. `PLAN.md` §7 simultaneously puts `jr run` and
§1.4's exit criterion in this wave *and* assigns the cross-file-call refusal to
the inliner, which are contradictory: `024-hello.jr` calls `print`, `print` lives
in `modules/Basic`, so the moment §3 gives its `main` the value of `MESSAGE` and
`COMPUTED`, lowering fails one line later on the callee instead. The exit
criterion cannot be met without this.

**It does not weaken ADR-0017 §3.** That section's rule is that the *built* MIR
query must have **no** cross-body dependencies, so that editing a widely inlined
leaf does not invalidate its whole fan-in. A cross-file *call* needs the callee's
**signature** — to know it is a procedure and to type the arguments, both of which
`jr-sema` has already done — and never its body. Resolution therefore rides on
`file_signatures`, which ADR-0016 §5 already established depends only on the other
file's HIR. The callee's *body* is fetched by the VM at call time, through that
file's own query. The staged `mir_built` → `optimized_mir` split ADR-0017 §3 asks
for is still the next split to make, and is still unmade.

**Rejected: defer, and accept that `024-hello.jr` cannot run this wave.** The
smallest honest option: `020-run-directive.jr` would lower fully and `#run` would
get a value, which is real end-to-end proof, while `018-import.jr` and
`024-hello.jr` stayed refused. Rejected because the exit criterion is the point of
the slice, and a VM that cannot run the slice's one program is hard to call done.

**Rejected: inline imported callees during lowering.** No cross-file callee
identity would be needed at all. Rejected outright: it is the cross-body read
ADR-0017 §3 forbids in the built-MIR query, it puts the salsa firewall at the
wrong grain, and it makes the inliner's first appearance an accident instead of a
design.

## Consequences

### Positive

- Bytecode lowering has no scheduling pass and no register allocator, because
  MIR's `ValueId`s *are* the register numbers and `reverse_postorder()` is the
  block order.
- ADR-0004 becomes executable rather than prose, and it becomes so in one place
  that Cranelift will later call, so the two backends cannot disagree about
  `string`.
- Two of the three MIR refusals enumerated in
  `crates/jr-db/tests/mir_corpus.rs` disappear, and the resulting snapshot diff
  is the proof the VM works — a better test than asserting the refusals do not
  happen.
- The third disappears too, via §5, so the valid corpus lowers with no refusals
  at all and `024-hello.jr` becomes runnable.
- `#run` gets a value, which closes ADR-0016 §4's stated cost ("`#run` results
  have types and no values").
- The comptime/runtime FFI distinction becomes a mode in code rather than a
  sentence in ADR-0006.

### Negative

- Register instructions are wider than stack instructions, and a frame is sized
  by `value_count()` rather than by live range, so a body with many short-lived
  values over-allocates. A liveness pass would fix it and is not written.
- Layout in `jr-pool` means that crate now knows a target exists, which it did
  not before. The knowledge is confined to a parameter, but the parameter is
  viral: every layout caller must have a `TargetLayout` to pass.
- The const-eval query evaluates thunks per file, so a constant referenced from
  many files is evaluated once per referencing file until cross-file reads exist.
  Interning makes this a time cost rather than a correctness one, which is the
  same trade ADR-0016 §5 already accepted for signatures.
- libffi and `libloading` put a C toolchain and a dynamic loader in the build of
  a compiler that previously needed neither, on every platform, including for
  contributors who will never run comptime code.
- Comptime execution can now diverge from runtime in one specific way: a foreign
  call succeeds at runtime and is refused at comptime. That is ADR-0006's
  intended behaviour, not a bug, but it is the one place where "the same MIR"
  does not mean "the same outcome".

### Follow-on work this forces

- **A liveness pass, if frame size ever matters.** Deliberately not written; the
  escape hatch is a per-body live-range map, and nothing measures a problem yet.
- **`#foreign_at_comptime` in wave W6** flips the mode from §4 rather than adding
  a mechanism, which is the whole reason the mode exists now.
- **Cranelift must call `jr-pool`'s layout**, not compute its own. This is the
  obligation §2 exists to create, and the verifier cannot enforce it — the first
  `jr-codegen-clif` wave has to honour it deliberately.
- **`ForeignInfo::library` resolution wants a home.** Two passes now resolve it
  independently (sema for E0225, the VM for the call). A third is the signal to
  intern the resolved library alongside `ForeignLibraryValue`.
- **Cross-file `#run`.** The const query is per file, so a `#run` whose expression
  calls into an imported module now *lowers* — §5 made the callee representable —
  but evaluating it means the const query reaching into another file's MIR, which
  is the cross-body read ADR-0017 §3 keeps out of the built-MIR query. Until that
  is staged, a cross-file `#run` is refused at evaluation rather than at lowering.

## Alternatives considered

The two bytecode alternatives (§1), the three layout homes (§2), the two
const-eval placements (§3), the two FFI strategies (§4) and the two alternatives
to widening the callee (§5) are each argued at their point of decision above, with
the project that chose them named where one did, rather than being restated here.

One cross-cutting alternative deserves recording: **a JIT for comptime instead of
an interpreter**, which is what `cranelift-jit` — already pinned in
`[workspace.dependencies]` — exists to make possible, and which Jai itself
effectively does by running comptime code as native code. It would delete the
bytecode, the interpreter and this ADR's §1 outright, and it would make comptime
and runtime share not merely the same MIR but the same *machine code*, which is a
stronger form of PLAN.md §3.1's invariant than the one we are buying.

It was not chosen because it inverts the risk profile of the slice. An
interpreter is portable, debuggable, and can run before any backend exists;
`cranelift-jit` would make `#run` depend on the entire codegen path being correct
first, so the first bug in comptime arithmetic would be indistinguishable from a
bug in code generation. The escape hatch, if comptime execution ever becomes a
compile-time bottleneck, is to add a JIT tier *behind* the same MIR — at which
point the interpreter remains as the reference implementation the tier is
differentially tested against, which is worth more than the code it costs.

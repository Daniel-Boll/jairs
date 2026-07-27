# ADR-0019: Native back end shape — a three-phase `Backend`, traps through a runtime helper, one interned foreign library

- **Status:** Accepted
- **Date:** 2026-07-26
- **Deciders:** dboll

## Context

`jr-codegen` and `jr-codegen-clif` are being built, and with them `jr-link`. Both
crates exist today as a `Cargo.toml` and a one-line `lib.rs`; nothing in the
workspace has ever emitted a machine instruction.

Everything upstream exists and is exercised: the lexer, the error-recovering
parser, the lossless CST, HIR with name resolution and spans, the module loader,
the `InternPool`, `jr-sema`, typed SSA MIR (ADR-0017), and — as of ADR-0018 — a
bytecode VM that runs `tests/corpus/valid/024-hello.jr` to completion.

`PLAN.md` §3.1's invariant is the reason this wave is constrained rather than
free:

> **The load-bearing invariant:** comptime and runtime execute *the same* MIR. The
> VM consumes bytecode lowered from the identical MIR that Cranelift consumes.
> Any other arrangement guarantees `#run` and runtime silently disagree.

The VM half of that sentence is built. This wave builds the other half, which
means every decision here is really the same question asked five ways: *what must
be shared with the VM, and what may differ?* Five things were open.

1. **What shape the `Backend` trait has.** ADR-0009 requires one — all Cranelift
   contact confined behind it — but says nothing about its granularity.
2. **How an ADR-0002 trap becomes machine code.** ADR-0002 says integer overflow
   always traps. The VM implements that with a message. Native has no message
   mechanism at all, and §1.4 asks for a trap that carries a source location.
3. **How MIR provenance becomes a source span at a trap site.** This was raised
   as an `AstIdMap` question and turned out not to be one — see §3.
4. **How `#foreign` resolves for a *linked* binary.** The VM uses a process-local
   `dlsym`, which a linked executable cannot do.
5. **Whether `jr-codegen-llvm` is in scope.** It is a declared, empty crate.

A sixth question was not a fork but a contradiction between two accepted
documents, found while writing this ADR, and is settled in §6.

Three facts about the existing code narrow the space, and all three were verified
rather than assumed.

**MIR is already in the shape Cranelift wants.** ADR-0017 §1 chose block
*parameters* over phi statements and forbade critical edges; `MirBody` exposes
`reverse_postorder()` and a cached `predecessors()`. Block parameters map onto
`FunctionBuilder::append_block_param` one-for-one, so there is no unphi pass to
write. Slots map onto Cranelift stack slots addressed by `stack_addr`. This is
what ADR-0017 was buying, and this is the wave that collects.

**Layout already exists and is not ours to recompute.** `jr-pool::layout` owns
`layout_of`, `field_offset`, `string_data`, `string_count` and `align_up`, with a
`TargetLayout` passed in (ADR-0018 §2). The VM calls it. Cranelift must call the
same functions.

**A `MirSpan` can already be resolved to a `Span`.** `jr-mir::cfg::resolve_span`
does it, for every non-synthetic variant, and has since the MIR wave. It is
private, which is a visibility problem and not a design one.

## Decision

### 1. `Backend` is a three-phase trait: declare, define, finalise

`jr-codegen` defines a `Backend` trait whose lifecycle is explicit rather than
hidden:

- **declare** — every procedure that will exist, with its signature, before any
  body is generated.
- **define** — one body at a time, from a `&MirBody` plus the pool and the
  target layout.
- **finalise** — produce the artifact (an object file's bytes), consuming the
  backend.

No `cranelift-*` type appears anywhere in the trait, as ADR-0009 requires. The
trait speaks `MirBody`, `ProcRef`, `PoolId` and `TargetLayout`, all of which are
ours.

The declare phase is not bureaucracy: it is what makes a forward call
representable at all. ADR-0018 §5 widened `Callee::Direct` to a `ProcRef`, so a
body may call a procedure defined later in the same file or in another file
entirely. A backend that learns about a callee only when it reaches the call
either needs a hidden pre-pass or has to patch afterwards. Declaring first makes
the ordering a property of the interface, where a reader can see it, and it is
also the shape `cranelift-module`'s own `Module` trait has — so
`jr-codegen-clif` is a thin adapter rather than a translation layer.

**Rejected: one `compile_body(&MirBody)` call with the backend owning all module
state.** Smaller today, and genuinely tempting while there is one back end. It
was rejected because it does not remove the two-phase requirement, it hides it:
the backend would have to walk every body to collect callees before generating
any, which is the declare phase with no name and no way for the driver to
sequence it. It also gives incremental recompilation nothing to hold onto later,
since "define one body" and "the module is complete" would be the same call.

### 2. A trap is a call to a runtime helper, not a bare machine trap

Each ADR-0002 trap site compares, branches to a per-procedure trap block, and
that block **calls a runtime helper** which reports and aborts. The helper is
handed a trap kind and an identifier for the site; it does not return.

This is the only one of the three plausible lowerings that can carry a
*message*, and that is the whole reason to prefer it. `PLAN.md` §1.4 asks for a
trap that names its source location, and the VM already produces one. Choosing
either mute option would mean the VM says "integer overflow in `add` at line 12"
where native says nothing at all — and then the differential criterion in the
same section could only ever compare programs that *succeed*, which is the half
that needs checking least. A trap is exactly where comptime and runtime are most
likely to diverge and most expensive to debug.

The helper lives in a small runtime that `jr-link` links in. That is not a new
dependency: `jr-link` has to exist this wave regardless, and a runtime object is
something it must already be able to place on the link line.

**Rejected: `trapif`-style flag checks after each arithmetic operation.** The
fastest and most idiomatic Cranelift, and it is what a production compiler
eventually wants for release builds. Rejected for now because it is mute, and
because it scatters trap knowledge across every arithmetic site instead of
concentrating it in one block per procedure. Nothing here forecloses it: it is a
per-build-mode substitution behind the same MIR, and the natural time to add it
is when there is a benchmark that shows the call is worth removing.

**Rejected: one shared trap block per procedure emitting a bare Cranelift trap.**
The tempting middle. Rejected for the same reason — mute — with the additional
observation that it is *almost* this decision: the block exists either way, and
the only difference is whether it calls out or halts. Choosing the call costs
one relocation and buys the message.

### 3. Trap spans reuse `jr-mir`'s existing resolution; `AstIdMap` stays deferred

`jr-mir` exposes its existing `MirSpan` → `Span` resolution, and the trap path
uses it. No new query, no map, and **no amendment to ADR-0013.**

This is recorded as a decision because it was raised as one, on a premise that
was false, and the false premise is the useful part of the record. `PLAN.md` §7
and this project's own handoff notes asserted that "`MirSpan` names an HIR node
and nothing resolves one back to a span; ADR-0013's deferred `AstIdMap` is the
blocker". The code says otherwise. ADR-0013's actual decision is that **HIR nodes
store `Span` directly**, so resolving MIR provenance is a field read, and
`jr-mir::cfg::resolve_span` has been performing it for the CFG diagnostics
E0227–E0229 since the previous wave.

The conflation was between two true-sounding statements: *MIR stores no byte
ranges*, which is true and which ADR-0017's follow-on work explains, and *MIR
provenance cannot be resolved*, which is false. One search of the workspace for
`MirSpan` would have separated them, and did, eventually.

ADR-0013 therefore stands unamended and keeps its own revisit trigger: wave W9,
when keystroke-to-diagnostic latency is *measured* rather than assumed. Nothing
in this wave is evidence about that.

**Rejected: build the `jr-db` CST query anyway.** It is the right eventual design
and it is what ADR-0013 anticipates. Rejected because it is real complexity
bought against an unmeasured cost — the precise thing ADR-0013 declined to do —
and because nothing in this wave needs it once the existing function is
reachable.

### 4. The resolved foreign library is interned in the pool

`#foreign` resolution moves into the pool, beside `Item::ForeignLibraryValue`, and
both existing consumers read it from there instead of resolving it themselves.

ADR-0018 §4 set this trigger explicitly: `jr-sema` resolves a `#foreign`
declaration's library for E0225 and records nothing, the VM resolves it again to
make a call, and "the day a third appears the answer belongs in the pool next to
`ForeignLibraryValue`". The native back end is that third consumer — it needs a
library name to put on a link line, which a process-local `dlsym` cannot supply —
so the trigger fires here as written.

The reason to honour it rather than resolve a third time is the same reason
ADR-0018 §2 put layout in the pool: three independent answers to one question
cannot be kept in agreement by any verifier, and their disagreement is silent.
A `#foreign` symbol that sema accepts, the VM calls, and the linker cannot locate
is a build failure at best; the reverse — three subtly different notions of which
library a name denotes — is a miscompile of the kind this project has already
produced twice by other means.

**Rejected: emit a linker directive straight from `ForeignInfo` and intern on the
fourth consumer.** Least work this wave, and defensible if the resolution were
trivial. Rejected because it is not trivial — it is where `#system_library`,
search paths, and platform naming meet — and because ADR-0018 §4 already spent
the argument. Deferring a trigger the moment it fires is how debt becomes
permanent.

### 5. `jr-codegen-llvm` stays an empty crate

Wave W8 owns it, and it stays out of scope. The `Backend` trait of §1 is what
makes adding it later cheap, and W8's stated exit criterion — three-way
differential testing, VM ≡ Cranelift ≡ LLVM — is what will actually prove the
trait is backend-agnostic rather than Cranelift-shaped.

**Rejected: sketch it alongside Cranelift to prove the trait generalises.** A
real benefit, since a trait with one implementor is a trait fitted to that
implementor. Rejected on cost: it roughly doubles the wave, and the risk it
mitigates is one that W8 addresses head-on with a differential criterion instead
of an untested second path.

### 6. ADR-0009's "inliner before any backend" is deliberately deferred

ADR-0009's follow-on work says a real inliner must live in `jr-mir` **before any
backend consumes MIR**, and `PLAN.md` §1.3 repeats it. `PLAN.md` §1.3 also states
that `jr-mir` has no mid-end and assigns the inliner to a §2.1 wave, while §7
scopes this one. Those cannot both be satisfied, and the contradiction is settled
here rather than resolved by whichever document is read second.

**The backend is built first, without an inliner.** ADR-0009's reason for the
ordering is that Cranelift cannot inline, so `#expand` macros and comptime-heavy
code must be inlined before reaching it. Jairs-0 has no `#expand` and no
comptime-heavy code. A back end without an inliner therefore emits code that is
*unoptimised*, not code that is *wrong* — every call is a real call, which is
correct, merely slower.

The deferral expires on the first of these, and this is the condition to test
against rather than a vague "later":

- the first `#expand` macro, which assumes inlining as a semantic and not as an
  optimisation, or
- the first compile-throughput or runtime number `PLAN.md` proposes to publish,
  since an uninlined number measures the missing mid-end rather than the backend.

This is an amendment to ADR-0009's follow-on work, in the sense ADR-0018 §5
amended ADR-0017: the decision ADR-0009 records — Cranelift pinned, all contact
behind `Backend`, the inliner ours and in MIR — is untouched. Only its ordering
claim is relaxed, and only with the expiry above.

## Consequences

### Positive

- Cranelift is confined to one crate behind a trait that speaks only our types,
  so an ADR-0009 API break, or W8's LLVM back end, touches one file set.
- The declare phase makes cross-file and forward calls work by construction
  rather than by a patch-up pass, which is what ADR-0018 §5's widened `Callee`
  needs from a back end.
- A native trap can say what happened and where, so `PLAN.md` §1.4's differential
  criterion can compare failing programs and not just succeeding ones — the
  half where comptime and runtime are most likely to disagree.
- One interned answer for a `#foreign` library, read by sema, the VM and the
  linker, removes a three-way divergence that no verifier could have caught.
- ADR-0013 is untouched and still has an evidence-based revisit trigger, rather
  than being pre-emptively superseded by a wave that gathered no evidence about
  it.

### Negative

- A trap costs a call rather than a single machine trap. Deliberate, and
  reversible per build mode behind the same MIR once a benchmark justifies it.
- There is a runtime object to build, link and keep working on every supported
  platform, however small. `jr-link` carries it.
- Interning the foreign library touches `jr-pool`, `jr-sema` and `jr-vm` in a
  wave whose subject is the back end. Accepted because the alternative is a third
  independent resolution.
- Native code is uninlined, so any performance number taken this wave describes
  the missing mid-end and not the back end. §6 names the condition that ends
  this; until then, no such number should be published.
- The `Backend` trait has exactly one implementor and is therefore fitted to it
  to an unknown degree. W8's three-way differential is what will expose that.

### Follow-on work this forces

- **Into this wave:** `jr-mir` must expose its span resolution; `jr-pool` must
  carry the resolved foreign library and both existing consumers must read it
  from there; `jr-link` must place a runtime object on the link line and ad-hoc
  codesign on macOS.
- **Into the mid-end wave (§2.1):** the inliner, whose absence §6 makes explicit,
  together with the first honest performance number.
- **Into wave W8:** the LLVM back end behind the same trait, and the three-way
  differential that tests whether the trait was genuinely backend-agnostic.
- **Into wave W9:** ADR-0013's own trigger — measure keystroke-to-diagnostic
  latency and decide then whether `AstIdMap` is worth building.

## Alternatives considered

Each fork's rejected alternatives are argued at its own point of decision above,
which is where a reader who disagrees with a choice will be standing. Two
alternatives span the whole ADR and belong here instead.

**Let Cranelift compute layout.** Cranelift has its own notion of types and
sizes, and using it would delete `jr-pool::layout` calls from the back end
entirely. This is the single most dangerous option available in this wave and it
is rejected outright. ADR-0018 §2 put layout in the pool precisely so that the VM
and Cranelift could not disagree, and a divergence here is *silent*: a struct
whose field sits at offset 8 at comptime and offset 12 at runtime produces two
different programs from one source, with no diagnostic, no verifier complaint,
and a failure that surfaces arbitrarily far away. No test can be relied on to
catch it in general, which is why it is stated as a prohibition rather than a
preference.

**Skip the VM/native differential and trust the shared MIR.** Sharing MIR is what
makes agreement *likely*; it is not what makes it *checked*. `jr-vm`'s
`tests/execute.rs` is 34 assertions about what each construct means, and running
the same programs through the native path is the cheapest possible oracle for the
one invariant `PLAN.md` §3.1 calls load-bearing. Rejected because the two silent
miscompiles this project has already had were both cases where a plausible
argument stood in for a check.

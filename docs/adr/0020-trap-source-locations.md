# ADR-0020: A trap names its source location, and one formatter decides how

- **Status:** Accepted
- **Date:** 2026-07-26
- **Deciders:** dboll

## Context

`PLAN.md` §1.4 asks for "integer overflow traps, in both the VM and native, **with a
source location**". After ADR-0019 both engines trap, they agree on the wording, and
they agree byte for byte on stderr — but neither says *where*. This is the last of
§1.4's twelve criteria that is not about a platform or an editor.

Four things about the existing code decide the shape, and all four were verified
rather than assumed.

**Resolution is not what is missing.** `jr_mir::resolve_span` turns a `MirSpan` into a
`Span` for every non-synthetic variant, and has since the MIR wave. ADR-0019 §3
records at length that a previous handoff blamed ADR-0013's deferred `AstIdMap` for
this and was wrong; ADR-0013 stores spans on HIR nodes directly, so resolution is a
field read.

**The VM cannot say where it is.** `Code` carries an instruction stream, a register
type table, slot plans and an entry index — and no spans at all. The interpreter
therefore has no way to name the instruction it trapped on, however well MIR
remembers.

**The back end cannot resolve a location.** `Backend::define` receives a `MirBody`, the
pool and a `TargetLayout`. Resolving a `MirSpan` needs `FileHir`; rendering it needs a
`SourceMap`. Neither is available, and ADR-0009 confines the back end so that neither
*should* be — a back end that reached into the front end to render a message would
undo the confinement the trait exists to create.

**The two engines render at different times.** Native embeds a string chosen when the
object is emitted. The VM builds one while the program is running. That asymmetry is
the whole difficulty: `crates/jr-cli/tests/differential.rs` compares stderr, so any
difference in format — a prefix, a separator, a trailing newline — is a test failure.
It already caught one, when native said `arithmetic overflowed` and the VM said
`error: addition overflowed`.

## Decision

### 1. The message is rustc's two-line shape

```
error: addition overflowed
  --> tests/corpus/valid/024-hello.jr:21:12
```

The first line is what ADR-0019 already produces. The second is new, and appears only
when a location is available.

This format is **ours**, which is the property that matters. Both engines can produce
it from one function (§2), the native side embeds about forty extra bytes per trap
site, and there is nothing in it whose rendering can drift underneath us.

**Rejected: the full `annotate-snippets` treatment**, quoting and underlining the
offending line the way `jr check`'s diagnostics do. It looks the best by a wide margin
and it is what a reader of this compiler's other output would expect. It is rejected
because it would make trap output depend on a third-party renderer's exact bytes: the
native binary must embed a *pre-rendered* excerpt, the VM must reproduce the same
bytes at runtime, and `annotate-snippets` is free to change its underlining, its
gutter or its colours in any release. `differential.rs` would then fail on a
dependency bump, and the failure would look like a miscompile. A trap message is not
the place to buy that coupling; `jr check`'s diagnostics, which are rendered once and
compared by nobody, are.

**Rejected: one line, `... overflowed at path:21:12`.** Slightly smaller and easier to
assert on, but it reads worse and it is the only diagnostic in the compiler that would
not look like the others.

### 2. One formatter, in `jr-base`

`jr_base::trap_message` is the single function that turns a reason and an optional
location into the bytes written to stderr. The native renderer and the VM renderer
both call it. Neither formats its own.

This is **ADR-0018 §2's layout discipline applied to a message**, and for the same
reason: two independent implementations of one format cannot be kept in agreement by
any verifier, and their disagreement is silent until something compares them. Layout
went into `jr-pool` because the VM and Cranelift both need a byte offset; the trap
format goes into `jr-base` because both need a sentence.

`jr-base` rather than `jr-diag` because `jr-base` has no dependencies and everything
depends on it — including `jr-codegen`, which does not depend on `jr-diag` and should
not acquire the dependency for one function. `jr-base` already owns `Span`,
`SourceMap` and `LineCol`, so a span-to-text helper is at home there.

**Rejected: each engine formats its own.** This wave has already run the experiment.

### 3. The back end is *given* locations, as a `define` parameter

`Backend::define` takes a fifth argument: a `&dyn TrapLocations`, which maps a
`MirSpan` to a rendered location string. The driver implements it, because the driver
is the one place that has the `FileHir` to resolve a span and the `SourceMap` to
render it.

A parameter rather than a setter on the back end, because a setter is hidden,
order-dependent state: forget to call it and every trap silently loses its location.
That is precisely the class of quiet degradation this project keeps being bitten by —
a well-typed placeholder standing in for a missing answer — and a required parameter
makes it a compile error instead.

**Rejected: hand the back end the HIR and the source map** so it can resolve
locations itself. It is less plumbing and it is the wrong direction: ADR-0009 confines
Cranelift to one crate by keeping the trait's vocabulary ours and narrow, and a back
end that takes `FileHir` has the front end in its signature.

### 4. Every bytecode instruction carries its span

`Code` gains a `spans: Vec<MirSpan>` parallel to `instrs`. The interpreter reads
`spans[pc]` when it traps.

A span for *every* instruction rather than only for the ones that can trap. The
narrow version is smaller and it needs a second notion of "which instruction is
this" kept in step with the first; more importantly, the set of instructions that can
trap grows every wave, and a new one would silently get no location — the failure
being an absent detail rather than a wrong answer, which is the kind this project has
learned to distrust. A `MirSpan` is a small `Copy` enum, and `Code` already carries a
`PoolId` per register, so the uniform version costs little and is trivially correct.

It also pays for itself elsewhere: a bytecode dump can now show provenance, which is
the first thing wanted when the two engines disagree.

## Consequences

### Positive

- §1.4's last non-platform criterion is met, in both engines, and the differential
  proves they agree rather than asserting it.
- The trap format exists exactly once, so the two engines cannot drift apart the way
  they already did once.
- A back end still cannot see the front end; the location arrives as text.
- The VM's bytecode can now report provenance generally, not only at a trap.

### Negative

- `Backend::define` grows a fifth parameter. Deliberate: see §3.
- The native object carries one message per trap *site* rather than one per kind, so
  a program with many arithmetic operations carries more strings. Each is short, and
  the alternative is a trap that cannot say where it happened.
- `Code` grows by one `MirSpan` per instruction.
- A trap in a body with no resolvable span — a compiler-invented value, where
  `MirSpan::Synthetic` is the honest answer — still reports without a location. That
  is correct rather than a gap: `resolve_span`'s docs argue that a diagnostic pointing
  at the wrong line is worse than one that is missing.

### Follow-on work this forces

- **Into this wave:** both engines must render through `jr_base::trap_message`, and
  `differential.rs` must assert on a *located* message so the agreement is checked
  rather than hoped for.
- **Into the mid-end wave:** an inliner must propagate spans, or an inlined body's
  traps will name the callee's source and not the call site. ADR-0017 §4's follow-on
  work already says the inliner must propagate rather than re-report; this adds spans
  to what it must carry.

## Alternatives considered

Each fork's rejected alternatives are argued at its own point of decision. One
alternative spans the whole ADR.

**Report the location only in the VM, and leave native mute.** It is much less work:
the VM has the span at run time once §4 is done, and no `TrapLocations`, no
per-site data objects and no `define` parameter are needed. It is rejected because it
would make the two engines disagree *by design*, which turns `differential.rs` from a
guard into a thing that must be taught to ignore a difference. Every exception a
differential harness carries is a place a real divergence can hide, and this project's
two silent miscompiles were both cases where something that should have been compared
was not.

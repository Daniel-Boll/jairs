# ADR-0178: A refused body gets a trapping stub — the declare phase's promise, kept

- **Status:** Accepted
- **Date:** 2026-09-02
- **Deciders:** dboll
- **A latent defect found while auditing W11**, not a planned wave. Probing whether a `#c_call`
  procedure could read a file-scope global turned up something unrelated and worse: a legal-looking
  program made the compiler **panic**.
- No design fork was put to the decider; the recommended approach was taken and the rejected
  alternatives are recorded below, per the session directive.

## Context

### What the panic looked like

```
counter: s64;
main :: () { counter = 5; exit(counter); }
```

`jr check` handles this **well**: E0245, with four notes that between them say exactly the right
things — *"the lowering step reported: a file-level item has no value until jr-vm"*, *"this program is
legal and this compiler has a gap"*, and the one that turns out to be the specification for this ADR:

> **calling it is an error; leaving it uncalled is not**

`jr build` did this:

```
thread 'main' panicked at cranelift-object-0.134.2/src/backend.rs:689:17:
function "jr$0$0" with linkage Export must be defined but is not
```

Exit 101. A mangled symbol. No connection to the diagnostic already printed, and nothing telling a
reader that a file-scope variable was the cause.

### Why it happened, and why the comment above it was half right

ADR-0019 §1's two phases are *declare everything*, then *define each body*. Phase 2 read the MIR and
skipped a refused body, under this comment:

> A body MIR refused is skipped rather than reported: the refusal is ADR-0017 §4 working, and
> something upstream already reported the cause.

Both halves are true, and the conclusion does not follow. Phase 1 had already declared the procedure
with `Linkage::Export`, so skipping the definition leaves the object promising a symbol nothing
supplies — and the object layer is entitled to panic on that, because a driver that declares and does
not define is a driver bug. The comment was reasoning about *diagnostics* in a place whose problem
was *linkage*.

**The bytecode VM got this right the whole time**, which is what makes the asymmetry a defect rather
than a shared gap: `jr run` on the same program reports *"cannot run: the compiler could not lower
`main`"* and exits cleanly. One engine refused honestly; the other panicked.

## Decision

### 1. A refused body is defined as a stub that traps

`MirBody::refused(proc, params, ret)`: the declared parameters, one block, terminated by
`Unreachable::Refused`.

This is E0245's own last note made true at run time. Calling the procedure is an error — it traps
with a message. Leaving it uncalled is not — the build succeeds and produces a binary.

**Rejected: failing the build.** It is the smaller change and it is what a first instinct reaches for,
but it breaks the second half of the promise: a program with one uncompilable procedure that nobody
calls would stop building, and E0245 exists precisely to say that such a program is fine.

**Rejected: skipping the declaration too.** Then a *call site* in another body finds no symbol and the
back end reports `CodegenError::Undeclared` — which relocates the panic into a different internal
error rather than removing it, and loses the case where the refused procedure is the entry point.

**Rejected: gating E0245 on reachability**, i.e. erroring only when something calls the refused
procedure. `AGENTS.md` records why this was already declined once: it *would have masked* ADR-0120's
four defects, which reached an engine only because a refused body still linked. This decision keeps
that property — the stub links, and the defect still reaches an engine — while removing the panic.

**Parameters are declared even though nothing reads them.** The signature phase 1 emitted has them, so
an empty entry block against a two-parameter signature is a Cranelift *verifier* error rather than the
trap the stub exists to be. `a_refused_stub_carries_the_declared_parameters_and_traps` pins it,
because the failure mode is one an eye passes over.

### 2. `Unreachable::Refused` is its own variant, and so is `TrapKind::Refused`

**Rejected: reusing `Unreachable::Trap`.** It compiles, it is one line shorter, and it prints
**"reached a deliberate trap"** for a program that deliberately did nothing of the kind. The two mean
opposite things to a reader: a deliberate trap is the *program* doing what it asked for; this is the
*compiler* admitting a gap. A message that confuses those sends a user looking for a `trap` in their
own source.

The new message is *"this procedure could not be compiled; the compiler reported a gap in it"* —
pointing back at the E0245 that already said which gap.

Adding the variant made **six** sites a compile error, which is the exhaustive-match rule doing its
job (`AGENTS.md`): `jr-mir`'s `cfg` and `dump`, the interpreter, and both back ends. Each answer is
reasoned rather than a `{}`:

- **`cfg::missing_return`** answers `false`. A stub did not fall off an end; it was never lowered. It
  also cannot reach that query at all, since the stub is built by the *driver*, downstream of it.
- **`dump`** prints `refused`, so a MIR snapshot distinguishes the two.
- **The interpreter** maps it, and says in a comment that it is unreachable there — the VM refuses to
  run a refused file rather than running a stub. Mapped anyway, because a `_` arm would silently mean
  `Deliberate` if that ever changed.
- **Both back ends** take it, for the same reason, which is why it is not a Cranelift detail.

### 3. `TrapKind::ALL`'s guard was a proxy, and it was hiding four missing kinds

Adding `TrapKind::Refused` failed `every_kind_is_listed_in_all`, whose assertion was
`ALL.len() == 11`. That test **worked by luck**: a count catches a variant added to `ALL` and nothing
else, and `[Self; 11]` compiles perfectly beside an enum with twelve variants. It fired here only
because this wave happened to bump the array's own length annotation first.

**Replaced with an exhaustive match** over every variant, so adding one is a compile error in that
file — the rule `AGENTS.md` states for match arms, applied to a registry.

**It immediately found a pre-existing hole.** `ShiftOutOfRange`, `IndexOutOfBounds`, `NullCall` and
`WrongVariantCase` were **never in `ALL`** — four of fifteen kinds. And `ALL`'s doc claimed *"so the
driver can emit one message object per kind up front"*, describing a loop that **does not exist**: a
back end interns a message lazily at each trap site, and nothing outside this file's own tests reads
`ALL` at all.

The stale sentence is what let the omission stand: the list looked load-bearing, so nobody audited it.
And the omission was not cosmetic, because `reasons_are_distinct` iterates `ALL` — a test whose whole
purpose is that no two kinds share a sentence, since the corpus differential compares *rendered
messages* and one shared wording would make a genuine engine disagreement invisible. **Four kinds were
never checked for it.** They are now, and they are distinct.

**This is the same defect shape as the E0290 collision and the `file_consts` feature list**: a hand-
maintained list, a comment asserting something enforces it, and nothing that does.

### 4. The file-scope mutable variable itself is untouched

`counter: s64;` at file scope remains unlowerable, and E0245 remains the right diagnostic for it. That
is a language feature — a data object with an address and an initialiser — and it is a wave. This ADR
fixes what a *refused* body does, which is a property of every present and future refusal rather than
of this one.

## Consequences

- **`jr build` no longer panics** on a program whose body lowering refused. Both back ends produce a
  binary that traps with the compiler's own admission and names its frame.
- **The two engines now agree in kind**: the VM refuses to run, the native path builds and traps. Both
  say something true and neither exits 101.
- **1071 tests** (1075 under gate 7), and `TrapKind`'s registry test now covers **sixteen** kinds
  rather than eleven: a `jr-cli` build test pinning build-succeeds-then-traps, and a
  `jr-mir` unit test pinning the stub's parameter shape.
- **The build test is not a corpus program**, for the reason ADR-0177 §5 gives about concurrency: the
  corpus differential asserts that all three engines agree on an exit code, and the VM deliberately
  produces none here. The asymmetry has no home in a suite whose premise is agreement.
- **Owed, unchanged**: a file-scope mutable variable, and the reachability question `AGENTS.md` records
  as deliberately open.

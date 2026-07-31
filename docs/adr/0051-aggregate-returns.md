# ADR-0051: An aggregate is returned through a caller-allocated `sret` pointer

- **Status:** Accepted
- **Date:** 2026-07-29
- **Deciders:** dboll
- **Scope:** Returning an aggregate from a **Jairs** procedure, in `jr-codegen-clif`. The adjacent
  `#foreign` aggregate-*parameter* refusal stays (§4), and no syntax changes.
- **Amends:** nothing. It *retires a refusal* whose stated reason has expired — see §1.

## Context

`jr build` refuses to compile a procedure returning a struct, a string or a view, while `jr run`
executes one correctly. That is the only place in the project where the two engines disagree about
what **compiles**, as opposed to what a compiled program computes — and the differential harness
cannot see it, because a program that does not build produces no output to compare.

The refusal in `repr.rs` gives its own reason:

> the caller-allocated `sret` convention is real work with no consumer: nothing in Jairs-0 returns
> a struct or a string, and a `#run` producing one is already refused

**That reasoning has expired, and this ADR exists because it has.** There are now two consumers and
a third arriving:

- ADR-0048's Consequences record that `Vec2 + Vec2 -> Vec2` — the *natural first example* of an
  operator overload — "gives 37 under `jr run` and fails `jr build`". Every overload in the corpus
  returns a scalar because of this, which the ADR calls "forced, not chosen".
- ADR-0044 §5 lists returning a `[]T` view as owed.
- Multiple return values, the next W2 feature, needs exactly this convention: `-> (s64, bool)` is
  an aggregate result however it is spelled.

Five facts were established by reading the code before this ADR was written, and three shaped the
decisions.

- **An aggregate `Repr` already travels as a pointer.** `Repr::clif_type` answers
  `pointer_type(target)` for `Repr::Aggregate`, and aggregate *parameters* already work that way.
  **This is the fact that decides §1**: the caller-allocated slot is the missing half of a
  convention the parameter side already implements, not a new one.
- **A returned aggregate would otherwise dangle.** The callee's value lives in its own stack slot,
  so returning that pointer returns the address of a frame about to be destroyed. That is why the
  refusal was right to exist rather than being an oversight.
- **Cranelift has `ArgumentPurpose::StructReturn`.** `Signature` supports a special parameter and
  `uses_special_param` finds it, so the convention is expressible without hand-rolling anything.
- **`jr-vm` already returns aggregates**, by value, because a VM `Value` can hold bytes. So the VM
  needs **no change at all** and the differential harness becomes the check that the two agree.
- **`jr-mir` needs no change either.** `Terminator::Return(Some(operand))` already carries an
  operand whose type is the aggregate; what changes is only how `jr-codegen-clif` *realises* it.

## Decision

### 1. The caller allocates the result slot and passes its address as a hidden leading parameter

```text
Jairs:      mk :: (a: s64) -> Vec2 { … }

Cranelift:  sig.params  = [ special(ptr, StructReturn), I64 ]
            sig.returns = [ ]

Caller:     slot = stack_slot(size_of(Vec2))
            call mk(stack_addr(slot), a)
            // the result is in `slot`

Callee:     copies its result into the pointer its first parameter holds,
            then returns no value
```

The hidden parameter is **first**, matching every C ABI that uses this convention, and it is marked
`ArgumentPurpose::StructReturn` rather than passed as an ordinary pointer so that Cranelift's own
verifier and any future ABI work can see what it is.

**Uniform for every aggregate, whatever its size.** A 16-byte `Vec2` goes through memory exactly as
a 200-byte struct does.

**Rejected: return small aggregates in registers, `sret` only for large ones.** This is what the
real arm64 and x86-64 ABIs do, and it is faster for the common case — a two-field struct becomes two
register moves instead of a store and a load. Rejected for one reason: the size threshold and the
field-classification rules are **platform-specific**, so the optimisation is where a silent
disagreement with C lives. `AGENTS.md` names "silent miscompiles from well-typed placeholders" as
this project's first failure mode, and a wrongly-classified struct is exactly that shape — garbage
in a register with no diagnostic. One self-consistent convention now; a register fast path is a
performance ADR for W8, where the differential harness across three back ends can police it.

**Rejected: return a pointer to the callee's own slot.** It dangles. Named only because it is the
shape someone reaching for the simplest change would try, and because the resulting bug — reading
a destroyed frame — reproduces intermittently rather than reliably.

### 2. Where the caller's slot comes from

The caller already has a place for most results: `x := mk(3, 4)` assigns into `x`'s slot, and
`jr-mir` gave `x` one because an aggregate local is never register-promoted (ADR-0017 §2).

The back end nonetheless allocates a **fresh** slot per call and copies out of it, rather than
passing the destination's address directly. That is one memory copy the direct version would avoid,
and it is the right default here:

- a call's result is not always assigned — `mk(3, 4).x` and a discarded call both have no
  destination slot to pass;
- passing the destination directly makes the callee write into the caller's variable *before* the
  call returns, which is observable if the callee traps halfway through. A trap must not leave a
  variable half-assigned, and ADR-0002's traps are real control flow.

Recorded as a deliberate cost, not an oversight: eliding the copy when the destination is a
whole-slot assignment and the callee cannot trap is a mid-end optimisation, and it needs the
alias reasoning ADR-0023's forwarding pass has and this back end does not.

### 3. What the VM does: nothing

`jr-vm` already returns aggregates by value, so this wave changes no VM code. **That is the check
rather than a convenience**: the differential harness compares both engines' observable behaviour,
so a corpus program that returns a struct and makes the result visible through an exit status is
what proves the new convention agrees with the one that already worked.

The obligation this creates is stated rather than implied: **the corpus program must return an
aggregate whose fields are read after the call**, because a convention that returned the right
*size* and the wrong *contents* would pass any test that only checked the call completed.

### 4. `#foreign` aggregate parameters stay refused

The adjacent refusal in the same function — an aggregate *parameter* on a `#foreign` procedure —
is deliberately untouched.

Passing a struct to a C function needs each platform's own classification rules: arm64 AAPCS and
x86-64 SysV both have per-field register/memory decisions, and they differ. Getting one wrong puts
garbage in a register with no diagnostic. A Jairs-to-Jairs call has no such constraint, because both
sides are compiled by this back end and only need to agree with *each other*.

So the two refusals are not one refusal: one is a convention we choose, the other is a convention we
must match. This ADR implements the first and leaves the second saying why it is refused.

**A returned aggregate crossing a `#foreign` boundary is refused for the same reason**, in the same
place, and the message must distinguish it from the parameter case.

### 5. What is deliberately absent

- **No register fast path** (§1), and no eliding the caller's copy (§2). Both are W8's.
- **No aggregate return through a procedure pointer.** `Callee::Indirect` is already refused for
  every call, so this adds nothing new to refuse.
- **No `#run` returning an aggregate.** ADR-0015's `Item` has no aggregate-value variant, so
  const-eval cannot represent the result. Unchanged by this wave, and unrelated to the ABI.

## Consequences

- **`repr::signature` gains a `sret` parameter and drops a refusal**, so its `describe` callback now
  has one fewer caller. The refusal for a *`#foreign`* aggregate return replaces it, which keeps the
  function total over the cases it must reject.
- **Every call site of an aggregate-returning procedure gains a stack slot and a copy.** Visible in
  the generated code and in `jr build`'s output size; not visible in any snapshot, because MIR is
  unchanged.
- **`jr-vm` is untouched, which makes the differential harness the only thing checking this.** A
  corpus program that returns a struct and reads its fields afterwards is therefore not optional —
  it is the sole verification that the two engines agree (§3).
- **ADR-0048's "every overload in the corpus returns a scalar" stops being forced.** It stays true
  of the existing corpus file, because rewriting a passing test to use a new feature would lose the
  coverage it has; the new corpus program is where an aggregate-returning overload appears.
- **No new diagnostic code.** The one new refusal reuses `CodegenError::Unsupported`, which carries
  its own message and has no `E`-code — consistent with every other back-end refusal. **E0251
  remains the first free code.**
- **`jr-mir`, `jr-sema`, `jr-hir` and both parsers are unchanged.** A wave that touches one crate is
  worth noting as evidence the layering holds: the aggregate return was always representable
  upstream, and only the machine-code realisation was missing.

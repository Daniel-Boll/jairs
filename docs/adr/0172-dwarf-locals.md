# ADR-0172: Local variables in DWARF — and the two boundaries writing it exposed

- **Status:** Accepted
- **Date:** 2026-09-02
- **Deciders:** dboll
- **W12's third item**, for the LLVM back end. It is *partly* delivered, and the partition is the decision.
- No design fork was put to the decider; the recommended approach was taken and the rejected
  alternatives are recorded below, per the session directive.

## Decision

### 1. A `DILocalVariable` per MIR slot that stands for a source local

`declare_slots` already allocates one `alloca` per slot. Each slot that stands for a source local now also gets
a DWARF variable with its name, its type DIE and a `llvm.dbg.declare` against the `alloca`.

**Precomputed on the back-end side, declared on the translator side.** Building a type DIE needs the back end's
cache and `&mut` access to it; declaring a variable needs the `alloca`, which exists only during translation. So
the two halves happen on opposite sides of that boundary and meet in `DebugScope::slots` — a slice indexed by
slot, **holes included**, because dropping them would misalign every later slot, the same trap ADR-0171 §3
records for parameters.

**Declared in the alloca block** via `insert_declare_at_end`, which LLVM places *before* the terminator. A
`llvm.dbg.declare` must dominate every use of its variable, and the alloca block dominates the whole body by
construction (ADR-0143 §4).

**Rejected: `insert_declare_before_instruction` with the block's branch.** It reads more precisely and it made
inkwell panic on `value.is_instruction()` — the insertion returned null and the wrapper asserted. Recorded
because that panic names inkwell's internals and says nothing about which call was wrong.

**A compiler temporary gets nothing.** MIR has far more slots than a program has locals, and a debugger listing
`s7` beside a user's own names is noise.

### 2. `SourceInfo::local_name` takes a `LocalId`, not a span — and the first draft was silently wrong

The first version keyed on `MirSpan::Local`. It found `total` and **missed `pair`**, because a slot's span is not
reliably a local span: an aggregate's slot carries the span of the expression that created it.

`SlotData::local` is the authoritative answer and the back end already holds it. So the lookup takes a
`LocalId`, and the implementor is per body — every body's arena starts at 0, so a `LocalId` means nothing
without knowing whose it is, which is the trap ADR-0017 records.

**A span-keyed lookup that silently names some locals and not others is worse than one that names none**, because
the gap looks like a property of the program rather than of the compiler.

### 3. Two boundaries, both found by writing the test, both now pinned

**A register-resident local is invisible.** The first test program was `total := 7; doubled := total * 2;` and
produced **no variables at all**. That is MIR's design, not a bug: only an **escaped** local gets a stack slot
(ADR-0017 §2), and a slot is what a `dbg.declare` describes. A local living entirely in SSA registers has no
address to point at.

**That is exactly what W12's "locals through value labels" item is for**, and it is now understood rather than
guessed: a register location is a *different DWARF expression*, not a missing call. So this item is half done,
and the half that remains has a name.

**An aggregate local is not named either.** The second version added one, expecting it to escape and be named.
It escapes and stays anonymous — its MIR slot carries no `LocalId`, so nothing connects it to a source name.
That is a MIR-side gap, not an emission one.

**Both are asserted, the second negatively**: the test requires that `pair` does *not* appear, with a message
telling whoever fixes MIR to invert the line. An absence that is asserted is a boundary; an absence that is
merely omitted is a thing the next reader rediscovers.

### 4. What the test checks, and why each part can fail silently

- The **name** is the one the programmer wrote. A variable called `s3` would parse fine and tell a reader
  nothing.
- The **location** is a frame-relative expression. Placed in the wrong block it is either a verifier failure or
  a variable with no location — a debugger that knows a name and cannot read the value.
- **No temporary appears.**

## Consequences

- **An escaped scalar local reaches DWARF** with its source name, its type and a stack location. `lldb` can
  print it.
- **1064 tests**, 1068 under gate 7 — three `llvm`-gated tests added across ADR-0171 and this one.
- **W12's third item is half done**, and the remaining half is *specified* rather than vague: a register-resident
  local needs a DWARF register expression, which is what Cranelift's `ValueLabel`s and LLVM's own register
  allocation would each supply differently.
- **Two gaps are pinned by assertions** rather than prose: register locals, and aggregate locals whose slot
  carries no `LocalId`.
- **Cranelift still has no `.debug_info`** — no types, no subprograms, no variables, only the line table. Every
  item in this ADR and ADR-0171 applies to LLVM alone, which is the split ADR-0170 predicted and this wave
  confirms twice over.

# ADR-0174: Cranelift's locals — and ADR-0172 §3's over-general claim, corrected

- **Status:** Accepted
- **Date:** 2026-09-02
- **Deciders:** dboll
- **Completes W12's third item for stack-resident locals in both engines**, and **amends ADR-0172 §3**, which
  stated a claim more general than its evidence. ADRs are immutable, so this is a new one that says so.
- No design fork was put to the decider; the recommended approach was taken and the rejected
  alternatives are recorded below, per the session directive.

## Decision

### 1. A `StackSlotKey` carries the MIR slot index through the compile

ADR-0173 §4 named this a thing to *probe rather than assume*, and the probe paid: `MachBufferFrameLayout` has a
`frame_to_fp_offset` and a per-`StackSlot` offset, populated unconditionally.

But a Cranelift `StackSlot` is not a MIR slot. **This back end also creates unkeyed slots for aggregate
temporaries**, so slot order is not a safe correlation — and a wrong one would name a local after somebody
else's storage, silently.

`StackSlotData::new_with_key` exists, so each MIR slot's index rides along as a `StackSlotKey` and the mapping is
a *fact* rather than an assumption about creation order.

**Keyed as `index + 1`**, so 0 can mean "not a MIR slot": `MachBufferStackSlot::key` is an `Option` and a
temporary reads as `None`, but a keyed zero would be indistinguishable from the first real slot if anything ever
defaulted it.

**Rejected: correlating by creation order.** It would work today and break the first time a temporary slot is
created before a body's own, which is a change nobody would connect to wrong debug info.

### 2. `DW_OP_fbreg` against a frame base of the frame-pointer register

Cranelift reports a slot's offset from the **bottom of the frame**; DWARF's `DW_OP_fbreg` is relative to whatever
the subprogram declares as its frame base. So the subprogram declares FP and each offset is
`slot.offset - frame_to_fp_offset` — negative, because every stack slot sits below FP.

**Rejected: `DW_OP_call_frame_cfa`**, which is the idiomatic frame base and needs `.eh_frame`. This compiler does
not emit unwind info, so there is no CFA to point at and the register is the honest base.

**The register number is per-architecture** — 29 on AArch64, 6 on x86-64 — because DWARF numbers registers per
ABI. An unknown architecture gets `u16::MAX`, which no ABI assigns, so a consumer *rejects* the expression rather
than reading a real register that means something else. Refusing to guess is the point.

**The test asserts the offset is negative**, not merely that a location exists. A location that parses and reads
the wrong memory is the failure mode here, and forgetting to subtract `frame_to_fp_offset` produces exactly that.

### 3. ADR-0172 §3 was wrong: an aggregate local's naming depends on how it is *used*

That ADR concluded, from one program, that **"an aggregate local is not named — its slot carries no `LocalId`"**.

It is not that general. In a program where the aggregate is **passed by value to a procedure**, its slot *is*
bound to its local and it *is* named — verified in both back ends, which agree. In a program where the aggregate
is only field-assigned and read, it is not.

So the rule is about **usage**, not about aggregates. ADR-0172 §3's sentence describes its test program and was
written as though it described the language.

**This is the ninth time the habit has paid, and the second time in this session against this project's own
accepted ADR** — ADR-0165 was the first, correcting ADR-0164 §5. The pattern is now specific enough to name:
**a negative result from one program is evidence about that program.** Generalising it needs a second program
that differs in the suspected dimension, and here that second program was one line away.

The test's negative assertion is narrowed accordingly, and its message now says which shape it pins.

### 4. What remains, and it is now only one thing

**A register-resident local.** ADR-0172 §3 established this is the only way such a local becomes visible, and
ADR-0173 §4 listed the three pieces: `value_labels_ranges` (a public field, present), `ValueLabel`s attached
during lowering (this back end emits none), and `enable_value_labels` in the ISA flags.

That is the whole of W12's remaining debug work besides the owed names — and it is symmetric: **neither** engine
shows a register-resident local, so the gap is a property of the project rather than of one back end.

## Consequences

- **Both back ends emit named locals with real stack locations**, by two different routes, and the Cranelift one
  asserts the arithmetic that reconciles the two frame conventions.
- **1066 tests**, 1070 under gate 7.
- **ADR-0172 §3 is superseded** in its general form; its specific observation about that program stands.
- **W12's remaining debug work is one item**: register-resident locals, specified in ADR-0173 §4, needed equally
  by both engines.
- **Still owed**: a struct's declared name, views, arrays, unions and variants, and the `dsymutil` step for a
  linked binary on macOS.

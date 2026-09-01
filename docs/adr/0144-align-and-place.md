# ADR-0144: `#align` and `#place` — layout control, in the one place layout lives

- **Status:** Accepted
- **Date:** 2026-09-01
- **Deciders:** dboll
- **W8 sub-wave 3.** §2.1 lists `#align`/`#place` in this wave's content. They are the first
  language features whose whole implementation is a *layout* change, which makes them a test of
  ADR-0018 §2's central claim: that one shared layout computation means a layout feature is
  written once and every engine gets it.
- No design fork was put to the decider; the recommended approach was taken and the rejected
  alternatives are recorded below, per the session directive.

## Context

### What a systems language needs them for

Jairs structs cross the `#foreign` boundary, and ADR-0018's layout module says so in its own
docs: fields are never reordered "because Jairs is a systems language whose structs cross the
`#foreign` boundary — a reordering compiler would make every `#foreign` struct declaration a
lie". The same argument reaches further than order. A C header that says
`__attribute__((aligned(16)))`, a hardware register block at fixed offsets, a file format with
a reserved gap, and a deliberate overlay of two fields are all things a systems language must be
able to *spell*, and today Jairs can spell none of them.

### Why this wave is cheap and why that is the interesting part

Every offset in the compiler comes from `jr_pool::field_offset`, and every size from
`jr_pool::layout_of`. The VM asks; the Cranelift back end asks; the LLVM back end (ADR-0143)
asks. Nothing else computes an offset — that is a prohibition restated in four places precisely
because violating it is silent.

So a layout feature is a change to the fold in `jr-pool/src/layout.rs` plus the syntax to reach
it. Three engines acquire it without a line each. If that turns out not to be true, the
prohibition was already broken and this wave finds out.

## Decision

### 1. A field attribute after the type: `x: s64 #align 16;`, `y: s64 #place 32;`

Both are attributes on a *field*, written after its type, in the position `#c_call` and
`#no_abc` occupy on a procedure. Each gets its own `SyntaxKind` — `ALIGN_ATTR`, `PLACE_ATTR` —
following the precedent that gave `C_CALL_ATTR` and `NO_ABC_ATTR` separate kinds rather than one
shared `ATTR` carrying its directive text: a downstream match on kinds is exhaustive where a
match on strings is not, and this project has recorded seven bugs from an unexhaustive
attribute list.

**Rejected: Jai's `#place <field-name>;` as a statement inside the struct body.** Jai's form
moves a *cursor*: `#place a;` makes subsequent fields start at `a`'s offset. It is expressive and
it is the wrong shape here for two reasons. It needs name resolution inside a struct body
*before* layout can run, which puts an ordering constraint between two phases that currently
have none. And it makes a field's offset depend on a statement written somewhere else in the
body, so reading one field's declaration no longer tells you where it is — whereas
`y: s64 #place 32;` is a fact about `y`.

**Rejected: a struct-level `#align N`.** Every use of it is expressible as an attribute on the
field that needs it, and a struct-level form would need its own rule for how it interacts with
a field's own alignment — a second, overriding notion of alignment where one will do.

**Rejected: `#align` on a local or a procedure.** A local's alignment is a stack-slot question
and a procedure's is a code-layout question; neither is this feature, and bundling them would
mean three unrelated meanings behind one directive.

### 2. The operand is an integer literal *or* a literal-valued constant

`#align 16` and `#align ALIGNMENT` where `ALIGNMENT :: 16` both work, through
`named_constant_int` — the helper ADR-0070 wrote for an array length and ADR-0129 generalised so
that "one `named_constant_int` answers for both callers". This is its **third** caller, and it
needed no change to serve one, which is the return on that generalisation.

Not a general constant expression. Layout is computed in `jr-pool`, which has no evaluator and
must not acquire one — ADR-0018 §3 puts const-eval in the VM, which is why an array length must
also be a literal or a literal-valued constant rather than `2 * N`. The two limits are the same
limit, and lifting them is one change rather than two.

### 3. `#align` is a **minimum**, and that is a decision found by building it

The value must be a power of two, at least 1 and at most 4096. The effective alignment of a field
is `max(natural, requested)`, so a value *below* the type's own alignment is not an error — it is
already satisfied.

**The first draft of this ADR said the opposite**: that `#align 1` on an `s64` would be refused,
because an underaligned field is undefined behaviour in the LLVM back end and merely slow in the
other two, so a feature whose misuse means three different things cannot be tested. The argument
still holds. What does not hold is that the compiler can *make* that check: a field's natural
alignment needs `layout_of` on the field's type, and field lists are resolved during the signature
phase, where a field whose type is a struct resolved later has no layout yet. So the rule would be
enforced sometimes, and a rule enforced sometimes is worse than a rule stated exactly.

Reading `#align` as a minimum removes the problem rather than hedging it: there is no underaligned
field to be undefined about, because the alignment only ever goes up. It is also the standard
reading — Rust's `#[repr(align(N))]` is a minimum and needs `packed` to go the other way — so the
word means what a reader coming from another systems language expects.

4096 is a page, and it is the ceiling because a stack slot must be able to honour the request: past
a page neither back end can promise the alignment it was asked for, and a request that is silently
not met is worse than a refusal. Zero, a non-power-of-two, a value past the ceiling and an operand
that needs evaluation are all **E0282**.

**Rejected: honouring a lower alignment as a packing request.** Packing is a real feature and a
*different* one: it changes the padding between fields rather than the alignment of one, and it
needs a decision about unaligned access this wave has no reason to take.

### 4. `#place N` is an exact byte offset; overlap is the point, and misalignment is allowed

The offset must be a non-negative integer literal or literal-valued constant (**E0283** otherwise).
Nothing else is required of it.

**Two fields may be placed at the same offset, deliberately.** That is what makes `#place` useful:
an overlay of two views onto the same bytes is exactly the hardware-register and file-format case,
and it is what a `union` cannot express when only *some* fields overlap. Nothing checks for
overlap, and that is stated rather than left to be discovered — the language already has an untagged
`union` whose whole contract is that reading the wrong field reinterprets bits (ADR-0045 §1), so an
overlapping `#place` is the same trade at a finer grain.

**A misaligned offset is allowed too**, and this is the second thing building it decided. The draft
required the offset to satisfy the field's alignment, for §3's original reason and with §3's
original problem. Probing found the real answer: the LLVM back end was *already* making an
alignment claim it had not established. It computes every address itself from `jr-pool`'s offsets,
and emitted `load … align 8` for an `s64` field — a promise about an address the compiler had not
proved anything about, and undefined behaviour when false. So the back end now claims `align 1`
everywhere except on an `alloca`, where it is the one making the promise rather than relying on
one. That is sound for *every* field, placed or not, and it costs nothing at
`OptimizationLevel::None`.

With that fixed, a misaligned `#place` is slow rather than wrong in all three engines, and needs no
refusal. `valid/115` places an `s64` at byte 3 and all three read it back.

**A placed field does not move the ones after it.** The layout cursor advances to the maximum end
reached by any field so far, so `#place` cannot silently shift another field. The struct's size is
the maximum of every field's `offset + size`, rounded up to the struct's alignment, and its
alignment is the maximum of its fields' effective alignments.

With no attribute anywhere, that is *exactly* today's fold — for which the MIR snapshot growing by
precisely the new corpus file and nothing else is the evidence.

**Rejected: refusing overlap.** It would make the feature a way to insert padding and nothing more,
which `#align` already does better.

**Rejected: Jai-style cursor semantics where later fields follow a placed one.** Same objection as
§1: it makes one field's declaration change another's offset.

### 5. The whole implementation is `jr-pool`'s fold, plus the syntax to reach it

`Field` gains `align: Option<u32>` and `place: Option<u64>`; the fold and the field-offset walk take
field *placements* rather than bare types. A results aggregate and the context pass none, because
neither has a source declaration to carry one.

**No engine changed for the feature.** Not the VM, not Cranelift, not LLVM — and the one LLVM change
in this wave (§4's alignment claim) is a soundness fix that was owed before `#place` existed. That
is the claim ADR-0018 §2 makes, and this is the first wave in which a layout *feature* tests it
rather than a layout *fix*.

The three-way differential (ADR-0143 §8) is the check that matters: a struct with an aligned field
and three overlapping placed fields lays out identically in three independently written engines only
if all three read the same numbers from the same place.

## Consequences

- **Three new diagnostic codes**: E0282 and E0283 in `jr-sema`, one per attribute — one code
  covering both was rejected, because a reader filtering by code wants to know which attribute they
  got wrong and the two have different rules — plus **E0132** in `jr-syntax` for an attribute with
  no value at all. **E0284 is the first free code, and E0133 the first free parser code**; the
  enforced registry test caught the stale claim within the same wave, which is what it is for.
- **`jr-fmt` needs both**, and the formatter has lost a construct in a majority of the waves that
  added one. A test asserts survival *and* canonicalisation, which is the checklist item that
  exists because round-tripping alone passes for a formatter that echoes raw text.
- **The tree-sitter grammar needs both**, and ADR-0057's lesson applies: a directive is a literal
  token in a rule, so a missing one is an ERROR node gate 6 catches.
- **`valid/115` exercises them in all three engines** and exits **114**, a checksum of offsets and
  sizes, so a wrong offset changes the number rather than only the shape. Every failure mode here
  is silent and gives both engines the same wrong answer — a `#place` ignored, an `#align` applied
  to the wrong field, a struct sized from the running sum rather than the maximum end — which is
  precisely what an agreement-only test cannot see.
- **1019 → 1027 tests** (1028 under gate 7), 228 → 231 corpus files: `valid/115` plus the two
  refusals in `type-errors/`. Six of the eight new tests are `jr-pool`'s, on the fold itself,
  because that is where the feature lives.
- **The LLVM back end now claims `align 1`** on every load, store and copy (§4). Owed before this
  wave and found by it.
- **Deliberately not done, and named**: `#pack` or any packing/lowering form (§3), a struct-level
  `#align` (§1), `#align` on a local or a procedure (§1), overlap *checking* (§4), and a
  general constant expression as the operand (§2 — one limit shared with array lengths).

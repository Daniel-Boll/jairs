# ADR-0173: Cranelift's `.debug_info` — types and subprograms, by hand

- **Status:** Accepted
- **Date:** 2026-09-02
- **Deciders:** dboll
- **The prerequisite W12's third item needs**, and it did not exist: Cranelift had a line table and no
  `.debug_info` at all. Locals through value labels remain, and §4 says exactly what they need.
- No design fork was put to the decider; the recommended approach was taken and the rejected
  alternatives are recorded below, per the session directive.

## Context

ADR-0171 and ADR-0172 gave the **LLVM** back end type DIEs, subprograms and stack-resident locals. Cranelift had
none of it — a line program pointing into a `.debug_info` that was not there.

So "locals through Cranelift value labels" was blocked on something PLAN never listed: **a DIE tree**. A
variable DIE needs a subprogram to live in and a type to point at.

## Decision

### 1. A `TypeDescription`, because names and DIEs are available at different moments

This is the wave's structural decision and it is forced.

A struct's members need **field names**, which need the driver's `SourceInfo` — available only while a body is
being *defined*. The DIEs can only be written once the object exists — at `finalise`. **The two moments do not
overlap.**

So a `TypeDescription` — a plain enum of base, pointer and struct shapes with resolved names and computed
offsets — is built during `define`, and the gimli DIEs are written from it during `finalise`.

**Rejected: threading a `SourceInfo` into `finalise`.** The driver's implementor is *per body* (it holds the HIR
body a `LocalId` indexes into, per ADR-0017's arena trap), so this would mean a second, module-scoped name
resolver beside the per-body one — a new channel for a question the existing one already answers, at the wrong
granularity.

Offsets come from `jr_pool::field_offset`, the same function both engines use to compile a field access —
ADR-0171 §1's argument, and the reason the two back ends' DIEs agree rather than merely coexist.

Deduplicated by `PoolId`, inheriting the pool's structural dedup. A cycle terminates because **a pointer's
pointee is described first and referenced only if that succeeded** — the same terminator as LLVM's.

### 2. Two passes over the DIE tree, because a reference needs an id

`DW_AT_type` on a member or a subprogram is a `UnitRef` — a reference to another DIE by id. So every type DIE is
created first and filled second. gimli's ids are stable once handed out, so the first pass's `Vec` is the whole
mapping.

**A subprogram per defined function**, with `DW_AT_low_pc` as a **relocation** against the function's symbol and
`DW_AT_high_pc` as a **length** — DWARF 4's form, which avoids a second relocation. Getting these wrong makes
every frame in a backtrace resolve to the first function in the object.

**The subprogram symbol is appended to the *same* side table the line program's sequences use.** gimli addresses
a symbol by index into one list per writer, so a second list would silently resolve to the wrong function.

**`PendingSubprogram` is separate from `PendingLines`** because a function with no source positions still
deserves a subprogram: it has a name, an address and a length, and a backtrace wants those even when no row
points inside it.

### 3. `DW_AT_name` needs the source name, which needed its own map

`ClifBackend::names` already existed and holds the read-only **data objects** a backtrace frame points at — not
the text. A DIE needs text, so `source_names` is a second map filled from `ProcDecl::name`, which the driver
already resolves.

A backtrace that says `jr$0$3` is a backtrace nobody can read; the mangled symbol is right for the linker and
wrong here.

### 4. What locals still need, now that it is knowable

This ADR is the prerequisite, not the item. A variable DIE needs a `DW_AT_location`, and there are two kinds:

- **A stack-resident local** needs `DW_OP_fbreg <offset>`, which needs the *frame offset* of its Cranelift stack
  slot. `MachBufferFinalized::frame_layout` exists; whether it exposes per-slot offsets is the next thing to
  probe rather than assume.
- **A register-resident local** needs `value_labels_ranges` — a public field on `CompiledCode` — plus
  `ValueLabel`s attached during lowering, which this back end does not emit, plus `enable_value_labels` in the
  ISA flags. Three pieces, and ADR-0172 §3 established that this is the *only* way such a local becomes visible
  in either engine.

**Named rather than estimated**, which is §5's rule about this project's least reliable table.

### 5. The test asserts agreement, not existence

The struct's members with their real offsets — ADR-0171 §6's reason: a tag-only check passes on a struct whose
every member sits at 0. And a subprogram per procedure carrying its **source** name.

**Kept separate from the LLVM struct test**, for ADR-0170 §7's reason: the two routes have nothing in common —
one is metadata LLVM writes, the other is bytes this crate writes — so what is worth asserting is that **two
unrelated emitters agree** about the same struct, and a shared test would assert only their intersection.

The *parsing* is genuinely shared and is factored into one helper, because that part is not what the tests are
about.

## Consequences

- **Cranelift emits a `.debug_info`**: base, pointer and struct types with named members at real offsets, and a
  subprogram per function with its source name, address and length.
- **Both back ends now agree** about a struct's layout in DWARF, by two entirely different routes.
- **1065 tests**, 1069 under gate 7.
- **W12's third item is unblocked and specified** — §4 lists the three pieces a register-resident local needs and
  the one open question for a stack-resident one.
- **A struct's declared name is still owed** (ADR-0171 §4), as are views, arrays, unions and variants, and the
  `dsymutil` step for a linked binary on macOS.

# ADR-0143: The LLVM back end, and a third engine to disagree with

- **Status:** Accepted
- **Date:** 2026-09-01
- **Deciders:** dboll
- **W8 sub-wave 2.** ADR-0009 put every `cranelift-*` reference behind
  [`jr_codegen::Backend`] and said why: "what makes wave W8's LLVM back end an addition rather than a
  rewrite". ADR-0019 §5 assigned `jr-codegen-llvm` to this wave. §2.1 names the deliverable as
  "LLVM backend via `inkwell`" plus "three-way differential testing: VM ≡ Cranelift ≡ LLVM".
- No design fork was put to the decider; the recommended approach was taken and the rejected
  alternatives are recorded below, per the session directive.

## Context

### The trait was designed for this and had never been used twice

`jr-codegen` names no Cranelift type. `Backend` is *declare → define → finalise*, `FileInput` is HIR
plus signatures, `TrapLocations` hands text across the boundary, and every byte count comes from
`jr_pool::layout_of`. All of that was built on the argument that a second back end would be an
addition. Until now nothing tested the claim, and an interface with one implementation is a guess.

Two things were found by using it, and both are recorded in §6 rather than hidden: the trait could
not tell the driver which libraries to link (an inherent method on `ClifBackend` did), and the trap
vocabulary — the words a trapping program prints — lived *inside* the Cranelift crate.

### Why a third engine is worth its weight

The corpus differential compares the VM against Cranelift and asserts exit codes rather than mere
agreement, because "two engines agreeing is not two engines being right". A third independent
lowering is the strongest available check on the parts of the compiler that are *shared*: MIR, the
pool's layout, and `jr_base::trap_message`. A layout bug that both existing engines read out of
`jr-pool` is invisible to a two-way comparison and stays invisible to a three-way one — but a bug in
either engine's own interpretation of MIR now has two witnesses instead of one.

### What LLVM does not have that Cranelift does

Three, and each forces a decision rather than a translation:

- **No block parameters.** MIR's phi-free design (ADR-0017 §1) maps one-for-one onto Cranelift's
  `append_block_param`. LLVM has `phi`, which is the same information written from the *other* end:
  the block lists its predecessors instead of the edge carrying arguments.
- **Opaque pointers.** Since LLVM 15 a pointer has no pointee type, and a `getelementptr` names the
  type it walks explicitly. There is no "load a struct field" that does not first say what the struct
  *is* in LLVM's type system.
- **Poison instead of traps.** `add` wraps, `shl` past the width is poison, `sdiv` by zero and
  `INT_MIN / -1` are undefined, and `fptosi` out of range is poison. Every one of those is a place
  ADR-0002 requires a trap or ADR-0040 §4 requires saturation.

## Decision

### 1. `inkwell` behind a default-off `llvm` cargo feature, and a seventh gate

`jr-codegen-llvm` depends on `inkwell` only when its own `llvm` feature is on, and `jr-cli`'s `llvm`
feature enables it. Default off.

**Why not unconditional.** `llvm-sys`'s build script needs an LLVM installation it can find — a
`llvm-config` on `PATH` or `LLVM_SYS_211_PREFIX` — and homebrew's `llvm@21` is keg-only, so neither
is true by default on the machine this project is developed on. An unconditional dependency makes
`cargo build` fail for anyone without LLVM 21, which is a wall in front of the whole compiler for a
back end that is not the default.

**So the wave adds gate 7 rather than pretending the code is covered**:

```sh
LLVM_SYS_211_PREFIX=$(brew --prefix llvm@21) \
  cargo clippy --workspace --all-targets --features jr-cli/llvm -- -D warnings
LLVM_SYS_211_PREFIX=$(brew --prefix llvm@21) \
  cargo test --workspace --features jr-cli/llvm
```

There is a precedent for a gate that needs an external tool: gate 6 shells out to `npx
tree-sitter-cli`, which is not a workspace dependency. And there is a precedent for the *failure* of
an ungated check — the Neovim script exists because editor integration rotted while nobody ran it —
which is why this is a numbered gate in `AGENTS.md` rather than a suggestion.

**Rejected: vendoring or building LLVM.** Hours of build time per toolchain bump for a project whose
own toolchain is pinned to make bumps deliberate.

**Rejected: making LLVM the default back end.** Cranelift is the verified one; ADR-0009 §"Cranelift
first" is unchanged by there being a second.

### 2. `--backend cranelift|llvm` on `jr build`, and the flag exists even without the feature

A build with no LLVM support still *accepts* `--backend llvm` at the clap layer and refuses it with a
message naming the feature. A flag that appears and disappears with a compile-time feature makes
"unknown argument" the diagnostic for a missing capability, which tells the reader the wrong thing.

`jr run` gets no such flag: it executes MIR in the VM and reaches no back end at all.

**Rejected: `-O2` selecting LLVM.** It bundles two independent choices — how much to optimise and
which code generator to use — which is the coupling ADR-0142 §1 refused `--release` for.

### 3. A block parameter becomes a `phi`, filled from the predecessor side

Every MIR block parameter gets an empty `phi` at the top of its LLVM block, created before any
terminator is translated. When a terminator is translated, each of its edges' arguments are recorded
against `(target block, predecessor block)`; after every block is done, the incomings are added.

This is the one place the two back ends genuinely differ in *shape*, and MIR's own design is what
makes it a bookkeeping change rather than a pass: ADR-0017 §1 forbids critical edges, so an edge has
exactly one predecessor block to name, and a `phi`'s incoming list is precisely the arguments those
edges carry.

**Rejected: an unphi pass that inserts copies in each predecessor.** It is the pass ADR-0017 §1
chose block parameters to avoid, it needs a fresh value per parameter per predecessor, and it would
make the LLVM back end's input differ from the Cranelift back end's — so a miscompile in the pass
would look like a back-end disagreement.

**Rejected: `alloca` per block parameter and let LLVM's `mem2reg` recover SSA.** It works, and it
throws away the SSA that ADR-0017 built during lowering — the thing this project has instead of a
`mem2reg` (ADR-0017 §2). Handing LLVM a body it must rebuild would also make the two back ends
consume MIR at different fidelities, which is exactly what a differential must not have.

### 4. Every address is an opaque `ptr`; every offset is a byte `getelementptr`

No Jairs aggregate acquires an LLVM `StructType`. A place's address is computed by walking the same
projections the Cranelift back end walks, asking `jr_pool::field_offset`, `string_data`,
`pair_data`, `triple_capacity` and `layout_of` for each step, and applying the result as a GEP over
`i8`. A load then names the *scalar* LLVM type of the leaf, which is the only type LLVM needs.

**This is the layout prohibition surviving contact with a typed IR.** Building an LLVM struct type
per Jairs type would put LLVM's own layout algorithm — its padding and alignment rules — in charge
of where a field sits, which is a *second* computation of the thing ADR-0018 §2 says must exist
once. The failure would be silent and exactly the one that ADR says no test catches in general: a
field at one offset in a `#run` and another at runtime.

So `jr-codegen-llvm` uses LLVM as an instruction selector and a register allocator, not as a type
system. Its aggregates are bytes at offsets this compiler chose, which is what they already are in
both other engines.

**Rejected: `StructType`s with explicit padding fields.** It reproduces `jr-pool`'s answers in
LLVM's vocabulary and then trusts LLVM to agree — a restatement, not a single source.

**Rejected: `%packed` structs.** Same objection, plus it would make every field access unaligned as
far as LLVM's optimiser can tell.

### 5. Traps use the overflow intrinsics, and float→int uses the saturating ones

`llvm.sadd.with.overflow` and its five siblings give the `(value, overflowed)` pair Cranelift's
`sadd_overflow` gives, so ADR-0002's "trap, never wrap" is one branch on the second element. A shift
count is compared against the width before the `shl`/`lshr`/`ashr`, because past the width LLVM
produces **poison** rather than Cranelift's masked count — and poison propagating into a value the
program prints is the silent-wrong-answer failure mode this project keeps naming. Division checks
zero and `MIN / -1` first, for the same reason: both are UB in LLVM, not merely wrong.

`llvm.fptosi.sat` and `llvm.fptoui.sat` are what make ADR-0040 §4's saturation *the same* in all
three engines. Plain `fptosi` is poison out of range, where `jr_pool::float_to_int` clamps.

**Rejected: `-fwrapv`-style reliance on `nsw`-free arithmetic and a manual overflow test.** It is
what the intrinsics do, minus LLVM's knowledge that the pair comes from one operation.

### 6. Two things the trait was missing, fixed rather than worked around

- **`Backend::libraries`.** The link line's contents were an inherent method on `ClifBackend`, so
  `build_object` could only ever drive that back end. It is a question every back end must answer —
  a `#foreign` declaration names a library whatever generates the code — so it belongs on the trait.
- **`TrapKind` and `TRAP_HELPER` move from `jr-codegen-clif` to `jr-codegen`.** They are the *words*
  a trapping program prints, paired with `jr_base::trap_message`, and the differential harness
  compares the finished bytes. A second copy in the LLVM crate would be a second chance to drift,
  and the drift would surface as a three-way disagreement whose cause is a duplicated string table.
  This is the "teach the shared layer, not each consumer" rule the project applies to layout,
  operators and coercions.

### 7. The trap helper and the entry shim are generated per back end

Both are emitted into the object rather than linked from a runtime (ADR-0019 §2's amendment), and
each back end writes its own. They are IR construction, and the only way to share them would be an
IR-agnostic builder abstraction — a bigger and less honest interface than two forty-line functions
whose behaviour a test compares.

The **shadow call stack** (ADR-0066 §1) is reproduced too, with the same two globals, the same
stride, and the same `SHADOW_CAPACITY`. Without it a trapping program's stderr would differ between
Cranelift and LLVM in its frame list, and the three-way differential would fail on every trap — for
a reason that is about backtraces rather than about code generation.

### 8. The differential becomes three-way

Under `--features llvm`, the corpus sweep compiles every executable corpus program with each back
end and compares all three behaviours. Without the feature it is the two-way sweep unchanged, so a
default `cargo test` does not silently skip a test it appears to run — the LLVM axis is `#[cfg]`-ed
out of existence rather than conditionally passing.

## Consequences

- **`jr-codegen-llvm` becomes real**: `repr.rs` (how a Jairs type becomes an LLVM value),
  `body.rs` (MIR → LLVM IR) and `lib.rs` (the module, the globals, the shim, `Backend`). The crate
  compiles to nothing without the `llvm` feature, which is what keeps the default build LLVM-free.
- **A seventh gate**, documented in `AGENTS.md` with the environment variable it needs. The six stay
  as they are, so a contributor without LLVM can still make every one of them green.
- **`Backend` gains a method and `jr-codegen` gains a module.** `jr-codegen-clif` imports `TrapKind`
  from `jr-codegen` instead of defining it; nothing outside that crate referenced it, so the move
  touches two files and a re-export.
- **`build_object` takes a back-end choice** rather than naming `ClifBackend`. Not a `BuildConfig`
  field: the choice changes no query result — `optimized_file_mir` is upstream of code generation —
  and making it an input would invalidate every MIR memo when it changed (the reasoning ADR-0058 §2
  gives for what *should* be an input, applied in the other direction).
- **Deliberately not done, and named**: LLVM optimisation passes (the `-O` level still selects only
  the *mid-end*, so `-O0` and `-O1` differ in MIR and not in LLVM), debug info, `--release`, and
  cross-compilation. The back end targets the host, exactly as the Cranelift one does.

### What building it actually found

**Every one of the 114 executable corpus programs agreed with the VM on the first run**, and so
did every trap tried by hand — the reason, the location and the two-frame backtrace, byte for
byte. That is the return on ADR-0018 §2 and ADR-0017 being decisions rather than habits: a
third engine that computes no layout of its own and consumes SSA it did not build has almost
nothing left to disagree about. It is also the strongest evidence available that the trait's
"an addition rather than a rewrite" claim was true, since the claim had never been tested.

Worth stating plainly, because it cuts the other way too: a third engine that agrees
immediately has *found* nothing. Its value is prospective — it is a second witness for every
future change to MIR, to layout, or to either back end's reading of them — and this ADR should
not be read as claiming a defect it did not find.

**1018 → 1019 tests by default and 1020 under gate 7.** The default build gains the refusal
test; gate 7 replaces it with the three-way corpus sweep and the trap comparison.

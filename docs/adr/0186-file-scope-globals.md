# ADR-0186: file-scope mutable variables

- **Status:** Accepted
- **Date:** 2026-09-02
- **Deciders:** dboll
- The wave PLAN §7 has owed since ADR-0178, built because Jai's real `Simp` API needs it. Its
  motivation is recorded here rather than in the graphics ADR, because the construct is a *language*
  feature and outlives the module that asked for it.

## Context

### What asked for it, and why the obvious answer was wrong

`modules/Simp` must expose Jai's real API. Vendored copies of Jai's own `Simp` module — found
verbatim in `valignatev/hitboxer` and `focus-editor/focus`, and diffed against each other — settle
what that is:

```jai
set_render_target :: (window: Window_Type, coords := Simp_Coordinate_System.RIGHT_HANDED)
clear_render_target :: (r: float, g: float, b: float, a: float)
immediate_quad :: (x0: float, y0: float, x1: float, y1: float, color: Vector4)
immediate_flush :: ()
```

**No state argument anywhere.** Jairs' version threaded a `*Renderer` through every call, because a
file-scope `var` was E0245 — *"a file-level item has no value until jr-vm"*.

The first answer looked like `#add_context`: Jai's `Simp` declares `#add_context simp:
*Immediate_State;` (immediate.jai:24) and allocates it on first use, so the state is *thread-local in
the context*. Jairs has a context (ADR-0057), so that seemed to be the mechanism.

**It is not sufficient, and the same research says why.** Jai's GL backend keeps the GL context in
"one process-wide global" (`Simp/backend/gl.jai:85-92`) — a plain module-level variable, not a context
field. So Jai uses **both**, and a faithful port needs the plain global regardless. A global is also
the more general of the two: `#add_context` adds thread-locality, which a 2D renderer does not need
and which would have made the context's layout program-dependent, since `CONTEXT_FIELD_NAMES` is a
Rust `const` today.

So: globals now, `#add_context` recorded as owed with its reason.

## Decision

### §1 — `PlaceBase::Global(GlobalRef)`, a third memory root

```rust
pub struct GlobalRef { pub file: FileId, pub item: ItemId }
```

Deliberately the same shape as `ProcRef`, for the same reason: a bare `ItemId` indexes one file's
items, so it cannot name a variable an imported module declares.

**Not a slot.** A `SlotId` belongs to a `MirBody` and dies with the call; a global outlives every body
and is shared by all of them, so making it a slot would need an invented body to own it.

**Not a dereference of an interned pointer.** A global's address is the linker's to choose and MIR is
built long before it exists. The base is symbolic and each engine resolves it — `symbol_value` for
Cranelift, a `GlobalValue` for LLVM, a region offset for the VM.

**A new variant, so every exhaustive match becomes a compile error.** That is the house rule and it
did the work here: nine sites in `jr-mir` failed to compile, and each had to *decide* what a global
means to it rather than inherit a default.

### §2 — The type comes from sema, the initial value from const-eval

`SigKind::Var` already records a file-scope variable's type: the signature phase resolved the
annotation or inferred it from the initialiser. `jr-mir` reads that rather than asking the HIR a
second time, which ADR-0009's confinement rule exists to prevent.

The initialiser becomes `Wanted::GlobalInit(ItemId, ExprId)`, a const-eval target. **Its own variant,
not a `Wanted::Item`**, and this is the important part: `Item` is keyed as a *constant*, and
`consts.item(id)` answering `Some` for a global would make every reader that asks "is this a
compile-time constant?" say yes about storage a procedure can write. At least three readers ask —
the MIR place path spills an aggregate constant into a slot, `scan_name` treats a value's existence
as permission to lower, and const-prop folds. Each would then read the *initial* value where the
current one was meant: a wrong answer that type-checks. So `global_inits` is a separate map from
`items`.

**A non-constant initialiser is refused**, and that is Jai's rule too rather than a limitation of this
compiler's shape: there is no moment before `main` at which arbitrary code could run to produce the
value.

`init: None` — no initialiser, `= ---`, or an initialiser const-eval refused — means **zeroed**, not
undefined. A `.bss` global is zeroed by the loader, and inventing undefined bytes for the third case
would make the native path differ from the VM about a program that is already refused.

### §3 — What each pass decided, and the one that mattered

| Pass | Decision | Why |
|---|---|---|
| `constprop` | nothing to substitute | `fold` answers `None` for *every* `Rvalue::Load` already |
| `dce` | keeps nothing alive, drops nothing | a global's storage is the program's, not the body's |
| `inline` | copied **unchanged** | a `GlobalRef` is absolute; every other base is callee-relative |
| `ssa`, slot remap | untouched | a global is not indexed by any slot table |
| `verify` | no range check | the `(FileId, ItemId)` pair is validated where the place is *built* |
| `forward` | **skipped** | store-to-load forwarding across a global is unsound |

**`forward` is the one that mattered, and the exhaustive-match rule did not protect it.**
`participating_slot` is a `let ... else`, not a `match`, so adding the variant compiled there with no
error and the pass skipped globals *by luck rather than by judgement*. Forwarding a store to a global
across a call is exactly the bug: any callee can read one, so the store being forwarded past is the
store the callee was meant to see — the reasoning ADR-0176 §2 gives for an atomic, without the marker.

**So the guarantee is narrower than this project has been stating it.** "Adding a variant is a
compile error at every site that must change" holds only where a `match` is written. A `let-else` on
an enum is a silent `_` arm. This is the fourth instance of the family AGENTS.md tracks — after the
E0290 collision, `file_consts`' feature list, and `TrapKind::ALL`'s length assertion — and it is the
first where the *mechanism this project trusts most* was the thing with the hole.

### §4 — One `declare_global` on the `Backend` trait, called before any body

`build_object` declares every global in phase 1, in its own loop over the files, before the procedure
loop. A body can read a global declared later in item order, so a forward reference must resolve to
real storage; with one interleaved loop it would be a body in an *earlier file* than the global's own
declaration.

**The trait method's default body is an error, not a no-op.** A back end that accepts a global
declaration and then emits a body reading storage nobody allocated is a wrong answer instead of a
message.

**One byte renderer for both engines.** `jr_pool::static_image` already exists — ADR-0152 §2 built it
for compiler-emitted tables — and a one-element table is exactly a global's initial bytes. Both back
ends call it. Two engines rendering one global's bytes by two routes is the divergence ADR-0020 §2
argues about for trap messages, and it would surface late.

## Consequences

- A module can hold state without threading a handle, so `modules/Simp` can have Jai's real signatures
  and `modules/Input` can drop its caller-owned `Events` buffer if it wants to.
- **Only same-file globals.** `modules/Simp`'s state is read by Simp's own procedures, which is the
  whole use case, so the cross-file path is not built. It is the `ImportedProcs` pattern again when
  something needs it — "resolve across files in `jr-db`, hand `jr-mir` the answer".
- **`#add_context` is owed**, with its reason: thread-local module state, and a context layout that
  becomes program-dependent rather than a Rust `const`.
- PLAN §7's owed-items list loses its second entry. ADR-0178's trapping stub stays: a *refused* body
  still needs one, and this ADR only removes one reason a body is refused.

## Verification

Deferred to the wave's own gates, and stated so a reader can check the claim scope:

- Both engines run a program that reads a global's initial value, writes it, reads it back, and sees
  the written value — **and a second procedure observes the first's write**, which is the property a
  global exists for and the one a per-frame implementation would get wrong.
- The three engines agree on the same program, through the differential harness.
- A non-constant initialiser is refused rather than miscompiled.

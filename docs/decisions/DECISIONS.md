# Wave decision log

A running record of the design forks put up each wave, the options considered, and which was
taken. Kept because the user asked for the considered alternatives to be persisted rather than
only the chosen one (the ADRs record the chosen decision and its rejected alternatives at the
point of decision; this is the lighter-weight per-fork log). As of the request during ADR-0060,
the recommended option is taken automatically without a blocking question.

Format: one section per wave, one subsection per fork.

---

## Wave: null and a memory source (ADR-0060), 2026-07-31

### Fork 1 — which wave

- Options: `null` + a memory source **(taken, recommended)**; the allocator protocol itself;
  traps with backtraces; pointer arithmetic.
- Why: `null` is the last reserved keyword (refusal still says "arrives in W1"), and there is no
  memory source anywhere. Both small and self-contained; the allocator protocol needs these two
  first, so doing it now would be two waves stacked. Verified by running: a proc-pointer struct
  field — which §7 claimed absent — already works.

### Fork 2 — null's type

- Options: context-typed like an integer literal **(taken, recommended)**; a distinct `*void`
  that coerces.
- Why: reuses the literal-typing path, adds no coercion. `*void` would be the language's first
  implicit coercion, which ADR-0016 exists to forbid. Cost: a bare `null` needs a context (E0257).

### Fork 3 — memory source

- Options: `malloc`/`free` via `#foreign` in Basic **(taken, recommended)**; `mmap`; none.
- Why: two `#foreign` decls beside `write`/`exit`, no new machinery. `mmap` needs platform flag
  constants Jairs cannot name yet.

### Fork 4 — comptime FFI

- Options: stay refused per ADR-0006 **(taken, recommended)**; allow malloc at comptime.
- Why: the VM's memory is its own address space; a host pointer read through it is a plausible
  wrong value. ADR-0006 already gates comptime FFI.

### Fork 5 — VM malloc (raised mid-wave, after running disproved ADR-0060 §4)

- Options: intercept malloc/free as VM builtins returning a VM offset **(taken, recommended)**;
  scope the corpus down (no VM deref of malloc'd memory); map host pointers in the VM.
- Why: a Jairs pointer is an offset into the VM's linear region; a raw host `malloc` address
  fails the VM's bounds check (native works). Intercepting keeps the sandbox and makes the byte
  round-trip work in both engines; bits differ per engine, which nothing observes. **A new ADR
  corrects ADR-0060 §4's false claim that the VM dereferences a host pointer via libffi.**

### Resolution

Fork 5 was resolved as recommended: intercept malloc/free as VM builtins. ADR-0061 records it and
corrects ADR-0060 §4. Both engines run `049-null-and-malloc.jr` to exit 0. From this point the user
asked that the recommended option be taken automatically without a blocking question, with the
considered options persisted here — which this file now does.

---

## Wave: the allocator protocol (ADR-0062), 2026-07-31

Recommended options taken automatically per the user's standing instruction.

### Fork 1 — which wave

- Options: **the allocator protocol in `context` (taken, recommended)**; traps with backtraces;
  pointer arithmetic; temporary storage.
- Why: §7 names it next, and running proved it *is* buildable — a Jairs-wrapper allocator struct of
  proc pointers works end to end in both engines. Three gaps surfaced by writing one, all small and
  all required, so they belong in this wave rather than a separate one.
- Verified by running, not asserting: `Allocator :: struct { alloc: (s64) -> *u8 }` with a Jairs
  wrapper filling the field, called through the field, allocating and writing — exit 0 in both
  engines.

### Fork 2 — a void-returning procedure pointer

- Options: **`(T)` with the arrow omitted, matching a declaration (taken, recommended)**; `-> void`
  as a spellable type name; a bare `-> ` with nothing after it.
- Why: this is the blocker. `free: (*u8)` is what an allocator needs, and today it is *unspellable*:
  `(s64)` demands `->`, `-> void` is E0212 (`void` has no name — ADR-0015 §3), and `-> ` is a parse
  error. A declared procedure already means `void` by omitting the arrow, so the type syntax should
  match the declaration syntax. Making `void` a type name would reverse ADR-0015 §3; a bare arrow is
  punctuation with nothing to read.
- Consequence: `(s64)` in *type* position becomes a proc-pointer type with a `void` return, which
  needs the results-list disambiguation extended — `(s64)` alone in return position is currently a
  one-element results list, normalised to `s64`.

### Fork 3 — a `#foreign` procedure filling a proc-pointer field

- Options: **keep it refused but fix the diagnostic (taken, recommended)**; allow it by making
  `ContextKind` part of the proc-pointer syntax; allow it by coercing.
- Why: the refusal is right (ADR-0059 §5 — a second calling convention), but the message is
  *unactionable*: "expected `(s64) -> *u8`, found `(s64) -> *u8`", identical text, because the types
  differ only in the invisible `ContextKind`. E0256 already exists for taking a `#foreign` procedure
  as a value; this is the same objection reached through assignment, so the fix is to report it the
  same way rather than let a type mismatch print two identical types.
- A `#c_call` proc-pointer type would be the general answer and is deferred: it needs a syntax for
  the attribute in a type, which is its own decision.

### Fork 4 — where the allocator lives

- Options: **`context.allocator` becomes a struct of proc pointers (taken, recommended)**; a separate
  global; leave the `s64` placeholder and add a free-standing `Allocator` type.
- Why: ADR-0057 §1 put the `s64` there explicitly as a placeholder for this, and ADR-0001's whole
  argument for a context is that an allocator travels with the call. A separate global would defeat
  that; leaving the placeholder would leave two ways to reach an allocator.

### Resolution

All four forks taken as recommended. ADR-0062 records them. Both engines run
`050-allocator.jr` to exit 0, and `046-context.jr` was rewritten (a corpus first) because its `s64`
field became a procedure pointer.

One thing found while implementing that was not a fork: `type-errors/` files are checked with modules
*unresolved*, so the E0256 refusal file had to move to `imports/invalid/010` — it needs the import
resolved to reach the case at all. The directory contract caught the misfiling.

---

## Wave: push_context (ADR-0063), 2026-07-31

### What running first revealed (not a fork, but it decided the framing)

Before choosing anything, the standing "verify by running" rule was applied to ADR-0057 §2's claim
that a callee's context writes do not reach its caller. They **do**: a callee that sets
`context.allocator_data = 42` leaves it 42 for a caller that set 7, in both engines. So §2's isolation
half is false, and corpus `050` actually *relies* on the leak (`counting_alloc` accumulates into
`allocator_data` and `main` reads the total). That reframed the wave: `push_context` is not "more"
isolation on top of §2, it is the *only* isolation boundary, and the ADR amends §2 rather than
building on it.

### Fork 1 — which wave

- Options: **`push_context` (taken, recommended)**; traps-with-backtraces; temporary storage; pointer
  arithmetic.
- Why: `push_context` is unblocked (it needs only the aggregate-copy and scope-exit machinery that
  already exist), it is the ADR-0057 §6 gap that turns the just-shipped allocator into a *scoped* one,
  and temporary storage explicitly wants it (ADR-0062 §5). Temporary storage itself is blocked on a
  bump allocator, which is blocked on pointer arithmetic — a taller stack, worth doing after. Traps
  -with-backtraces is independent and equally available, but `push_context` closes a gap the last two
  waves kept naming, so it is the one that reduces the open list rather than adding a parallel track.

### Fork 2 — the form of the construct

- Options: **`push_context { … }` with no explicit value (taken, recommended)**; Jai's
  `push_context <expr> { … }`; a narrower `push_allocator(a) { … }`.
- Why: `Context` is unspellable — naming it is E0212, because it is the first compiler-declared type
  and has no `DeclId` — so a program has no `Context` value to pass, which makes the value-taking form
  impossible without first making `Context` nameable (not needed for the slice). `push_allocator` was
  rejected because it bakes one field into the language, and the context grows fields; scoping the
  whole context and writing the one field you mean inside the block generalises.

### Fork 3 — the lowering mechanism

- Options: **copy-plus-compile-time-pointer-swap in `jr-mir` (taken, recommended)**; a new MIR
  statement/terminator; a runtime context stack with save/restore.
- Why: the swap is which SSA operand `context` resolves to, so leaving the block on any path (fall
  through, `return`, `break`, `continue`) uses the outer pointer with nothing to run — unlike `defer`,
  there is no per-exit emission. It needs no new IR node, VM opcode or Cranelift primitive: the copy is
  the same `Load`/`Store` of an aggregate that `b := a` already lowers. A runtime stack would
  reintroduce the global ADR-0001 refused, and the context is a parameter, not a global.

### Fork 4 — where a `defer` inside the block frees

- Options: **`defer` runs against the pushed context (taken, recommended)**; against the restored
  outer context.
- Why: a `defer context.allocator_free(p)` inside the block should release through the same allocator
  that allocated `p`. The block's defers are emitted on the fall-through path *before* the pointer is
  restored — exactly where an ordinary block emits them — so the pushed context is still in scope. This
  is the one ordering that needed deciding, and it is why restore happens after the block's own defers.

### Resolution

All four forks taken as recommended. ADR-0063 records them and amends ADR-0057 §2. No new diagnostic
code: a `push_context` in a `#c_call` procedure reuses E0254 (needs a context, has none).

---

## Wave: pointer arithmetic (ADR-0064), 2026-07-31

### What running first revealed (not a fork)

`*x[i]` — the address of an indexed element — already works in both engines and scales the index by
the element stride (verified by running: `a: [4]s64; a[2] = 99; p := *a[2]; exit(p.*)` prints 99).
That decided the lowering: `p + n` is the address of `p.*` indexed by `n`, a shape both back ends
already handle, so the wave adds no machine arithmetic.

### Fork 1 — which wave

- Options: **pointer arithmetic (taken, recommended)**; traps-with-backtraces; temporary storage.
- Why: §7 (after ADR-0063) named pointer arithmetic "the blocking gap" — temporary storage wants a
  bump allocator and a bump allocator wants `p + n`. It is more self-contained than
  traps-with-backtraces (whose native half is a call-stack representation entangled with the inliner)
  and it unblocks the W3 feature that is otherwise stuck. Temporary storage itself comes after, once
  this lands.

### Fork 2 — which operations

- Options: **`p ± int` only (taken, recommended)**; add `p - q` (the pointer difference) too; add
  `p[n]` indexing sugar or pointer ordering `< >`.
- Why: `p ± int` is what a bump allocator needs (it only advances a pointer) and it lowers to an
  indexed address the back ends already scale — **no new MIR node**. `p - q` was in the first draft but
  cut mid-wave when the lowering was worked out: its result is a count of *elements*, so it must divide
  the byte distance by the element stride, and the stride is layout that ADR-0017 §5 keeps out of
  `jr-mir` (the back ends scale a `Projection::Index`; `jr-mir` never sees a size). Delivering it would
  need a new MIR node or a layout query `jr-mir` lacks — a decision of its own that the motivating use
  case does not force, so it is deferred (ADR-0064 §5). `p[n]` and pointer ordering are separate
  decisions too; everything else on a pointer stays E0223.

### Fork 3 — scaling

- Options: **element-scaled like C (taken, recommended)**; byte-scaled.
- Why: `p + 1` advancing one `T` is what a systems programmer expects and keeps `p + (q - p) == q`.
  Byte scaling makes stepping to the next element the verbose case (`p + sizeof(T)`) and invites the
  forgotten-stride bug. A `*u8` is where the two coincide and is the common case, which is exactly why
  the rule is chosen for the `*s64` case that differs.

### Fork 4 — bounds checking

- Options: **unchecked (taken, recommended)**; a checked pointer that carries a length.
- Why: a raw pointer has no length to check `n` against — that is the boundary of ADR-0003, not a hole
  in it. The checked type already exists (`[]T`, the view); adding a second length to `*T` would make
  it no longer one machine word. Walking past an allocation is UB by construction, the same trade
  `--no-bounds-check` offers for arrays.

### Resolution

All four forks taken as recommended, with Fork 2 narrowed mid-wave to `p ± int` (dropping `p - q`, now
deferred). ADR-0064 records them and lifts the refusal ADR-0060 §5 deferred. No new diagnostic code
(E0258 still first free), no new MIR node: the type rules are new `check_binary` arms and the lowering
builds an indexed address, which both back ends already scale by element stride.

---

## Wave: temporary storage (ADR-0065), 2026-07-31

### What running first confirmed (not a fork)

Before designing, a program that `malloc`s a region and bumps a cursor through it with pointer
arithmetic (`p + off`, storing and reading back) was run in both engines and gave the expected result.
So temporary storage is not new machinery — it is `malloc` + pointer arithmetic + context fields, all
of which already lower. That decided the wave's shape: two context fields plus Basic code.

### Fork 1 — representation

- Options: **two flattened fields `temp_data: *u8`, `temp_mark: s64` (taken, recommended)**; a nested
  arena struct; a single packed field.
- Why: flat matches ADR-0062's allocator choice and costs nothing here — both `PTR_U8` and `S64` are
  already well-known pool ids, so `CONTEXT_FIELD_TYPES` references them with no new interning and
  `WELL_KNOWN_COUNT` does not move. A nested struct would need a `DeclId` a compiler-declared type has
  not got. `temp_mark` is a byte count (offset), not a pointer, so reset is one integer store.

### Fork 2 — backing memory

- Options: **fixed region, lazily `malloc`'d on first `talloc` (taken, recommended)**; allocate it in
  the entry stub; a growable region.
- Why: lazy means a program that never uses temporary storage never allocates it, and — deciding —
  the entry stub stays free of `malloc`, so `modules/Basic` is not a runtime dependency (ADR-0062 §4's
  argument, reused). Growable means either a `realloc` that moves the arena (invalidating every pointer
  already handed out) or a block list — more than W3 needs. Fixed + null-on-overflow is honest.

### Fork 3 — overflow behaviour

- Options: **return null like `malloc` (taken, recommended)**; trap.
- Why: running out of scratch space is a resource condition a caller may handle (fall back, or reset
  and retry), not a bug that should end the process. One failure convention shared with `malloc`.

### Fork 4 — where the code lives

- Options: **`talloc`/`reset_temporary_storage` in Basic (taken, recommended)**; in the language.
- Why: this is the *opposite* call from ADR-0062 §5, which kept the allocator *protocol* out of Basic.
  The protocol is a language mechanism (how a callee reaches its caller's allocator); temporary storage
  is a *concrete allocator* with one policy, which is exactly what a library provides. It still travels
  with the context (it reads `context.temp_*`), but the code is a library's.

### Resolution

All four forks taken as recommended. ADR-0065 records them. No new pool id, no new diagnostic code
(E0258 still first free), no new MIR node — two context fields plus Basic code from `malloc`, pointer
arithmetic and field access, all of which already lower.

---

## Wave: traps with backtraces (ADR-0066), 2026-07-31

### What running first established (this decided the whole scope)

Five constraints, each verified rather than assumed, and together they define what a backtrace can be:

1. **Native embeds a fixed message string per trap site at compile time** (`report` in
   `jr-codegen-clif/src/body.rs` writes a read-only data object) — a linked binary has no source map.
2. **There is no runtime object at all**: `jr-link`'s docs say the trap helper is *generated into the
   object* by codegen, so no unwinder and no symbol table exist for a stack walk.
3. **The differential harness compares a trapping program's stderr byte for byte**, so both engines
   must produce *identical* backtraces — ruling out "native uses the platform unwinder, VM uses frames".
4. **Inlining erases callee frames on purpose**: both engines consume `optimized_file_mir` (VM at
   `jr-db/src/run.rs:81`, native at `build.rs:66`), and ADR-0021 §3 rewrites every copied span to the
   call site because a callee `MirSpan` names the callee file's arenas. `Splice::span` takes no
   argument, so this is structural.
5. **Neither engine records who called whom.** The VM's `Frame` holds only `regs`/`slots`; a four-deep
   *recursive* chain (which the inliner cannot flatten) still printed one line. So the gap is real
   bookkeeping, not an inlining artefact.

A correction worth recording: a first grep suggested `optimized_file_mir` had *no* production consumer
(only tests), which would have meant inlining was not in the trap path at all. That was wrong — the
grep searched `jr-cli` while the consumers are in `jr-db/src/run.rs` and `build.rs`. Checked again
before relying on it, because the whole scope turned on it.

### Fork 1 — how deep to go

- Options: **a shadow call stack with per-frame *names* (taken, recommended)**; a full source-level
  backtrace with an inline-provenance chain in every `MirSpan`; leave traps as one line.
- Why: the full version is what rustc/LLVM do (`SourceScope`, `DILocation` inline-at chains) and is the
  right long-term answer, but it means every `MirSpan` gains a field every pass must maintain, replacing
  ADR-0021 §3's *structural* guarantee with a discipline no verifier can check — precisely the "a flag
  some passes ignored" shape PLAN.md §5's first failure mode names. Leaving traps as one line abandons a
  W3 feature. The shadow stack delivers "how did I get here" honestly at a wave's size.

### Fork 2 — which stack

- Options: **a shadow stack both engines maintain identically (taken, recommended)**; the platform
  stack via an unwinder.
- Why: constraint 2 says no unwinder or symbol table exists, and constraint 3 says native's output would
  have to match the VM's byte for byte regardless. A shadow stack is the only mechanism *both* engines
  can implement the same way, which makes agreement structural instead of something the harness must
  catch after the fact.

### Fork 3 — what each frame shows

- Options: **the procedure's name (taken, recommended)**; name plus the call site's line; just a count.
- Why: a per-frame line needs a return-address-to-span table embedded in the binary — the subsystem
  Fork 1 declined. The innermost frame's line, the one a reader actually wants, is already there from
  ADR-0020. A name answers the question the chain exists to answer.

### Fork 4 — inlined frames

- Options: **omit them, and say so (taken, recommended)**; reconstruct them from source.
- Why: a frame the inliner removed has no runtime existence — there was no call, so there is nothing to
  push, and both engines agree because the pass is deterministic (fixed 24-statement threshold, same
  `Callees`). Reconstructing them would describe the *source* rather than the *execution*; ADR-0020 §4
  already held that reporting no location beats reporting a neighbouring one, and the same applies to a
  frame. Stated explicitly in ADR-0066 §4 because a reader comparing against their source will notice.

### Resolution

All four forks taken as recommended. ADR-0066 records them and bounds ADR-0020. No new diagnostic code
(a backtrace is a runtime message, not a diagnostic; E0258 still first free).

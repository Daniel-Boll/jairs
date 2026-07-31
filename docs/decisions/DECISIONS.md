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

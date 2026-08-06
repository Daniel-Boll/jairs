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

---

## Wave: switch and exhaustiveness (ADR-0067), 2026-07-31 — W4.5 opens, reordered before W4

### What running first established (it moved the wave)

PLAN §2.1 placed W4.5 after W4 because "exhaustiveness diagnostics **want** comptime type info". That is
a want, not a need, and it was checkable:

- `Pool::enum_members(decl)` already exists (ADR-0041 §4) and is **populated during checking** by
  `jr-sema`'s `ctx.rs`, which is the phase a non-exhaustive `switch` would be reported in. Three sites in
  `jr-sema` already read it.
- `c == .GREEN` and `c == Colour.GREEN` both compile and run in both engines *today* — verified by
  running, not by reading. So resolving a bare `.RED` case against the scrutinee's type and comparing an
  enum value are mechanisms that exist; `check_bare_member` takes an expected type (ADR-0046), which is
  exactly what an arm supplies.

So W4.5's first two deliverables (a `switch`, exhaustiveness) need nothing from W4. Recorded as an
amendment in ADR-0067 §0 rather than done quietly, because PLAN §5 names "plans that contradict
themselves" a project failure mode, and a wave order justified by a dependency that does not exist is
one. What comptime *would* add — exhaustiveness over a computed type (RTTI), a generated `switch`
(`#insert`) — is real but is not what the row asks for.

### Fork 1 — statement or expression

- Options: **a statement (taken, recommended)**; an expression yielding a value.
- Why: the same reason `push_context` is a statement (ADR-0063 §5) — an expression raises "what is its
  type" and "what does a non-exhaustive one evaluate to", and Jairs-0 has nowhere that needs a
  `switch`'s value. A compatible extension later.

### Fork 2 — arm syntax

- Options: **`case <value>;` then statements until the next `case` (taken, recommended)**; a braced block
  per arm; `=>` per arm.
- Why: matches Jai, and reuses the statement-list parsing every block already has, so no new body shape
  enters the grammar. Braces are noise on the common one-statement arm; `=>` is not a token Jairs has and
  would appear in exactly one place.

### Fork 3 — cases: values or patterns

- Options: **values compared with `==` (taken, recommended)**; patterns with destructuring/ranges/guards.
- Why: keeps the wave to what §2.1 asks and to machinery that exists — a `switch` lowers to the chain of
  `==` tests a program writes by hand today. Patterns are a much larger surface and want the tagged
  variant (§7) to be worth having.

### Fork 4 — the catch-all, and whether exhaustiveness is an error

- Options: **`else` as the catch-all, non-exhaustive is an *error* (taken, recommended)**; a new
  `default` keyword; a warning instead of an error.
- Why: `else` is already this language's word for "the other branch", and a second word for one idea is
  a second thing to remember. An error rather than a warning because the whole point of adding matching
  is that the compiler can *prove* a case is handled — a warning leaves the proof optional, the same
  "behaviour depends on something invisible" ADR-0014 §3 refuses. And an `else` on an
  already-exhaustive enum `switch` is itself an error (E0260), since otherwise every `switch` could end
  in `else` and the member check would never fire.

### Fork 5 — fallthrough

- Options: **none (taken, recommended)**; C-style implicit fallthrough; an explicit `fallthrough`.
- Why: implicit fallthrough is the most-regretted control-flow default in this language's lineage and
  Jai does not have it. Sharing an arm between two values is a future multi-value `case`, recorded as
  absent rather than faked.

### Resolution

All five forks taken as recommended. ADR-0067 records them and amends PLAN §2.1's wave order (§0). Three
new diagnostic codes — E0258 non-exhaustive, E0259 duplicate `case`/second `else`, E0260 unreachable
`else` — so **E0261 becomes the first free code**, the first wave in five to add any. No new MIR node:
the lowering is the `if`/`else if` branch chain that already exists. The tagged variant type, W4.5's
third deliverable, stays for its own wave (§7) and is now unblocked by this one.

---

## Wave: tagged variants (ADR-0068), 2026-07-31 — closes W4.5

### What reading first established

ADR-0045 §1 rejected a tagged `union` on three grounds, and **two of them have since gone away**:
"Jairs has no pattern matching" (ADR-0067 shipped `switch` last wave — the ground that ADR called
decisive) and "a program has no way to *ask* which field is live" (a `switch` over the tag is that
question). The third stands — a tag makes the type bigger than its largest field — which is exactly why
ADR-0045 said the tagged thing should be **a different declaration form**, "the way `enum_flags` is
different from `enum`", rather than a change to `union`. This wave follows that instruction.

A second constraint was found by reading `Struct::is_union`'s own doc: the struct/union arena is shared
**deliberately**, because a `DeclId` names an index but not an arena, so a separate `unions` arena would
collide with structs while both share `Pool::struct_fields`. A third form therefore cannot be a third
arena — and a `bool` cannot express three kinds, so the flag has to become an enum.

### Fork 1 — the declaration form

- Options: **a new `variant { … }` keyword (taken, recommended)**; `#tagged union { … }`; reuse `union`
  with an optional tag.
- Why: ADR-0045 §1 instructed exactly this, and the reason holds — the tagged thing has a different size
  and a different access cost, so it is a different *type*, not a `union` with a flag. An attribute reads
  as a modifier on a union; reusing `union` is the "silent change to what `union` means" that ADR forbade.

### Fork 2 — how three forms share one arena

- Options: **`Struct::is_union: bool` becomes `kind: AggregateKind` (taken, recommended)**; a second bool;
  a separate arena.
- Why: a separate arena is *unrepresentable* (colliding `DeclId`s, per the field's own doc). Two bools
  would allow the nonsense state "union and variant". An enum makes each of the nine readers an
  exhaustive match, so a fourth form would be a compile error at every site that must decide rather than
  a `false` silently meaning "struct" — the project's first named failure mode applied to a flag.

### Fork 3 — where the tag lives and how wide it is

- Options: **a leading `u8` field (taken, recommended)**; trailing; the case's own index type.
- Why: leading means offset 0 regardless of what follows, so nothing computes a position from the case
  count — the same argument ADR-0057 §4 used for the context parameter. A trailing tag sits at an offset
  derived from the largest case, which every reader would re-derive. `u8` because no expressible variant
  has more than 256 cases, and padding usually absorbs the byte. Layout stays the existing
  `sequential_layout` over `[tag, union-of-cases]`, so no new layout algorithm.

### Fork 4 — what a wrong-field read does

- Options: **trap at run time (taken, recommended)**; a static diagnostic; return garbage as `union` does.
- Why: which field is live is not statically decidable — ADR-0045 §1 settled that when it rejected a
  static cross-field check as "either unsound or maddening". The tag makes the *runtime* answer available,
  which converts an undetectable bit reinterpretation into a located trap. Not strippable by
  `--no-bounds-check`: that setting removes a check redundant with a proof the programmer has, whereas a
  variant's tag is the only thing that knows which field is live — removing it removes the type's meaning.
  A program that wants no check writes `union`, which still costs nothing.

### Fork 5 — how `switch` destructures it

- Options: **an arm names a *field*, exhaustiveness over the cases, reusing ADR-0067 (taken,
  recommended)**; a binding form `case i => n;`; a separate `match` construct.
- Why: reusing last wave's arms means one matching mechanism rather than two, and exhaustiveness falls out
  — E0258 lists missing cases and E0260 still refuses a redundant `else`, with **no new diagnostic code**,
  which is the evidence the shapes fit. A binding form is the pattern surface ADR-0067 §2 declined, and
  adding it here would take that decision sideways.

### Resolution

All five forks taken as recommended. ADR-0068 records them and follows ADR-0045 §1's instruction rather
than reversing it. One new keyword (`variant`), one new pool item, one new `TrapKind`, **no new
diagnostic code** (E0261 still first free). The `is_union` → `kind` change is the largest and least
interesting part of the diff, which the ADR says so a reviewer reads its shape rather than each line.

### Fork 5a — the case spelling, corrected mid-wave

- Options: **a bare `.member`, reusing ADR-0046 (taken, recommended)**; a bare `member` name as ADR-0068
  §5's draft wrote it; a `case v.i;` path form.
- Why: writing `case i;` looked natural but does not work, and running it said so — a bare `i` goes
  through ordinary name resolution and reports E0201 for a name nothing declared. A bare `.i` already
  parses and already arrives at `check_bare_member` with the scrutinee's type as its expected type
  (ADR-0046's "the context supplies what the source omits"), so accepting a *variant* there is a smaller
  change than a new resolution rule — and it makes `case .i` read exactly like an enum switch's
  `case .RED`. ADR-0068 §5 is corrected to record this rather than left describing a form that fails.

---

## Wave: `#run` across files and in a body (ADR-0069), 2026-07-31 — W4 sub-wave 1

### What running first established, and what it corrected

§7 has said for waves that the compiler has "one *trivial* `#run`: a call or a constant expression, same
file only". Running it showed **two of those three qualifiers were wrong**: nested calls
(`#run add(add(1,2),3)`), arithmetic around a call (`#run add(1,2) * 10`) and a `while` loop in the
callee (`#run sum(5)`) all already evaluate. The handoff was *underselling* the compiler — the mirror of
the rot §7 warns about, and worth correcting in the same breath, because a handoff that undersells is as
untrustworthy as one that oversells.

Two things genuinely do not work, and the first is worse than an absence: a `#run` calling an *imported*
procedure reports `internal compiler error: no routine for file 1 proc 11` — compiler internals shown to
a user who wrote a reasonable program. The cause is one line: `file_consts` calls `add_file` for the file
being evaluated and no other. The second: a `#run` in a *body* does not lower at all.

### Fork 1 — how to take on W4 at all

- Options: **split it into four sub-waves, each its own ADR and each shippable (taken, recommended)**;
  attempt the wave whole; reorder W4 behind W5.
- Why: every other wave here has been one ADR and one branch. A 10–14 week wave attempted whole cannot be
  verified the way the others were — the handoff at the end would be a claim nobody could re-run in a
  sitting, which is precisely how §7 "rots toward *what remains is small*". The sub-waves are numbered so
  a later one can be reordered on evidence (as ADR-0067 §0 reordered W4.5), not as a commitment.

### Fork 2 — the cross-file ICE

- Options: **add every reachable file's bytecode to the comptime program (taken, recommended)**; refuse
  cross-file `#run` with a new actionable diagnostic; leave it.
- Why: the refusal was the other honest answer and was seriously considered — a code saying "a `#run`
  cannot call an imported procedure yet" would at least stop leaking internals. Rejected because the
  limitation is not real: the call resolves, the callee has MIR, and the only thing missing was that
  nobody put it in the program. A diagnostic explaining a limitation the compiler does not have is worse
  than none. And this is *not* the cross-file dependency `consts.rs` refuses at length — that refusal is
  about reading another file's constant **values** (`ImportedValues` stays empty); a routine is not a
  value, and `imported_procs` already resolves cross-file procedures acyclically today.

### Fork 3 — what a body `#run` lowers to

- Options: **the constant const-eval computed, evaluated in `file_consts` (taken, recommended)**; a call
  into the VM at run time; keep it refused.
- Why: `#run` runs at *compile* time and the body gets a value — that is what the construct means, and
  ADR-0016 §4 already arranged it for file-scope constants. Lowering it as a runtime VM call would
  reverse that and make a `#run` in a hot loop a per-iteration interpreter call. Evaluating it in
  `file_consts` rather than a second place keeps one round-robin and one cycle detector: two evaluators
  would be two chances to disagree about what a `#run` means.

### Fork 4 — whether the existing refusals move

- Options: **unchanged, and stated (taken, recommended)**; lift some for a body `#run`.
- Why: an operator overload, a default or named argument, and an imported constant are refused inside a
  `#run` because const-eval runs before the check phase that resolves them (ADR-0018 §3's cycle). The
  *position* changing does not change the phase ordering, so the rules are identical — stated explicitly
  because a new position might suggest otherwise.

### Resolution

All four forks taken as recommended. ADR-0069 records them. **No new diagnostic code** (§1 removes a
failure, §2 lifts a refusal — E0261 still first free), and §7's "one trivial `#run`" claim is corrected in
both directions.

### Fork 2a — where the imported file's MIR comes from (forced mid-wave)

- Options: **lower it inside `file_consts` from the front-end queries (taken, recommended)**; take it
  from `file_mir`; give `file_consts` a salsa cycle-recovery function.
- Why: taking it from `file_mir` is the obvious implementation and salsa rejected it outright —
  `file_consts(A) → file_mir(B) → imported_values(B) → file_consts(A)`, because `file_mir` folds imported
  constants — and three corpus tests failed with a cycle panic. Lowering it here from `imported_procs`,
  `checked` and `resolved` (all already called from this module) with the same empty
  `ImportedValues`/`OperatorCalls`/`FilledArgs` this module already uses for its own file is not a
  workaround but the honest position: const-eval precedes the check phase for an imported file exactly as
  for the local one, so an imported callee gets precisely the same restrictions. A cycle-recovery function
  would have made the *fixpoint* the answer to a question that has a direct one.
- **ADR-0069 §1's claim that this "adds no dependency that was not already there" was wrong about
  `file_mir`** and is corrected in the ADR rather than quietly patched, because the obvious
  implementation hits the same cycle.

---

## Wave: an array length from a constant (ADR-0070), 2026-07-31 — W4 sub-wave 2, rescoped

### What probing established, and why the wave changed shape

ADR-0069 §0 scheduled "aggressive const folding" here, and ADR-0069 §4 plus §7 both said a `#run` result
"reaches the body as a constant, but the arithmetic around it is not re-folded". **All of that is wrong
about the optimized MIR**, which is what both engines consume. The built MIR does show
`v1: s64 = 5_s64 * 10_s64`, which is what the claim was looking at — but the optimized body for
`m := n * 10 + 7; return sink(m);` is a single `Return(Some(Constant(...)))`, and the program exits 57.
ADR-0022's const-prop folds through a `#run` result exactly as through any other constant, because by the
time it runs the `#run` *is* one.

Seeing this took a probe of the *optimized* body: the corpus snapshots the **built** query, so the
distinction was invisible in everything the tests display. That is the second time this session a
scheduled dependency turned out not to exist (ADR-0067 §0 was the first), which makes it a pattern worth
naming rather than a coincidence.

### Fork 1 — what to do with a sub-wave whose work is already done

- Options: **rescope it to a real gap (taken, recommended)**; implement folding in the *built* MIR anyway;
  skip to sub-wave 3.
- Why: folding the built MIR would be work for a claim rather than a capability — the engines consume the
  optimized query, and duplicating const-prop into the builder would give two answers to "what does
  `2 + 2` mean". Skipping ahead would leave §7 asserting something false. Rescoping keeps the sub-wave
  and records the correction.

### Fork 2 — which gap

- Options: **`[N]T` where `N` names a literal-valued constant (taken, recommended)**; a `#must` ADR; the
  three cross-file gaps; `p - q`.
- Why: ADR-0039 §3a deferred `[COUNT]u8` explicitly and it is the most-felt absence — `modules/Basic`'s
  `print_int` still owes a `[20]u8` buffer and cannot name its size. It is also the one that turns out
  **not** to need the sema↔comptime recursion ADR-0039 assumed, which makes it available now.

### Fork 3 — how far to go without inverting the phase order

- Options: **a length that is already a literal one name away (taken, recommended)**; fold arithmetic in
  sema too; thread `ConstValues` into sema.
- Why: for `N :: 4` nothing needs evaluating — the literal is in the HIR, and `Ctx` already holds `hir`
  and `resolve` and already resolves type names against the file scope. Verified rather than assumed:
  `jr-sema`'s `Cargo.toml` depends on neither `jr-vm` nor `jr-db`, and still does. Folding arithmetic
  would mean a *second* constant folder beside ADR-0022's — the duplication ADR-0018 §2 refuses for layout
  and ADR-0020 §2 for trap messages. Threading `ConstValues` in is the dependency inversion ADR-0039 §3a
  named and it stays refused.
- So ADR-0039 §3a is **half amended**: the literal-valued case arrives now, and every case needing an
  actual *value* — arithmetic, `#run`, a cross-file constant — still waits for the RTTI sub-wave.

### Fork 4 — the diagnostic

- Options: **reword E0233 to name what was found (taken, recommended)**; keep the message; add a code.
- Why: after Fork 3 the current text — "an array length must be an integer literal" — is simply false; a
  literal-valued *constant* is now accepted. A reader should learn which side of the line they are on. No
  new code, because E0233 already means "this length is not usable" (E0261 stays first free).

### Resolution

All four forks taken as recommended. ADR-0070 records them and amends ADR-0039 §3a. No new diagnostic
code, no new pool item, no MIR change — after this, nothing downstream can tell how the length was
written, which is the evidence it belongs where it was put.

---

## Wave: type values (ADR-0071), 2026-07-31 — W4 sub-wave 3, scoped

### What running found: a silent miscompile, not a gap

`t := Point;` — a bare type name bound to a local — **type-checks cleanly and compiles in both engines
today**, exiting 0. Its MIR is `s0: type` and `v1: type = undef`: a placeholder stored into a slot whose
type has *no runtime layout at all* (`layout_of` answers `ComptimeOnly` for `Item::TypeType`, whose docs
call asking for its size "a category error"). That is PLAN §5's first failure mode exactly — a construct
the grammar allows, no representation on the lowering path, filled in with a placeholder that is a
legitimate value — so neither the verifier nor ADR-0017 §4's poison gate catches it.

Only the MIR dump shows it. Third wave running that a false claim survived because nothing displayed the
contradicting thing.

The other half: `T :: Point;` (a type alias) fails with "compile-time evaluation failed: a file-level item
has no value yet" — a const-eval internal for a natural construct, because `file_consts` deliberately does
not treat a struct as an evaluation target.

### Fork 1 — how much of RTTI belongs in this sub-wave

- Options: **`Type` values only, deferring `type_info()` and `Any` (taken, recommended)**; all three
  together; `Type` plus `type_info()`.
- Why: the three divide on one question — *does the value exist at run time?* A `Type` does not
  (`ComptimeOnly`), so it never reaches a back end and adds no engine risk. `type_info()` returns a struct
  *describing* a type, which does exist at run time and needs that struct declared, populated and laid
  out; `Any` is a `{type, pointer}` pair needing the same plus rules for what goes in and how it comes
  out. So `Any` is not "more RTTI" — it is the first construct that makes a type into runtime data, and it
  is what §5's "sema and the VM become mutually recursive" is actually about. Splitting here keeps this
  sub-wave's claim checkable: afterwards, a type is a compile-time value and nothing else.

### Fork 2 — where the runtime refusal lives

- Options: **`jr-sema`, when a name's type comes back as `PoolId::TYPE` in a body (taken, recommended)**;
  `jr-mir`'s `scan`; leave it and let the back end refuse.
- Why: rejecting a construct is a semantic judgement — ADR-0039 §3a's reason for array lengths and
  ADR-0017 §4's generally — and a lowering refusal produces a compiler-internal message for a
  well-formed-looking program, which is exactly what the alias case was already doing. `type-errors/`
  files must lower cleanly, so the diagnostic has to be sema's. Leaving it to the back end is what
  produced the placeholder.

### Fork 3 — where a type-valued constant's value comes from

- Options: **`FileSignatures::type_value`, which the signature phase already computed (taken,
  recommended)**; make a struct an ordinary const-eval target; a new side table.
- Why: const-eval is downstream of *signatures* (ADR-0018 §3), so this reads a value that already exists
  rather than inverting a phase — the same move ADR-0070 §1 made for an array length, available for the
  same reason. Making a struct a const-eval target would mean evaluating something with nothing to
  evaluate (its "value" is a declaration, as `wanted`'s docs argue), and the thunk would still have to
  know it had produced a type rather than a number.

### Fork 4 — how far the capability goes

- Options: **one level of aliasing, no type comparison, no `Type` parameters (taken, recommended)**; add
  `T == U`; allow a `Type` parameter.
- Why: a chain (`B :: A` where `A :: Point`) needs a fixpoint and a cycle check, which is the same
  machinery ADR-0070 §4 declined for a length chain. `T == U` is decidable and cheap — a `PoolId`
  comparison — but its *meaning* is ADR-0015's type-identity question, and settling that in passing would
  answer a design question this ADR has no argument for. A `Type` parameter is a second route to W5's
  `$T`.

### Resolution

All four forks taken as recommended. ADR-0071 records them. **One new diagnostic code, E0261** (a type used
at run time), so **E0262 becomes the first free code** — the wave's real content is removing a silent
miscompile, which is why it ships separately rather than inside a larger RTTI change. No MIR change and no
back-end change: a type value never reaches either.

---

## Wave: `#insert` (ADR-0072), 2026-07-31 — W4 sub-wave 4, scoped

### What running found first

`#insert` and `#code` are both genuinely absent (E0209 and E0100 respectively) — worth checking, because
the previous two sub-waves each found their scheduled work already delivered (ADR-0067 §0, ADR-0070 §0).
Three further facts, checked rather than assumed:

* **`jr-hir` already depends on `jr-syntax`** and `jr_syntax::parse` is public, so lowering can parse a
  string of source with no new dependency. That is why a *literal* `#insert` needs none of W4's mutual
  recursion.
* **The parser already produces the node.** `#insert "text"` parses as the generic `DIRECTIVE_EXPR` with a
  `string_arg`, because the lexer is deliberately permissive about `#anything`. No grammar change, no
  lexer change, no new `SyntaxKind` — so gate 6 has nothing new to check and `grammar.js` is untouched.
* **A `Span` is `(FileId, TextRange)` into a real file, and out-of-range offsets are *clamped*, not
  rejected** — `jr-diag`'s renderer takes `.min(primary_len)` so it "never panics". So a span into
  synthesized text is caught by nothing: it silently underlines real source the user did not write and
  says the error is there. This single fact decides the whole design.

### Fork 1 — how much of sub-wave 4 to take

- Options: **`#insert "literal"` only, spans pointing at the directive (taken, recommended)**; `#insert` of
  a computed string; `Code` as a first-class value first; skip to a deferred item.
- Why: a computed operand is where sema and the VM become mutually recursive, and the reason is a
  dependency *direction* rather than difficulty — lowering produces the HIR that `resolved` consumes,
  which `checked` consumes, which `file_consts` consumes, so `#insert build_it()` would need
  `file_hir → file_consts → checked → resolved → file_hir`. That is a salsa cycle, the same shape ADR-0069
  §1 had to restructure around. A `Code` value needs a representation for a quoted syntax tree, whose first
  question is ADR-0071 §4's — does it exist at run time? — and it is only useful once something can splice
  it. The literal form is the smallest thing that is genuinely `#insert`, and it needs no VM at all.

### Fork 2 — what a diagnostic inside inserted code points at

- Options: **the `#insert` directive, plus a note naming the offset in the inserted text (taken,
  recommended)**; a synthesized `FileId` for the inserted text; the directive and nothing more.
- Why: the directive's span is *honest* — the `#insert` is where that code entered the program, there is no
  other place it exists — and always in range, so the clamping above can never fire on it. A synthesized
  `FileId` gives genuinely-real spans and the better message, and was rejected on cost: a `FileId` is a
  load-order index (AGENTS.md forbids printing one into a snapshot for this reason), and four subsystems
  would have to learn about a file with no path — the `SourceMap`, the module loader, salsa's inputs and
  the LSP's document store. It can be added later without changing what programs mean. Saying nothing more
  produces a diagnostic the reader cannot act on the moment an insert has two statements, which is
  ADR-0043's "true and useless" failure.

### Resolution

Both forks taken as recommended. ADR-0072 records them, and §5 lists what is deliberately absent: a
computed *or named* operand (refused even when the string might already be known, so the refusal does not
depend on how the string was written), `#code`/`Code`, `#insert` at file scope (it would change the item
tree, so the signature phase would see declarations no file walk produced), and — as drafted — a nested `#insert`.

**That last one was wrong, and running it is what showed why.** Nesting *works*, with no code: the
recursion falls out of `lower_stmt` calling itself. And it cannot run away, because **escaping doubles the
text at every level** — 12 levels is 8 KB of source, 18 levels is 512 KB, 40 levels would be ~10¹² bytes.
A literal `#insert` is bounded by the file it is written in, so no depth bound is needed. (My first attempt
to test this generated a 40-level file and appeared to hang; the file had never been written, and the real
lesson was about the exponent.) A depth bound *will* be owed when the computed operand of §4 arrives, since
a generated string can reproduce itself without growing. Two new codes, so **E0264 becomes the
first free code**. Lowered in `jr-hir` rather than spliced pre-parse, so the CST stays lossless — a
pre-parse splice would leave no node for the directive, and the formatter would delete it, which is
precisely the `is_stmt_kind` failure that destroyed source four times in one wave.

### Three things implementing it changed

* **E0262's corpus file went to `imports/invalid/`, and the rule is the stage rather than the imports.**
  `type-errors/`' harness requires its files to "parse, lower and resolve cleanly" *before* checking the
  code they declare, and E0262 comes out of **lowering** — so the file failed two `jr-sema` corpus tests as
  first written. ADR-0050's `using` refusals are in `imports/invalid/` for the same reason, so this used a
  precedent rather than weakening a contract.
* **E0263 re-words the parser's E0114 rather than adding a parser code.** Same fault — a token where a
  statement belongs — differing only in which text the offset indexes.
* **The number that distinguishes the two designs was asserted nowhere.** The corpus differential checks
  only that both engines *agree*, and giving an insert its own defer scope makes both exit **63** in
  agreement — whole suite green but for one MIR snapshot diff. Verified by making that change. `#insert`'s
  corpus program must exit **64**, and that now has its own test instead of resting on a snapshot a
  reviewer could accept. Generalising §5's lesson: when a claim is about behaviour, assert the behaviour.

**Eight tests (936 → 944)**, each teeth-checked by disabling the mechanism it pins — neutering the span
override fails exactly the two span tests, pushing a scope fails exactly the enclosing-scope test, and a
defer scope fails exactly the exit-status test.

---

## Wave: `#insert` of a computed string (ADR-0073), 2026-08-03 — W4 sub-wave 5

### What running found first

PLAN §7 called W4's remainder "one problem with two faces". It is two problems, and probing separated them
in minutes:

* **A `#run`-computed *string* constant already works** — `S :: #run mk();` checks cleanly. So the value a
  computed `#insert` needs is one const-eval already produces.
* **A `#run`-computed *struct* is refused by name**: E0230, "a compile-time struct value arrives with a
  later wave", because `jr-pool`'s `Item` has **no aggregate value variant at all**. `type_info()` returns
  a struct, so it is blocked on a *representation*, not on a dependency direction. It is a different
  sub-wave and the E0230 refusal is what to lift first.
* **`#insert S;` does not parse today** — the parser wants `;` after `#insert`, so a bare name is E0100
  *then* E0262. Unlike ADR-0072, this wave needs a grammar change, so gate 6 acquires work.
* **`file_signatures` depends only on `file_hir` and `resolved`**, never on `checked` or `file_consts`.
  That single fact is what makes an acyclic pre-pass possible, and it was read out of the query.

The cycle is also sharper than ADR-0072 §4 drew it: `file_consts` is gated on `frontend_diagnostics`, which
reaches `checked`, `resolved` **and `lower_file` directly**. So the loop closes through the *error gate*,
not merely through the type checker.

### Fork 1 — which sub-wave comes next

- Options: **a computed `#insert` (taken, recommended)**; `type_info()` first; aggregate constants as their
  own prerequisite-only wave.
- Why: the computed `#insert` needs **no new pool variant** — only the dependency direction inverted — and
  it is the wave's *named* deliverable (cycle detection with readable errors). `type_info()` needs a
  describing struct in `modules/Basic`, a new aggregate-value representation, static data emission and a
  layout: four decisions before one program runs. An aggregate-constants wave is smaller and independently
  testable but ships no user-visible feature.

### Fork 2 — how to break the cycle

- Options: **a narrow acyclic pre-pass query (taken, recommended)**; salsa's fixed-point cycle recovery;
  refuse with a better diagnostic and defer.
- Why: **salsa 0.28.1 does support `cycle_fn`/`cycle_initial`**, which the plan never mentioned, and it is
  the more general answer — it would also serve a `#run` reading another file's constant. Rejected on two
  grounds. Convergence would have to be *proved*: an insert whose text declares what another insert's text
  reads is a fixpoint whose termination is a property of the program, and a wrong fixpoint is a silently
  wrong program. And decisively, opting a query into recovery **removes salsa's cycle panic as a guard** —
  the panic that caught ADR-0069's mistake in three corpus tests at once. Disabling the project's best cycle
  detector to gain a feature a narrow query delivers acyclically is a poor trade.

### Resolution

Both forks taken as recommended. ADR-0073 records them. The pre-pass evaluates **string-valued constants
only** (§2), because a general one would be a second partial const-eval, and two evaluators that must agree
is the shape ADR-0019 refuses for the two execution engines. This wave also owes the **depth bound**
ADR-0072 §5 named: a computed operand can reproduce itself without growing, so the escaping argument that
bounded a literal insert no longer applies. And it corrects the plan: a `#run` reading another file's
constant does **not** come free, since the general mechanism that would deliver it is the one rejected.

---

## Wave: aggregate constants (ADR-0074), 2026-08-03 — W4 sub-wave 6

### What running found first

```
P :: struct { x: s64; y: s64; }
V :: #run mk();      // error[E0230]: a compile-time struct value arrives with a later wave
```

* **The gap is in `jr-pool` alone.** `Item` has eight value variants and not one of them is an aggregate,
  which is exactly what `reduce` says when it refuses.
* **Both engines can already hold one**: the VM has `Value::Aggregate(Vec<u8>)` (it builds one for a string
  today) and `jr-codegen-clif` already emits static data via `define_data`. So this is a representation
  decision, not a back-end one.
* **The refusal covers arrays too and its message does not say so** — a `#run` returning `[2]s64` is told
  about structs.
* **`string` already works**, because `reduce` special-cases it and interns the *text* rather than bytes.
  That is the shape the decision generalises.

### The fork — how an aggregate constant is represented

- Options: **the field values, in order, as `Item::AggregateValue(Vec<PoolId>)` (taken, recommended)**; the
  byte image the VM already produced; a side table keyed by declaration.
- Why: **the pool is target-independent** — `layout_of(pool, target, ty)` takes a `TargetLayout` and the
  `Pool` holds none — so a byte image would put a target fact inside the shared pool. The VM writes those
  bytes with `write_le` at offsets for one target, so interning them would bake in the host's padding and
  pointer width; a cross-compile would then read plausible wrong values rather than fail, and every target
  in the slice being `LP64` is exactly why it would go unnoticed. Field-wise interning has no target in it,
  and `field_offset(pool, target, …)` already turns it into bytes at the point that knows which target is
  meant. A declaration-keyed side table works for field *types* (which belong to the declaration) and not
  for values, since two constants of one type differ — the key would have to be the constant, which is what
  `intern` already is.

### Resolution

Fork taken as recommended. ADR-0074 records it. Scope is **struct and array**; `string` keeps `StrValue`
(its contents are its identity and its runtime form is a pointer, which has no compile-time value), and a
**union constant is refused** because untagged storage makes "which field is valid" unanswerable
(ADR-0045 §1). A struct or array **literal** stays absent — that is ADR-0039 §6's syntax question, and
worth stating because after this `V :: #run mk();` works while `V :: P.{1, 2};` still does not parse.
`Item` gains its first **recursive** value variant, so every exhaustive walk must decide what a nested
aggregate means — the mechanism that found ADR-0068's two wrong answers.

---

## Wave: `Any` (ADR-0076, ADR-0077), 2026-08-03

Recommended option taken automatically per the standing request; the user restated during this wave
that the recommended choice should always be taken and logged here for later review, without blocking.

### Fork 1 — which wave next (after ADR-0075)

- Options: **pointer conversions then `Any` (taken, recommended)**; pointer conversions alone; per-kind
  `Type_Info` detail; `#code`/`Code`.
- Why: probing showed the `{*Type_Info, *u8}` pair `Any` needs already works for a `u8` (erase, store,
  read back — exit 8), so the only blocker is that a `*T` cannot become the erased `*u8`: `cast` refuses
  it (E0232). That is the honest remaining piece, and doing the conversion *and* `Any` in one wave reaches
  a user-visible capability, which ADR-0075 named as the next step. `#code` was rejected as this wave
  because it does not even parse — it needs new grammar across parser, CST, tree-sitter and formatter, the
  widest surface of the three. Per-kind detail is independent of the pointer question and can come later.

### Fork 2 — how a pointer erases to `*u8`

- Options: **implicit `*T` → `*u8` only at an `Any` boundary (taken, recommended)**; a general
  `cast(*u8, p)`; a `reinterpret`/`transmute` keyword.
- Why: a general pointer cast makes every pointer type interconvertible, so a wrong pointee type becomes a
  silent wrong read — the reinterpretation ADR-0045 §1 confined to `union`, put on every pointer unmarked.
  `Any` needs one direction of one conversion, with a *checked* inverse (Fork 4); a general cast supplies
  the forward step and leaves the reverse unchecked, the worst split. A `transmute` keyword is a bigger
  feature than `Any` needs and would have to answer cross-size questions no user is waiting on. Implicit
  rather than `xx`, because an `Any` parameter already declares the callee does not know the type — marking
  it twice reads as though something lossy happened, and nothing is lost (ADR-0057's argument for the
  implicit context parameter).

### Fork 3 — where `Any` is declared

- Options: **in `modules/Basic`, validated on lookup (taken, recommended)**; a compiler-declared
  structural type like `Context`.
- Why: ADR-0075 §2's argument, unchanged and now load-bearing twice — a `Type_Info`/`Any` a program must
  *name* has to be spellable, and no compiler-declared type is (`t: Type;`, `c: Context;` both E0212). The
  validation mechanism (E0265) gains a second client, the first evidence it generalises.

### Fork 4 — how `any_as` establishes type identity at run time (ADR-0077)

- Options: **add a stable `id: s64` (the pool id) to `Type_Info` (taken, recommended)**; emit each
  `Type_Info` as deduplicated static data so pointers compare equal; compare by `name` string; defer
  `any_as` to a follow-up sub-wave.
- Why: running proved two `type_info(Point)` calls have **different addresses** (ADR-0075 by-value return
  spills a fresh slot each time), so pointer comparison is out — which ADR-0076 §2 already anticipated by
  saying "compare what it says". But the four-field schema *says* nothing usable: `kind` is too coarse,
  `size`/`alignment` collide (`Point` and `[2]s64` are both 16/8), and `name` is **unsound** because
  nominal identity is a declaration site, not a spelling (ADR-0015 §1) — a local `Point` and an imported
  one share a name and are different types, so matching on it is the silent bad read the check exists to
  prevent. The pool id is *the* identity the whole compiler already uses and is identical in both engines
  because they share one pool, so `any_as` becomes a plain integer compare the differential checks like any
  other. Static-data dedup would also work but drags in the memory-ownership decision ADR-0075 §2 deferred.
  This amends ADR-0075 §3's schema, so it gets its own ADR (ADR-0077) the way ADR-0018 §5 amends ADR-0017,
  rather than an edit to ADR-0075.

### Resolution

Forks taken as recommended. ADR-0076 records `Any` and the erasing conversion; ADR-0077 records the
`Type_Info.id` amendment. `any_of(p)` erases a pointer, `any_as(a, T)` reads it back trapping on an
`id` mismatch (ADR-0068's tagged-read rule, one level up). Deliberately absent: every value coercing to
`Any` implicitly (a literal has no address, so it would need a materialised temporary — the storage
decision deferred again); an `Any` in a compile-time constant (interning a pointer, which has no
comptime value, ADR-0074 §2); a general pointer cast; `transmute`.

### Fork 5 — an ADR-0076 §1 gap found after merge: implicit `*T` → `Any` coercion

- Found by probing `takes(*n)` where `takes :: (a: Any)`: it reports E0214 (`expected Any, found *s64`),
  but **ADR-0076 §1 explicitly promised it**: "`any_of(p)` — *and passing a `*T` where an `Any` is
  expected* — erases". Sub-wave 8 shipped only the explicit `any_of` form.
- Options: **complete the promised coercion (taken, recommended)**; amend ADR-0076 to make `any_of` the
  only form.
- Why complete rather than amend: the ADR is accepted and the coercion is the ergonomic half — a
  reflection API where every argument must be wrapped in `any_of(*x)` is clunky, and `print_any(x)` reading
  naturally is the whole point. Leaving it unimplemented is the "plans that contradict themselves" failure
  AGENTS.md names. It reuses the `any_of` lowering exactly, so it is small. Distinct from ADR-0076 §4's
  *deferred* coercion, which is a bare **value** (`a: Any = 3;`) needing a materialised temporary — this is
  a **pointer**, whose lifetime is already visible, which §1 put in scope.
- This is a follow-up commit on `main` completing sub-wave 8's accepted scope, not a new wave.

---

## Wave: per-kind `Type_Info` detail — fixed-size slice (ADR-0078), 2026-08-03

Recommended option taken automatically per the standing request; logged here for later review.

### Fork 1 — which wave next (after `Any` and its coercion)

- Options: **per-kind `Type_Info` detail, fixed-size slice only (taken, recommended)**; `#code`/`Code`;
  making `type_info` accept a structural type argument (`type_info([4]s64)`).
- Why: per-kind detail is what ADR-0075 §3 explicitly deferred and is the honest completion of RTTI. The
  key insight that makes it a *small* wave: a struct's field **count**, an array's **length**, and an
  array/pointer's **element type id** are all **fixed-size `s64`s** — they need none of the
  memory-ownership decision ADR-0075 §3 flagged, which is only about the variable-length field *list*.
  So the fixed-size facts ship now with no new representation, and the list stays deferred with its
  ownership question intact. `#code` was rejected as next because it is a whole new grammar family (it
  does not parse at all). The `type_info([4]s64)` gap was rejected because making one intrinsic parse a
  *type* in argument position turns it into a syntactic special form — invasive and unprincipled; a
  structural type *alias* (`Arr :: [4]s64;`) would be the clean fix, but that is ADR-0071 §5's deferred
  fixpoint territory and its own wave.

### Fork 2 — how the extra facts are shaped in `Type_Info`

- Options: **flat optional fields, zero when irrelevant (taken, recommended)**; a per-kind `union`; a
  separate `type_info_struct(T)` returning a different struct per kind.
- Why: flat fields (`element: s64` — the element type's id, 0 for a non-element kind; `count: s64` — a
  struct's field count or an array's length, 0 otherwise) extend the schema by *adding fields*, which
  ADR-0075 §3 already said "does not break a reader that names only the [existing] fields", and needs no
  new machinery — the builder fills them from the pool it already reads. A `union` reintroduces the
  "which field is valid" problem `Any` exists to solve and which ADR-0045 refuses for unions. A
  per-kind struct multiplies the compiler's `Basic` dependency by the number of kinds. `element` is an
  `id` (a pool id, like `Type_Info.id`) rather than a `*Type_Info`, because a `*Type_Info` would need
  the element's `Type_Info` to be built and live somewhere — the static-data decision deferred; an id is
  a fixed `s64` and a program recovers the element's `Type_Info` by other means later.

### Resolution (pending — ADR-0078 records it)

Taken as recommended: `Type_Info` gains `count` and `element` (both `s64`, 0 when not applicable),
filled for struct (field count), array (length + element id) and pointer (pointee id) kinds. The
variable-length field *list* stays deferred with its memory-ownership question. Amends ADR-0075 §3 the
way ADR-0077 did, via a new ADR.

---

## Wave: `Type_Info`'s field list (ADR-0079), 2026-08-03

Recommended option taken automatically per the standing request; logged for later review.

### Fork 1 — which wave next (to finish W4)

- Options: **the variable-length field list (taken, recommended)**; `#code`/`Code`; a cross-file `#run`
  value.
- Why: it is the last *RTTI* gap and what a struct printer needs, so it completes a story three ADRs have
  been building. It is also the piece whose blocking question — memory ownership — turns out to be
  **already answered twice** (see Fork 2), so the "large decision" the earlier ADRs deferred is smaller
  than it looked. `#code` remains the widest surface (new grammar) and a cross-file `#run` value needs its
  own cycle analysis; both are better done after RTTI is closed.

### Fork 2 — where the field elements live (the deferred memory-ownership question)

- Options: **the mechanism both engines already use for string literals (taken, recommended)** — VM interns
  into its own memory at startup, native emits `define_data`; a comptime-built table threaded through the
  program; give the field list the program's static lifetime via a new allocation scheme.
- Why: ADR-0075 §3 and ADR-0078 §4 framed this as an open decision, and probing shows it is not: a
  **string literal already has exactly this problem and both engines already solve it** — `jr-vm`'s
  `intern_strings` allocates every `Item::StrValue` into VM memory before execution, and
  `jr-codegen-clif` emits a `DataDescription`/`define_data`. A `Type_Info`'s `name` field is already a
  `string`, so a field list of `{name: string, type_id: s64, offset: s64}` needs *no new lifetime story* —
  it is the same static data one level up. A bespoke comptime table would be a second mechanism for a
  problem the first already covers, which is the duplication ADR-0018 §2 and ADR-0055 §1 both argue
  against.

### Fork 3 — the field list's shape

- Options: **a `[]Type_Info_Field` view on `Type_Info`, elements `{name, type_id, offset}` (taken,
  recommended)**; a linked list through the fields; a flat parallel-arrays encoding.
- Why: a view is `{data, count}` (ADR-0044), which the language already has and both engines already lay
  out; a program iterates it with the `for` it already has. `offset` is included because it is what makes
  the list *usable* with `Any` — a printer needs the byte offset to reach a field's value through
  `Any.data`. `type_id` rather than a nested `*Type_Info`, consistent with ADR-0078's `element`: an id
  needs nothing further built.

### Resolution (ADR-0079 records it)

Taken as recommended.

### Fork 4 — a silent miscompile found while probing Fork 2

- Found by probing whether a constant aggregate can hold a view (the field list's premise):
  `H :: struct { p: *s64; n: s64; }` returned from `#run` interns the **pointer** field as a plain 8-byte
  integer — `reduce_element` treats `PointerType`/`ViewType` in its scalar arm. `V.p.*` then returns
  **48** in the VM and **segfaults** natively: a wrong answer, two different wrong answers, and no
  diagnostic. The project's named failure mode (a legitimate-looking value standing in for something
  unrepresentable), and the corpus differential is blind to it because no corpus file holds a pointer in
  a constant aggregate.
- Options: **refuse a pointer/view element in a compile-time aggregate (taken, recommended)**; try to
  relocate the pointee into interned data; leave it and document.
- Why refuse: a compile-time pointer has no meaning at run time — the address is the VM's, and ADR-0074 §2
  already refuses `string` as an *aggregate* element for exactly this reason ("its runtime form is a
  pointer, which has no compile-time value"). The same argument covers a raw pointer and a view, and the
  existing code simply did not extend it to them. Relocating a pointee is the static-data decision, and
  doing it implicitly would silently change what the program pointed *at*. Leaving it is not an option: a
  wrong answer with no diagnostic is what ADR-0017 §4 says must refuse.
- This lands **before** the field list, because the field list would otherwise be built on a path that
  silently miscompiles pointers.

### Fork 5 — the field list's representation, after ADR-0079 closed the view route

- What probing established: a **fixed array** of structs each holding a `string`
  (`[2]F where F :: struct { name: string; off: s64; }`) interns and round-trips in both engines, because
  every element's identity is its *contents*. A **view** cannot be a constant at all (ADR-0079). And a
  `Type_Info` needs a *per-type* field count, which a single fixed `[N]` cannot express.
- Options: **defer the field list and record the sharpened constraint (taken, recommended)**; a fixed
  max-N array plus a count in every `Type_Info`; implement the static-data relocation ADR-0079 §1
  rejected doing implicitly.
- Why defer: the max-N option makes **every** `Type_Info` pay N field slots regardless of the type it
  describes — a `s64`'s info would carry (say) 32 empty `Type_Info_Field`s, and N would be an arbitrary
  cap that a struct can exceed, so it trades a silent truncation for a size cost. That is a worse answer
  than not shipping it. The relocation option is a genuine, separable decision — it needs a declared
  mechanism (ADR-0079 §1 refused doing it quietly because it changes what a program points at), which
  means new syntax or a new back-end contract for emitting per-type static data. That is its own wave, and
  it is honest to say so rather than pick the cheap encoding.
- What ships instead: nothing for the list; the constraint is recorded in PLAN §7 and ADR-0079's
  consequences so the next attempt starts from "a view cannot be a constant, and a fixed array cannot vary
  per type" rather than rediscovering it.
- **W4's remaining scope is therefore `#code` and the cross-file `#run` value**, with the field list
  explicitly out of W4 and owed its own wave.

### Fork 6 — `#code`, assessed before committing to it

- What probing established: `#insert CODE;` where `CODE :: "n := 7;"` **already works** (ADR-0073), and a
  malformed operand already reports well — E0263 names the parse fault *and* its offset into the inserted
  text. So `#code`'s marginal value over a string operand is **syntactic**: unquoted source, checked at the
  quote site rather than at splice time.
- Options: **implement `#code` as unquoted syntax that lowers to the same string operand path (taken,
  recommended)**; a full quoted-syntax-tree value (`Item::CodeValue` holding a CST/HIR fragment); leave
  `#code` unimplemented and close W4 without it.
- Why the string-backed form: it delivers the *ergonomic* win (no quoting, no escaping — and escaping is
  what ADR-0072 §5 said bounds a written nest) with no new pool variant, no new comptime-only type, and no
  new engine path, because it reuses the `#insert` machinery three sub-waves already built and tested. A
  full quoted **syntax tree** value answers ADR-0072 §4's "does it exist at run time?" with "no", which
  makes it comptime-only like `Item::TypeType` — and then a `Code` value can only ever be spliced, which is
  exactly what the string form already does. Paying for a CST-in-the-pool representation to reach the same
  observable behaviour is cost without benefit until something *manipulates* a tree (a macro that inspects
  its argument), which nothing in W4 asks for.
- Consequence recorded honestly: this makes `#code` sugar, and the ADR must say so rather than implying a
  syntax-tree value. If a later wave needs tree *manipulation*, it supersedes this with the real
  representation.

---

## Wave: W5 Polymorphism, sub-wave 1 — a single `$T` parameter (ADR-0081), 2026-08-04

Recommended option taken automatically per the standing request; logged for later review.

### Fork 1 — the first sub-wave's scope

- Options: **one `$T` parameter inferred from the call, instantiated, monomorphic body (taken,
  recommended)**; parse-and-refuse (`$T` lexes and lowers, calling is refused until a later sub-wave);
  the whole of `$T` + `$$T` + multiple type params at once.
- Why the single-`$T` slice: it delivers a *real capability* end to end — `id :: (x: $T) -> T` runs in
  both engines — which is the bar the refused-body lesson sets (a refusal must name something refused by
  design, not merely unimplemented; "polymorphism arrives later" is exactly the "arrives in wave W1"
  pattern the early waves spent effort removing). Parse-and-refuse fails that bar. The whole-of-`$T`
  option is a 8–12 week wave in one step, unverifiable the way W4 taught sub-waves to be. One `$T`
  exercises every layer — lex, parse, HIR, the signature/instantiation split, MIR, both engines — which
  is what makes it the honest smallest slice: it forces every architectural decision while keeping each
  small.

### Fork 2 — where instantiation lives in the pipeline

- Options: **at the call, in the check phase, producing a concrete `ProcType` keyed structurally (taken,
  recommended)** — ADR-0005's model; a separate monomorphisation pass after checking; instantiate lazily
  in MIR.
- Why at-the-call-in-check: ADR-0005 fixed the *identity* (structural, on interned comptime-argument
  tuples) but not the *phase*. Checking is where a call's argument types are known and where a mismatch is
  already reported, so inferring `$T` from the argument and interning the concrete `ProcType` there needs
  no new phase and reuses the argument-type machinery `check_call` has. A separate pass would re-walk
  every call; MIR-time instantiation would put type inference in a crate ADR-0017 §4 keeps a pure fold.
- The polymorphic procedure itself gets **no concrete signature** — its `$T` parameter has no type until a
  call supplies one — so its body is checked *per instantiation*, not once. That is the structural change
  this sub-wave introduces and the reason it touches the signature phase.

### Fork 3 — how `$T` binds

- Options: **`$T` in a parameter's type position introduces `T` as a type name in that signature's scope
  (taken, recommended)**, Jai's spelling; a separate `poly` keyword; angle brackets `id<T>`.
- Why `$T`: it is the spelling PLAN §2.1 and every ADR that mentions polymorphism use, and it is
  inference-first — `$T` at a *use* site of a type says "bind `T` from the argument here", so `id(42)`
  needs no explicit type argument. Angle brackets invite explicit type arguments first, which is the
  opposite ergonomic default. The `$` introduces the binding at the first `$T`; a later bare `T` in the
  same signature refers to it.

### Resolution (ADR-0081 records it)

Taken as recommended. Sub-wave 1: a single `$T`, inferred, instantiated structurally at the call in the
check phase, monomorphic body, both engines. Deferred to later W5 sub-waves: `$$T` (comptime-only
parameters), `#modify`, `#bake_arguments`, `#expand` macros, multiple *distinct* type parameters,
polymorphic structs, and instantiation backtraces beyond a single frame.

---

## Wave: W5 Polymorphism, sub-wave 2 — instantiation (ADR-0082), 2026-08-04

Recommended option taken automatically per the standing autonomy directive; logged for later review.

### Fork 1 — how an instantiation gets a procedure identity

- Options: **expand the HIR with instantiated procedures appended, then re-resolve/re-check/lower it,
  reusing ADR-0073's computed-`#insert` machinery (taken, recommended)**; a parallel instantiation table
  the back ends consult; monomorphise in MIR by cloning `MirBody`s.
- Why the expanded-HIR route: the compiler keys everything by `ProcRef = (FileId, ProcId)`, and an
  instantiation is a *new procedure* not in the source `procs` arena. ADR-0073 already solved the
  isomorphic "produce N program elements from one source, then check them like any other" problem by
  building an expanded `FileHir`, re-resolving and re-checking it (`checked_expanded`), and lowering
  *that* — `file_mir` already branches on `expanded`. Appending instantiated `Proc`s to that expanded HIR
  gives each a real `ProcId`, so MIR, the signature phase, both engines and the differential treat it as
  an ordinary procedure with **no new keying**. A parallel table would make every `ProcRef` consumer
  learn about instantiations. Cloning `MirBody`s skips checking, so a body wrong for the instantiated type
  (e.g. `a + b` on a struct) would miscompile rather than be rejected — ADR-0081 §2's "checked per
  instantiation" would be violated.

### Fork 2 — where instantiations are collected

- Options: **in the check phase, recorded like `type_info_calls`, keyed by call expression → (proc, bound
  type) (taken, recommended)**; a dedicated pre-pass over the HIR.
- Why in check: `check_call` already infers the argument types a `$T` binds from, and ADR-0081 §2 put
  instantiation there. It records each polymorphic call's (callee proc, bound type tuple) the way it
  records `type_info_calls`, and the expansion pass reads that set — one type inference, reused, exactly
  as ADR-0075's `type_info_calls` avoided a second walk.

### Fork 3 — de-duplication key

- Options: **the structural key ADR-0005 fixed — the tuple of bound interned type IDs (taken,
  recommended, and mandated by ADR-0005)**.
- Why: not a fresh decision. ADR-0005 fixed instantiation identity as structural on interned
  comptime-argument IDs; this builds it. `id(s64)` reached from two calls or two files interns to one
  instantiation because the bound-type tuple is the same `PoolId` sequence.

### Resolution (ADR-0082 records it)

Taken as recommended. Sub-wave 2: `check_call` infers `$T` and records the instantiation; an expansion
pass appends a substituted `Proc` per distinct structural key and rewrites the call to target it;
`checked_expanded` and `file_mir` check and lower the result. The E0268 refusal is removed — a call now
instantiates. Still deferred: `$$T`, multiple distinct type variables, macros, polymorphic structs.

### Fork 4 — how a call is redirected to its instantiation (refinement while building ADR-0082)

- Building §2 surfaced a cleaner split than "rewrite the HIR call's callee". MIR's `call_rvalue` already
  consults `ConstValues` (keyed by `(scope, ExprId)`) for `#run`/`type_info`/`any_op`. Adding an
  **instantiation-target** entry there — call expr → the instantiated `ProcRef` — lets `call_rvalue`
  redirect the call with no HIR rewrite: the expanded HIR only needs the *appended instantiated procedures*
  (so they get checked and lowered), and the call site is redirected MIR-side.
- Options: **redirect via a `ConstValues` instantiation map (taken, recommended)**; rewrite the call
  node's `Res`/callee in the expanded HIR.
- Why the map: rewriting a call's `Res` means editing resolution results in the cloned HIR and keeping
  every consumer (sema re-check, dump) consistent with the edit; the map leaves the HIR call untouched and
  redirects only where the `ProcRef` is finally chosen — `direct_callee`/`call_rvalue`. It reuses the exact
  channel `#run` and `any_op` already ride, which `scan` and both engines already understand.
- Consequence: the expanded HIR is the original tree **plus** the instantiated procedures appended; no call
  node changes. The instantiations carry their bindings via a `FileHir` side map the signature/check
  phases consult, so a clone need not be substituted in the HIR.

---

## Wave: W5 Polymorphism, sub-wave 3 — multiple type variables (ADR-0083), 2026-08-04

Recommended option taken automatically per the standing autonomy directive; logged for later review.

### Fork 1 — the next W5 increment

- Options: **multiple distinct type variables, `pair :: (a: $A, b: $B)` (taken, recommended)**; `$$T`
  comptime-only params; polymorphic structs; macros.
- Why multiple type variables first: it is the direct **generalisation of the machinery sub-wave 2 built**
  — inference, the structural key, the binding map, the clone — from one variable to N, so it reuses every
  piece and mostly deletes the "exactly one `$T`" guards. It closes ADR-0081 §4's first named gap. `$$T`
  interacts with const-eval (its own can of worms), polymorphic structs need the type-value machinery to
  carry a parameterised type, and macros are a whole family — each strictly larger than lifting the
  one-variable restriction.

### Fork 2 — the structural key with N variables

- Options: **the tuple of all bound types, in the variables' first-seen order (taken, recommended, and
  what ADR-0005 already says)**.
- Why: ADR-0005 fixed the key as "the tuple of resolved comptime-argument IDs" — already plural. Sub-wave 2
  used a one-element tuple as a `PoolId`; this uses the full `Vec<PoolId>`, ordered by the variables'
  first appearance in the signature so the key is deterministic. `pick_first(s64, bool)` and
  `pick_first(s64, s64)` are distinct keys → distinct instantiations, which is correct: the bodies differ.

### Fork 3 — inference sites for N variables

- Options: **each variable inferred from the first parameter that is *directly* `$Var` (taken,
  recommended)**; unify across all positions.
- Why the direct-position rule, extended from sub-wave 2: it is what one variable already did, generalised
  — variable `A` binds from the argument at the first parameter typed exactly `$A`, `B` from the first
  `$B`, and a later bare `A`/`B` is checked against the binding. Full unification (inferring `$T` from
  inside `*$T` or `[]$T`) is a distinct capability ADR-0081 §4 left out and this keeps out, so the slice
  stays "N independent direct variables" rather than "an inference engine".

### Resolution (ADR-0083 records it)

Taken as recommended. Sub-wave 3 lifts the one-variable restriction: `check_polymorphic_call` infers every
variable, forms the `Vec<PoolId>` key, and the instantiation carries N bindings; `expand_instantiations`
and `proc_bindings` generalise to N. Still deferred: `$$T`, polymorphic structs, macros, nested-position
inference.

---

## Wave: W5 Polymorphism, sub-wave 4 — nested-position inference (ADR-0084), 2026-08-04

Recommended option taken automatically per the standing autonomy directive; logged for later review.

### Fork 1 — the next W5 increment

- Options: **nested-position inference, `deref :: (p: *$T)` (taken, recommended)**; `$$T`; polymorphic
  structs; macros.
- Why nested inference: smallest remaining piece, no new representation, and it removes a limitation that
  makes `$T` genuinely partial — a pointer or view parameter is the common polymorphic shape (a `sort`
  takes `[]$T`), and today none of those can be called. It is a targeted extension of the inference
  `check_polymorphic_call` already does: match the parameter's `TypeRef` structure against the argument's
  resolved type structure. The other three are each strictly larger.

### Fork 2 — how a nested binding is inferred

- Options: **structural match of the parameter `TypeRef` against the argument's `PoolId` (taken,
  recommended)**; a general Hindley-Milner unifier.
- Why the structural match: it is exactly as much as the direct case, one layer deeper. `*$T` against a
  `*s64` argument peels the pointer on both sides and binds `T=s64`; `[]$T` against `[]s64` peels the
  view. A parameter shape that does not match the argument shape (`*$T` given a non-pointer) binds nothing
  and is a mismatch, reported by the existing argument check against the re-resolved concrete type. A full
  unifier is a solver with occurs-checks and substitution — far more than one `$T` in one structural
  position needs, and nothing in W5's scope calls for two-way unification.

### Resolution (ADR-0084 records it)

Taken as recommended. `infer_var_in` walks a parameter `TypeRef` and the argument's resolved `PoolId` in
lockstep, binding a `$T` where the structures align (`*$T`↔`*U`, `[]$T`↔`[]U`, direct `$T`↔`U`). The
re-resolution and per-instantiation check are unchanged — only the *inference* reaches one layer deeper.
Still deferred: `$$T`, polymorphic structs, macros.

---

## Wave: W5 Polymorphism, sub-wave 5 — polymorphic structs (ADR-0085), 2026-08-04

Recommended option taken automatically per the standing autonomy directive; logged for later review.

### Fork 1 — the next W5 increment

- Options: **polymorphic structs `Box($T)` (taken, recommended)**; `$$T` comptime value params; macros.
- Why polymorphic structs: the most *foundational* remaining piece — the stdlib's `Array`, hash table and
  bucket array (W7) are all `Struct($T)`, and macros are the largest of the three. It builds on the
  type-value and instantiation machinery already in place. `$$T` is deferred because it interacts with
  const-eval (a comptime *value* parameter, not a type) and no W5 corpus needs it yet.

### Fork 2 — how a parameterised struct instance is identified in the pool

- Options: **a new `Item` variant keyed on `(decl, [type args])` (taken, recommended)**; reuse
  `StructType { decl }` with a side table of args; monomorphise structs into fresh `DeclId`s like the
  procedure clone.
- Why a keyed variant: `Box(s64)` and `Box(bool)` must be *distinct types* from one `DeclId`, and the
  interner keys on the whole `Item`, so putting the type-arg list in the variant makes the pool dedupe and
  distinguish them for free — the same way `ArrayType { elem, len }` distinguishes `[2]s64` from `[3]s64`.
  A side table keyed by `DeclId` cannot hold two instances. Cloning into fresh `DeclId`s (the *procedure*
  approach) would work but a struct's identity is its `DeclId` (ADR-0015 §1), and minting synthetic decls
  for types is a bigger disturbance to nominal identity than a parameterised variant.
- The field types are computed per instance by resolving the declaration's field `TypeRef`s under the
  type-argument bindings — the same substitution-by-binding the procedure instantiation uses, so `Box(s64)`
  and `Box(bool)` get `value: s64` and `value: bool` from one declaration.

### Resolution (ADR-0085 records it)

Taken as recommended. This is the largest remaining W5 piece; built as its own sub-wave with a refusal for
anything beyond a single-`$T` struct used monomorphically, so nothing miscompiles. `$$T` and macros stay
deferred.

---

## Wave: polymorphic structs — implementation (ADR-0085), 2026-08-04

The build of the design ADR-0085 fixed. Staged into two sub-waves because a half-finished type-identity
change is this project's named catastrophic failure mode (a well-typed placeholder that miscompiles).

### Fork 1 — how to stage the identity change

- Options: **sub-wave 5a lands the representation as a zero-behavioral-change refactor, 5b layers grammar +
  instantiation on top (taken, recommended)**; do it all in one commit.
- Why staged: 5a changes the pool's most load-bearing key (a struct's identity) and re-keys the field side
  table across ~40 call sites and ~44 match sites. Proving that state byte-identical (same snapshots, same
  corpus output) isolates the risky identity change from the new *behaviour*. If a snapshot moves in 5a, the
  refactor is wrong and it is visible before any new grammar can hide it. One big commit would tangle "the
  representation changed" with "Box(s64) now works", so a snapshot move could not be attributed.

### Fork 2 — one field map re-keyed, or two maps

- Options: **keep `struct_fields: DeclId→fields` for ordinary structs untouched, add
  `instance_fields: PoolId→fields` for parameterised instances, dispatch in `fields_of` (taken,
  recommended)**; re-key the single map from `DeclId` to instance `PoolId` as ADR-0085 §2 states literally.
- Why two maps: it reaches ADR-0085's stated *consequence* verbatim — "an ordinary struct is unchanged, a
  parameterised one is a generalisation" — while making 5a a genuine zero-diff refactor. Re-keying the one
  map to `PoolId` would touch every `set_struct_fields(decl, …)` and `struct_fields(decl)` caller in 5a
  (sema, sigs, ctx, mir, codegen, vm, lsp) and change what they pass, which is exactly the behaviour-mixing
  fork 1 avoids. The ordinary path stays `DeclId`-keyed and provably identical; the instance path is new
  and reached only once grammar exists (5b), so 5a adds a map nobody writes yet — a dormant generalisation,
  not a speculative half-change, because 5b is the same wave.

### Fork 3 — the surface syntax for a parameterised type reference

- Options: **`Box(s64)` — call-shaped, a name applied to arguments in parentheses (taken, recommended,
  ADR-0085 §3)**; `Box[s64]` (bracket, Rust/Zed-ish); `Box<s64>` (angle, C++/Rust generics).
- Why call-shaped: it is what ADR-0085 §3 fixed, it reuses the type-value view (a type argument *is* an
  interned type, ADR-0071), and it reads as "apply the `Box` constructor to `s64`" — the same mental model
  as a procedure instantiation. Brackets collide with `[N]T` array syntax in type position; angle brackets
  reintroduce the `<` / less-than parsing ambiguity Jai itself avoids. The `(` binds tightly to the name in
  `parse_type_inner`, and a proc-pointer type's leading `(` is a different arm, so there is no ambiguity.

### Fork 4 — one field map re-keyed vs a second instance-keyed map (implementation of 5a fork 2)

- Resolved as fork 2 planned: `Pool::fields_of(ty)` dispatches on whether the `Item` carries arguments —
  ordinary structs stay in the `DeclId`-keyed `struct_fields`, instances land in a new `PoolId`-keyed
  `instance_fields`. Every field-*reading* site (layout, field_offset, sema, mir, codegen, vm, lsp, db)
  moved to `fields_of`; the *writers* for ordinary structs are unchanged. Proven zero-diff before the
  parameterised path had a writer (commit de1c4dd, 969 tests, no snapshot moved).

### Fork 5 — where a parameterised struct's fields are resolved, and the recursion guard

- Options: **resolve per reference, in `resolve_apply`, keyed on the instance `PoolId`, guarding recursion
  by reserving an empty field list before resolving (taken, recommended)**; resolve once at the
  declaration into a template and substitute lazily at each read.
- Why per-reference: it mirrors the procedure instantiation already in place (bind the variables, resolve
  under the bindings) and it puts the substituted fields exactly where `fields_of` looks. The recursion
  guard — `set_instance_fields(instance, vec![])` before resolving the body — is ADR-0015 §1's
  identity-before-fields fixpoint applied per instance, so a future `List($T) { next: *List(T); }` does not
  loop. Lazy substitution at each read would recompute on every field access and would need the bindings
  reconstructed at each site, which is where the two would eventually disagree.

### Fork 6 — what this sub-wave defers (ADR-0085 §5)

- Deferred, each with a by-design no-op arm rather than a half-implementation: inferring a struct's argument
  through a `$T` procedure parameter (`(b: Box($T))` — nested inference, `infer_var_in`/`collect_poly_in_type`
  leave `Apply` unbound); `using` on a parameterised struct (`type_ref_name_in` returns `None` for `Apply`);
  cross-file parameterised structs (E0269 names the limit). Multiple struct type parameters *do* parse and
  lower (`Map($K, $V)`), matching how the corpus exercises them, though the differential corpus file uses
  one variable — the resolution path handles N by construction (zip of vars and args).

---

## Wave: comptime-value parameters `$N: s64` (ADR-0087), 2026-08-04

### Fork 1 — the next W5 increment

- Options: **comptime-value parameters `$N: s64` (taken, recommended)**; the macro family; the deferred
  polymorphic-struct pieces.
- Why `$N`: §7's own recommendation, smaller than macros, reuses the instantiation harness. Its premise was
  checked first (AGENTS.md's rule): `$N: s64` genuinely does not parse today (E0108 "expected a parameter
  name"), so it is a real feature and not already done.

### Fork 2 — stage it, or do the whole feature in one pass

- Options: **6a delivers the surface with the call refused by design, 6b makes it run (taken,
  recommended)**; one pass.
- Why staged: the hard part is evaluating a `$N` argument to a compile-time constant *at the call site*,
  which is the sema↔VM mutual recursion ADR-0073 broke with an acyclic pre-pass — a substantial, risky
  integration. `$T` was staged for the same reason (ADR-0081 surface, ADR-0082 run), and this follows that
  precedent. 6a is a complete, gate-green, non-miscompiling unit: the surface parses and the body checks,
  the call is refused with a by-design code.

### Fork 3 — is a `$N: s64` proc's body checked, like an ordinary proc, or left unchecked like a `$T` template?

- Options: **checked, with `N` typed as its ordinary annotation `s64` (taken, recommended)**; left
  unchecked like a `$T` template.
- Why checked: unlike `$T` — whose *type* is unknown until instantiation, so its body cannot be checked —
  a `$N: s64` parameter's **type is fully known** (`s64`); only its *value* varies. So `N` is a genuine
  `s64` in the body and the body type-checks soundly at template time, catching body errors a sub-wave
  earlier. What is deferred is only instantiation-per-value (and `[N]T`, where `N` must be a compile-time
  constant, which needs 6b's evaluation). MIR for the template is skipped exactly as a `$T` template's is,
  so no runtime artefact treats `N` as an ordinary parameter.

---

## Wave: comptime-value instantiation `$N` (ADR-0088), 2026-08-04 — sub-wave 6b

### Fork 1 — where the argument value comes from

- Options: **a jr-db pre-pass reusing `file_consts`/`insert_operands`, keyed by call span (taken,
  recommended)**; evaluate in the checker; a second evaluator.
- Why the pre-pass: const-eval lives in jr-db over the VM, downstream of check (ADR-0018 §3), so the checker
  *cannot* know a `$N` argument's value — it records the argument expression, and the pre-pass evaluates it,
  exactly as `#insert` does (ADR-0073). Keyed by span because the HIR expansion that follows shifts ids
  ([[jairs-insert-operand-key-by-span]]). A second evaluator would be a second chance to disagree with the
  VM the corpus differential trusts.

### Fork 2 — does the instantiation keep the comptime parameter or drop it

- Options: **drop it from the clone's parameter list and bake its value into the body (taken,
  recommended)**; keep it and bind it like a `$T`.
- Why drop: a comptime parameter has no runtime existence in the instantiation — the caller passes nothing
  for it — so keeping it would make the instantiation's ABI disagree with its call site. Dropping keeps the
  clone an ordinary procedure whose parameter count is its runtime arguments, which both engines already
  handle. The cost, `Res::Param` index shift, is remapped in `instantiate.rs` where the body is already
  deep-copied. Keeping-and-binding is how `$T` works, but a `$T` occupies no runtime slot either — it is a
  *type*, erased before MIR — whereas a `$N` left in the list *would* claim a slot.

### Fork 3 — how a bare `N` in the body lowers

- Options: **substitute the baked constant for a `Res::Param` to a dropped comptime parameter, in MIR
  (taken, recommended)**; rewrite the HIR to replace the name with a literal.
- Why substitute in MIR: the value is a `PoolId` the pre-pass produced, and MIR already substitutes a folded
  value for `type_info` — so a `Res::Param(dropped)` emitting the constant reuses that path. Rewriting the
  HIR would need a literal `Expr` synthesised per use and re-typed, more surface for no gain.

### Resolution (ADR-0088 records the design; implementation deferred)

Taken as recommended for the *design*, but the **implementation is deferred** exactly as ADR-0085's was for
polymorphic structs. Writing it revealed a real gap fork 3 glossed: `instantiated()` re-resolves the
expanded HIR, so dropping the comptime parameter (fork 2) makes the body's bare `N` unresolvable unless the
value is substituted *before* re-resolution — i.e. the HIR-rewrite path fork 3 rejected may actually be
required, contradicting the MIR-substitution choice. Rather than land a half-built pipeline — and in
particular rather than remove ADR-0087's E0271 refusal before the whole thing works, which would make a
comptime call fall through to the MIR-less template — the design is fixed in ADR-0088 and the build is left
as the next sub-wave's own work, starting from the resolved fork above. The 6a surface (E0271 refuses the
call) is unchanged and green.

---

## Wave: `[N]T` where the length is a `$N` comptime parameter (ADR-0089), 2026-08-04 — sub-wave 6c

### Fork 1 — where the baked value is read from when resolving `[N]s64`

- Options: **a `FileHir::param_values: Vec<(ProcId, Symbol, PoolId)>` side table the instantiation fills,
  read by `constant_array_length` exactly as `proc_bindings` is read by the signature phase for `$T`
  (taken, recommended)**; rewrite the `TypeRef::Array`'s `len` during the clone; thread the value through
  `Ctx` as a scoped binding map.
- Why the side table: it is the **exact shape `proc_bindings` already has** for the `$T` case — the clone
  records `(proc, name, value)`, and the signature phase reads it while resolving that proc's types. So the
  mechanism is proven, symmetric with the type side, and needs no new plumbing through `Ctx`. Rewriting the
  `TypeRef` during the clone was rejected because parameter/return `TypeRef`s live in the *shared*
  `FileHir::type_refs` arena and `copy_type_ref` already copies them per instantiation — but a *local's*
  annotation lives in the **body's** arena, and `[N]s64` on a local is the common case, so the rewrite
  would have to happen in two places with two different arena rules. A scoped `Ctx` map is what
  `type_bindings` is for the type side; a *value* map would be a second such map, and the side table keeps
  the value where the instantiation already writes its other per-instantiation facts.

### Fork 2 — does this reopen ADR-0039 §3a's "no const-eval in sema" constraint

- Options: **no — the value is already a `PoolId` the instantiation baked, so sema *reads* it rather than
  computing it (taken, recommended)**; accept a dependency on `jr-db`.
- Why it does not reopen it: ADR-0070 §1 made exactly this move for a file-level constant — sema reads a
  literal already in the HIR rather than evaluating anything. Here it reads a `PoolId` the const-eval
  pre-pass already produced and the instantiation already recorded, so `jr-sema`'s `Cargo.toml` still names
  neither `jr-db` nor `jr-vm`. The value arrives *through the HIR*, which is the same channel
  `proc_bindings` uses for a bound `$T`.

### Fork 3 — what a *template*'s own `[N]T` resolves to

- Options: **a placeholder `[0]T` with length-dependent checks withheld (taken, recommended)**; skip the
  template's body check entirely (as `$T` does); refuse E0233 in the template.
- Why the placeholder: ADR-0087 §2's point is that a `$N` template's body **is** checked (its parameter
  types are known, only values vary), which catches body errors a sub-wave early — skipping the body gives
  that up. Refusing E0233 in the template would be a false error about correct code. So the length resolves
  to a placeholder recorded in `Ctx::placeholder_arrays`, and the checks that read a length (E0236's literal
  index range) withhold on it. Safe because the template is never lowered (`is_template` skips its MIR and
  native declaration), so no code is generated against `[0]T`; each instantiation resolves a real length and
  is checked normally, which is where a genuinely bad index is still caught. **Not** PLAN §5's dangerous
  placeholder: that one reaches code generation, this one reaches only a type that never does.

### Resolution (ADR-0089 records it)

Taken as recommended, shipped, teeth-checked (clearing the bindings makes the instantiation report E0233,
a refusal rather than a wrong length). `$N` is now complete: surface (0087), instantiation (0088), `[N]T`
(0089). No new diagnostic code — E0233 and E0236 are *withheld* in one new case rather than joined.

---

## Wave: `#expand` macros (ADR-0090), 2026-08-04 — sub-wave 7a

### Fork 1 — which macro first

- Options: **`#expand` (taken, recommended)**; `#modify`; `#bake_arguments`.
- Why `#expand`: it is the *core* of Jai's macro family — a procedure whose body is spliced into the
  caller's scope rather than called — and the other two are refinements *of a macro* (`#modify` runs at
  compile time to reject or alter an instantiation; `#bake_arguments` produces a specialised procedure from
  a partial application). Neither is meaningful before a macro exists. `#expand` also composes with the
  `#insert`/`#code` splice already built (ADR-0072/0080), so the mechanism is partly in place.
- Premise verified by running first, per AGENTS.md: `double :: (x: s64) -> s64 #expand { … }` is **E0106**
  today ("expected a procedure body or `#foreign`"), so this is a real feature and not already present.

### Fork 2 — how a macro's body reaches the caller

- Options: **reuse `Stmt::Insert`'s splice — lower the macro's body text into the call site's scope, the
  mechanism ADR-0072/0080 built (taken, recommended)**; a new MIR-level inlining pass; a HIR-level body
  clone like `$T` instantiation.
- Why the splice: ADR-0080 already showed that "unquoted source spliced into the enclosing scope" is what
  `#code` is, and a macro is *that* with arguments bound. The splice's hygiene question (does the macro's
  body see the caller's locals?) is exactly what ADR-0072 §1 answered for `#insert` — the statements land in
  the **enclosing** scope, so they do. A MIR inliner (ADR-0021) already exists but inlines a *call*, which
  keeps the callee's own scope — the opposite of what a macro needs. A HIR body clone is what `$T` does, and
  it also keeps the callee's scope.
- **The hygiene decision this forces**, recorded because it is the interesting one: a Jai macro is
  *deliberately unhygienic* — it can read and modify the caller's locals, which is what makes `#expand`
  useful for things like a custom `for` — and PLAN §2.1 lists "hygiene" as W5 scope. The recommendation is
  to ship the **unhygienic** splice first (matching Jai and matching `#insert`'s existing behaviour) and
  treat any hygiene mechanism as its own later decision, because a hygiene scheme that nothing needs yet
  would be designed against no use case.

### Fork 3 — ship the refusal with the surface, or after it

- Options: **with it (taken, recommended)**; after, as a separate step.
- Why with it: this is not merely staging. With `#expand` parsed and nothing consuming it, a macro was
  **accepted and silently ignored** — `double(21)` returned 42 by ordinary call, with nothing to say it had
  not spliced. That is exactly ADR-0058 §3's "a directive that is silently ignored is worse than one that is
  rejected", and it made the surface-only state a live defect rather than a partial feature. E0272 refuses
  the call, so the sub-wave ships nothing that quietly does the wrong thing.

### Resolution (ADR-0090 records it)

Taken as recommended. The surface ships with E0272; the splice is the next sub-wave and will reuse
`Stmt::Insert` unhygienically (fork 2). Confirmed the lossy-CST trap is still live: **jr-fmt dropped
`#expand` on the first run**, turning every macro into an ordinary procedure, and gate 5 caught it on this
wave's own corpus file. `#modify` and `#bake_arguments` remain unbuilt, each owed its own decision.

---

## Wave: the `#expand` splice (sub-wave 7b) — design recorded, 2026-08-04

Recorded after tracing the mechanism, so the build starts from a settled plan (the ADR-0085/0088 pattern).

### Fork 1 — how the macro's body text reaches the call site

- Options: **a macro-body-text map collected in a pre-scan of the `SourceFile` and threaded to
  `BodyLowerCtx`, exactly as `InsertOperands` is (taken, recommended)**; look the declaration up through
  the CST from the call; store the text on `Proc`.
- Why the map: `lower_file_with_inserts` already holds the whole `SourceFile` AST and already threads one
  such map (`operands`) into every `BodyLowerCtx`, so this is the *same proven shape* — `name → block inner
  text`, collected by walking the file's items for a `#expand` proc and calling the existing
  `block_inner_text`. Walking the CST from the call site would need the call to find its declaration, which
  lowering does not do (resolution is a later pass). Storing text on `Proc` would put source text in the HIR,
  which nothing else there does.

### Fork 2 — how arguments bind

- Options: **synthesize a `name := arg;` prelude per parameter before the spliced body, in the generated
  text (taken, recommended)**; substitute argument expressions into the body text; bind them as HIR locals
  directly.
- Why the prelude: it reuses the splice wholesale — the generated text is `x := <arg text>; <body text>` and
  `expand_insert_text` lowers it in the enclosing scope, so argument evaluation happens **once** (a
  substitution would re-evaluate a side-effecting argument per use, a real wrong answer) and each parameter
  becomes an ordinary local the body's names resolve to. It also keeps the whole feature inside the
  mechanism ADR-0072/0080 built, which is ADR-0090 §2's decision.
- **The argument's text** comes from the call's own CST node, which lowering has in hand.

### Fork 3 — what a macro's `return` means

- Options: **refuse a `return` inside a macro body for now (its own diagnostic), and support the
  value-producing form via a trailing expression later (taken, recommended)**; make `return` return from the
  *caller*; make it produce the splice's value.
- Why refuse first: a spliced `return` returning from the **caller** is what Jai does and is the useful
  semantics, but it changes what `return` means depending on where the text came from — and the corpus's
  `defer`/`return` interaction (ADR-0049 §3) makes that a real risk of a silent wrong exit path. Refusing it
  keeps 7b's scope to the splice itself, which is already the biggest piece, and names the deferral rather
  than half-supporting it. `valid/074`'s macros all `return`, so they will need rewriting or the refusal must
  arrive with a companion form — that is the first thing the 7b build must settle.

### Fork 3 (revised, at the point of building) — a macro's `return`, and expression position

The earlier note recommended refusing `return` inside a macro. Tracing the *call sites* showed that is not
viable as written: `valid/074`'s macros all `return`, and a value-producing macro is the common case
(`double(21)` in expression position needs a value). Refusing `return` would leave `#expand` able to express
only statement-position void macros, which is not the feature.

- Options: **rewrite the splice into a result local — declare `__macro_result_N: T;` before the splice, and
  turn the body's `return <e>;` into `__macro_result_N = <e>;` in the generated text, then use that local as
  the call's value (taken, recommended)**; refuse `return` (rejected above); make a spliced `return` return
  from the caller (Jai's semantics).
- Why the result local: it makes a macro work in **both** positions with one mechanism, keeps the splice
  wholesale (the generated text is still just text handed to `expand_insert_text`), and evaluates each
  argument once via the prelude. Returning from the *caller* is Jai's real semantics and strictly more
  powerful, but it changes what `return` means by provenance and interacts with `defer` (ADR-0049 §3) — so it
  is recorded as the **deferred** generalisation, with a diagnostic when a macro's `return` is not in tail
  position, rather than silently doing the weaker thing.
- **Consequence, stated because it is a real limit:** only a `return` in **tail position** is handled this
  wave. A `return` inside an `if` in a macro body would need the caller-return semantics, so it is refused
  by its own code rather than miscompiled into a fall-through.

### Resolution (ADR-0091 records the build)

Shipped. Three things the build discovered, each recorded because none was in the design:

1. **A macro's own body must not be lowered.** Lowering it standalone resolves its names against the macro's
   own (empty) scope, so a macro reading the caller's locals — the entire point — reported them unresolved.
   It follows that `declarations()` must skip it too: leaving it declared gave the linker
   `function "jr$0$0" with linkage Local must be defined but is not`, caught by the corpus differential.
2. **`looks_like_proc_signature` needed `#expand`** — the token-set trap for the fifth time. A *void* macro
   `f :: (x: s64) #expand { … }` reaches neither `ARROW` nor `L_BRACE`, so it was read as a
   parenthesised-expression constant and produced fourteen cascading errors. That function's own comment
   already warned this had happened for `#c_call`.
3. **A cross-file macro call was reaching the VM as an ICE** (`no routine for file 1 proc 0`) — the fifth
   leaked internal error for a reasonable program. E0272 was repurposed from ADR-0090's pending-splice
   refusal (which the splice lifted) to name it, with `FileSignatures::is_macro` carrying the fact across
   the boundary because an importer has signatures and not HIR.

Fork 3's revised recommendation held: the result local makes a macro work in both positions through one
mechanism, and an early `return` is refused (E0273) rather than rewritten into a fall-through.

---

## Wave: `#modify` (ADR-0092), 2026-08-04 — sub-wave 7c

Premise verified by running first: `#modify { … }` after a signature is **E0106** today, so it is a real
feature. It is the hardest of the three macro pieces, because it runs *arbitrary compile-time code* while an
instantiation is being decided.

### Fork 1 — what `#modify` is allowed to do this sub-wave

- Options: **accept or reject an instantiation — the block returns a `bool`, and `false` refuses the call
  with a diagnostic (taken, recommended)**; also let it *alter* the bound types (Jai's full form, where the
  block may assign to `T`); a `#modify` that only reports.
- Why accept/reject first: it is the half that needs **no new machinery** in the instantiation pipeline. The
  block is a compile-time predicate over the bound types, so it evaluates through the *same* acyclic
  const-eval pre-pass `#insert` and `$N` already use (ADR-0073, ADR-0088) — and a `false` becomes a refusal
  at the call, which is a diagnostic rather than a rewrite. Letting it **alter** `T` would mean the
  instantiation key is no longer what the checker inferred, so `instantiated()` would have to re-run
  inference against the modified binding — a second fixpoint, and its own sub-wave.
- The block's result must be a **compile-time constant `bool`**, judged exactly as a `$N` argument is
  (ADR-0088 §2): a non-constant is refused rather than assumed true.

### Fork 2 — where the block's code lives, and what it can see

- Options: **a body attached to the procedure, evaluated per instantiation with the bound types available as
  compile-time values (taken, recommended)**; a separate top-level predicate procedure the attribute names;
  a textual condition.
- Why an attached body: it is what Jai writes and it keeps the predicate beside the thing it guards. What it
  can *see* this sub-wave is deliberately narrow — it is evaluated per instantiation, so the natural input is
  the bound type, and ADR-0071 already makes a type a compile-time value. A predicate that inspects a type's
  fields needs `type_info` of a *variable* type, which is ADR-0078's deferred variable-length list; so the
  first version supports predicates over the type *identity* (`T == s64`), and richer introspection follows
  that ADR rather than this one.

### Resolution — `#modify` is blocked, and the blocker was worth more (ADR-0092)

Designing `#modify` (forks 1 and 2 above) surfaced that its enabling piece did not exist: **`type_info(T)` on
a bound type variable was E0261**, so a compile-time predicate would have had nothing to predicate on — and,
more importantly, a `$T` procedure could not reflect on its own parameter at all. That is a bigger gap than
`#modify` and on the same critical path, so it was fixed first (ADR-0092): bindings consulted first in
`described_type`, seeded per body in `check_file`, withheld in a template, and an instantiation's `Type_Info`
folded in `file_mir` against its own check — which also turned a sixth leaked ICE ("no routine for file 0
proc 2") into working code.

`#modify` itself remains unbuilt, now genuinely unblocked: a predicate can ask
`type_info(T).id == type_info(s64).id`. Its forks 1 and 2 above stand as the design.

### Resolution (ADR-0093 records the surface; evaluation deferred with its design)

Surface shipped: `#modify { … }` parses (its own kind carrying a **block**), formats with its body, and its
text rides on `Proc::modify`. A call is refused **E0274** — *before* the instantiation is recorded, because
instantiating would mean the predicate was parsed and silently ignored, so a guard that should reject a call
would accept it. ADR-0058 §3's rule for the **third** time (after `#no_abc` and `#expand`).

`looks_like_proc_signature` needed `#modify` — the token-set trap for the **sixth** time.

**Evaluation is designed and deferred** (ADR-0093 §2): the predicate becomes its own appended procedure per
instantiation (body = the block text, returns `bool`, same `proc_bindings` as the instantiation so
`type_info(T)` sees the binding), evaluated as a `#run`-shaped target — **no new query**, since `file_consts`
has that machinery. Attempting it showed why it is its own sub-wave: it needs `FileHir::modify_predicates`
and a way to lower a body *from text* outside `LowerCtx`, which owns the arenas — an API change, and a
half-built version leaves exactly the parsed-and-unevaluated predicate the refusal exists to prevent.

---

## Wave: `#modify` predicate lowering (ADR-0094), 2026-08-04 — sub-wave 7e

### Fork 1 — text re-lowered per instantiation, or lowered once at the template

- Options: **lowered once at the template as a synthetic procedure, then *cloned* per instantiation (taken,
  recommended)**; carry the block's source text and re-lower it per instantiation (ADR-0093 §1's choice).
- Why lowering once: **ADR-0093 §2's stated blocker did not exist.** It said this needed "a way to lower a
  body from text outside `LowerCtx`" — but `lower_body` takes an AST `Block`, and a `#modify` block *is* one,
  so the predicate lowers through the same path every procedure uses. Text was the right shape only while the
  block had to be re-lowered per instantiation; lowering once makes it unnecessary. `Proc::modify` changed
  from `Option<String>` to `Option<ProcId>`.
- **Cloned rather than shared** per instantiation, and not as an optimisation: two instantiations must
  evaluate the predicate against *different* bindings, so one shared procedure would evaluate once and apply
  the answer to both — silently wrong for at least one.

### Fork 2 — how the predicate's `type_info(T)` resolves at the template

- Options: **record the guarded template's `$T` names against the predicate (`FileHir::predicate_vars`) and
  have sema withhold on them (taken, recommended)**; skip checking the predicate's body at the template.
- Why record-and-withhold: a predicate has no `poly_vars` of its own, so `type_info(T)` in it was E0261 —
  the same gap ADR-0092 fixed one level up, and the same withholding answer. Skipping the body would give up
  checking the predicate at all, and a predicate with a type error would then only fail once something
  instantiated it.

### Resolution (ADR-0094 records it)

Shipped. The predicate is a real lowered procedure, cloned per instantiation with its bindings, and excluded
from MIR lowering and native declaration — the **same three exclusions a macro needed** (ADR-0091 §1),
discovered the same way: the linker's "must be defined but is not", then "defined without being declared",
both caught by the corpus differential. What remains is *running* the clone, which needs the expanded tree's
MIR and so a new query — the one thing ADR-0093 §2 sized correctly. E0274 keeps refusing a call meanwhile, so
nothing is silently unguarded.

---

## Wave: `#modify` evaluation (ADR-0095), 2026-08-04 — sub-wave 7f

### Fork 1 — where the predicate runs

- Options: **in `file_mir`, right after the expanded tree is lowered (taken, recommended)**; a new salsa
  query; inside `instantiated()`; inside `file_consts`.
- Why `file_mir`: it is the only place with all three things a predicate needs — the expanded HIR, that
  tree's MIR (just produced), and the VM. `instantiated()` runs before any MIR exists and `file_consts`
  evaluates the *unexpanded* tree, which is exactly what ADR-0094 §3 identified. Rejections ride out on
  `MirResult::expanded_diagnostics`, the channel an instantiation's own diagnostics already take, so this
  needs **no new query and no new plumbing** — better than ADR-0094 §3's estimate.

### Fork 2 — what a predicate that *fails to run* means

- Options: **not a rejection — the instantiation stands and the failure is reported by the ordinary refusal
  path (taken, recommended)**; treat a failure as a rejection; refuse the whole compile.
- Why not a rejection: "the guard could not be evaluated" and "the guard said no" are different findings, and
  only the second is the author's intent. Conflating them would turn a compiler limitation (a trap, an
  unsupported comptime operation) into a false rejection of correct code — the same asymmetry ADR-0071 §3
  argues for its allowlist.

### Resolution (ADR-0095 records it)

Shipped; **`#modify` is complete** and E0274 is retired (the fourth by-design refusal raised then lifted,
after E0268, E0271's first meaning and E0272's first meaning). Two things the build found by running:

1. **A predicate clone's body must be lowered to MIR.** ADR-0094 skipped it in *both* MIR and
   `declarations()` — right for the native back end, wrong for the VM: no MIR means no routine
   (`no routine for file 0 proc 4`). The two exclusions are for different reasons and had to be separated;
   only a *template's own* predicate stays MIR-skipped, since `T` is unbound there.
2. **A predicate takes the hidden context parameter** like every Jairs procedure (`called a procedure taking
   1 arguments with 0`). Its layout is read before the VM borrows the pool — the non-reentrant-mutex order
   `run_main` already uses.

---

## Wave: `#bake_arguments` (ADR-0096), 2026-08-04 — sub-wave 7g, the last of W5's macro family

Premise verified by running: `add_five :: #bake_arguments add(a = 5);` is a parse error today.

### Fork 1 — what a baked procedure *is*

- Options: **a cloned procedure with the baked arguments dropped from its parameter list and their values
  substituted in the body — the *same* mechanism `$N` instantiation uses (taken, recommended)**; a wrapper
  procedure that calls the original with the baked values filled in; a call-site rewrite.
- Why the clone: it is **literally ADR-0088 §3's mechanism**, already built and teeth-checked — drop the
  baked parameters from the clone, rewrite their `Res::Param` name-uses into literals, and remap the
  remaining indices (`append_one` in `instantiate.rs` does all three). A wrapper would work but adds a call
  layer the inliner would then have to remove, and a call-site rewrite would make `add_five` not a value.
- **What this buys:** `#bake_arguments` becomes a *reuse* of the polymorphism machinery rather than a new
  feature, which is why it is the right one to finish W5 with.

### Fork 2 — where the baked value comes from

- Options: **const-eval at the declaration, exactly as a `$N` argument is evaluated (taken, recommended)**;
  require a literal; allow any expression and evaluate at each use.
- Why const-eval: a baked argument is a compile-time constant by definition — the whole point is that the
  specialised procedure has it built in — so it is judged the way a `$N` argument is (ADR-0088 §2, E0271) and
  a non-constant is refused rather than assumed. Requiring a *literal* would be needlessly narrower than `$N`
  already is.

### Fork 3 — where `#bake_arguments` is refused, and what that replaced

- Options: **in lowering, with its own code E0276 (taken, recommended)**; in sema; leave it to fall through.
- Why lowering: that is already where a directive's validity in *expression* position is judged
  (`check_directive_as_expression`). And leaving it to fall through was not neutral — the declaration lowered
  to a poisoned expression and the **caller** reported *"the compiler could not lower `main` … this program is
  legal and this compiler has a gap — please report it"*. That wording is right for an unknown gap and wrong
  for a feature whose absence is known and named, so the refusal replaces a spurious bug report with a
  sentence a reader can act on — the same correction ADR-0069 and ADR-0079 made for leaked *internal* errors,
  here for a leaked *gap report*.

### Resolution (ADR-0096 records the surface)

Surface shipped: the directive parses with a call-shaped operand (the `#insert` arm's shape), reusing the
ordinary named-argument spelling. E0276 refuses the specialisation, which is the **fifth** by-design refusal
raised in this project (after E0268, E0271-first, E0272-first, E0274) — every one has named the sub-wave that
removes it, and four have already been lifted.

Fork 1 stands as the design for the remaining step: the specialised procedure is a **clone with the baked
parameters dropped**, which is literally ADR-0088 §3's `append_one` — drop, substitute, remap — so the last
piece of W5 is a reuse rather than a new mechanism.

### Resolution — the specialisation shipped, and W5 closes (ADR-0097)

Fork 1 held: the specialised procedure is a **clone with the baked parameters dropped**, and it is ADR-0088
§3's three steps (drop, substitute, remap) applied during *lowering* rather than at an instantiation, because a
baked procedure is a **declaration**. W5's last piece is therefore a reuse rather than a new mechanism.

**Fork 2 did not hold, and the correction is the useful part.** It recommended evaluating a baked argument
through ADR-0088 §2's const-eval pre-pass — and building it showed **that pre-pass runs after lowering**,
while the value is needed where the clone is built. So a baked value must be a **literal**, which is the same
narrowing ADR-0039 §3a took for an array length; ADR-0070 §1's widening route (read a literal already in the
HIR from a named constant) is available here later by the same argument.

Also met again: a `NAMED_ARG` is not an `Expr`, so the arguments must be read from the arg list's *children* —
ADR-0053 §1's trap, which that ADR recorded after it silently dropped every named argument.

**W5 — Polymorphism is complete** in fifteen sub-waves, ADR-0081 through ADR-0097. Next: W6 — Metaprogram,
then W7 — Stdlib, whose `Array`/hash table need exactly the polymorphic structs W5 delivered.

---

## Wave: W6 — Metaprogram opens, 2026-08-04

W5 is complete. W6's scope (PLAN §2.1): workspaces, the compiler message loop, `#run build()` build scripts
replacing makefiles, plugin hooks, `@note` attributes.

### Fork 1 — which W6 piece first

- Options: **`@note` attributes (taken, recommended)**; the compiler message loop; `#run build()` build
  scripts; plugin hooks.
- Why `@note` first: it is the **only** W6 piece that is self-contained. A note is metadata a declaration
  carries — `@deprecated`, `@Cleanup` — that a metaprogram can *read*, so it is the thing the other three
  consume. Concretely: the message loop's whole purpose is to hand declarations to a build script, and a
  declaration with nothing extra to say is not worth handing over; a build script's first real job is
  "collect every declaration tagged `@X`". So notes are the data the rest of W6 operates on, and they need
  parse + HIR + reflection surface and nothing else.
- Why **not** the message loop first: it needs a *reason to exist* — a script that can act on what it is
  told — and that reason is either notes (this fork) or the build-script driver. Building the loop first
  would mean designing its message shape against no consumer, the failure ADR-0080 §3 named for a `Code`
  value ("worth representing only once something can inspect it").
- Why **not** `#run build()` first: it needs the workspace notion *and* a way to say "compile these files
  with these settings" — `BuildConfig` exists (ADR-0058 §2) but nothing composes it from a script. It is the
  largest of the four and the natural last.

### Fork 2 — what a note is attached to, and what it carries

- Options: **a declaration, carrying an interned name and an optional string (taken, recommended)**; any
  expression; a name only; arbitrary key-value pairs.
- Why a declaration with name + optional string: Jai's notes are `@name` on a declaration, and the useful
  cases are a bare tag (`@deprecated`) and a tag with a payload (`@requires "x"`). Allowing them on *any*
  expression would raise "what does a note on `a + b` mean", which nothing needs. Arbitrary key-value pairs
  are a superset nothing in W6 consumes yet, and ADR-0080 §3's rule applies: represent it when something
  reads it.

## W6 sub-wave 2 — a reader for `@note`

**The fork: what is the reader?** Four options, and the recommendation is (a).

- **(a) `has_note(f, "x")` and `note_value(f, "x")` — two folding intrinsics over a *named declaration*.**
  A `bool` and a `string`, folded at compile time from `Proc::notes` with **no VM involved**, exactly the way
  `type_info` folds (ADR-0075 §2). Cost: it reads one declaration at a time, so a build script cannot ask
  "every declaration tagged `@X`" without naming each. Benefit: it is the smallest thing that gives notes a
  reader at all, and it needs no new query, no new value shape and no loop — the pool is already mutable in
  sema, and `FileHir::proc.notes` is already there. **Chosen.**
- **(b) A genuine message loop — `compiler_wait_for_message()` returning a `Message` value.** What Jai has,
  and W6's headline. Cost: it needs a `Code`/`Declaration` value that ADR-0080 §3 *declined* to represent
  until something could inspect one, plus a compile-time iteration protocol, plus a re-entrancy story (a
  metaprogram running while the compiler is mid-check). Every one of those is its own sub-wave. Rejected as
  the *next* step, not as a destination: (a) is the inspection primitive (b)'s message would hand over.
- **(c) A callback the compiler invokes per declaration.** Cheaper than (b), but it inverts control before
  anything knows what a declaration *value* is, so the callback's parameter type is the same undecided
  question with less room to change its mind.
- **(d) Emit notes into a generated table a script reads as data.** No new language surface at all, but it
  moves the question to "what file, in what format", which is a build-system decision W6 has not made yet.

**Why a declaration argument rather than a string name.** `has_note(add, "inline")` takes the procedure
*itself*, so a typo in the name is an ordinary unresolved-name error rather than a silent `false`. A silent
`false` is exactly the failure mode the formatter's dropped notes had, and it is worth not rebuilding.

**Deferred with reasons:** notes on a struct or a constant (only `Proc` carries them today — the parser takes
notes in the *procedure* attribute loop, so widening is a parser change, not a reader change); querying every
declaration with a note (needs (b)); a note on a parameter or field.

## W6 sub-wave 3 — iteration over noted declarations

**The blocker, stated first, because it decides the fork.** A folding intrinsic is answered at *check* time,
so every argument must be readable then. A `for` loop's variable is not: it exists only at run time. So

```jai
for i: 0..noted_count("serialise") {  name := noted_name("serialise", i);  … }
```

**cannot** be made to work by folding, no matter how the intrinsic is spelled. Genuine loop-driven iteration
needs the query to lower to *real code* reading a **compiler-emitted table** — static data the back end emits
and the VM can also see. That mechanism does not exist: it is the same one `Type_Info`'s variable-length field
list has been deferred for since ADR-0078, and it is owed its own wave.

Four options, given that:

- **(a) `noted_count(note)` and `noted_name(note, i)` with a *literal* index.** Folds like `has_note`, needs
  nothing new, and is exactly the data a loop would deliver — a script unrolls by hand. Cost: it does not
  scale past a handful, and a script cannot be written once for an unknown number of declarations. Benefit:
  it makes the *query* complete and reduces the message loop to purely the iteration mechanics, which is
  then a wave about static data rather than a wave about notes. **Chosen.**
- **(b) The static-data table now.** The honest full answer, and the right eventual one. Cost: it is a wave —
  a declared static-data mechanism, both back ends emitting it, the VM reading it, and a decision about who
  owns the memory. Doing it *inside* a notes sub-wave would bury an architectural decision in a feature.
- **(c) Return the names as one space-separated `string`, spliced with `#insert`.** Genuinely useful for code
  generation, and it needs no table. But splitting and rebuilding text needs `String`, which is W7 — so it
  would ship a query whose only consumer does not exist yet, ADR-0080 §3's rule again.
- **(d) A `#for_each_note` directive that expands at lowering.** The most powerful and the least honest: it
  would be a second, hidden iteration construct with its own scoping rules, and the language already has
  `for`. A metaprogram facility should not need a parallel `for`.

**What this deliberately does not claim.** After (a), notes can be *counted* and *named*; they cannot be
*looped over*. `PLAN.md` §7 says so in those words, so the message loop's remaining scope is not overstated.

## W6 sub-wave 4 — generating code for *every* noted declaration

**A capability found by probing, not by planning.** `#insert note_value(f, "gen")` **already works**:
`@gen "n = n + 5;"` on a declaration, spliced into a body, runs. Three shapes were checked and all three
worked with no changes — two splices in one body, a splice that calls a procedure, and a splice of an absent
note (empty, quiet). So the *effect* half of a metaprogram — a note driving code generation — was already
there and undocumented, which is exactly the kind of thing PLAN §1.5 is supposed to make visible.

**What is missing is the `for` over it**, and ADR-0100 §2 established that folding can never supply one. But
that argument only forbids a loop *in the program*. It says nothing about looping **inside the fold**.

**The fork: how does a script generate code for each noted declaration?** Four options.

- **(a) `noted_insert(note, template)` — the fold does the loop, `#` stands for each name.**
  `#insert noted_insert("serialise", "write(#);")` folds to `write(a);write(c);`, which `#insert` then splices
  through the mechanism ADR-0073 already built. Cost: one placeholder character to specify, and a template is
  text rather than structure. Benefit: it is the *whole* remaining metaprogram loop for the code-generation
  case, it needs **no table, no `String`, and no run-time iteration**, and every part of it already exists —
  the query (ADR-0100), the fold channel (ADR-0099 §2), and the splice (ADR-0073). **Chosen.**
- **(b) Wait for the static-data table and a real `for`.** The eventual right answer for *inspection*, and it
  is still owed its wave. But for *generation* it is the wrong tool: a run-time loop cannot declare
  procedures or fields, because those are decided at check time. Generation is inherently a fold, so a table
  would not actually deliver this use case.
- **(c) Return the names as one space-separated string and let the script build the code.** Needs `String` to
  split, which is W7 — ADR-0080 §3's rule, a facility whose consumer does not exist.
- **(d) A `#for_each_note name { … }` directive expanding at lowering.** Rejected in ADR-0100 §2 already, and
  the same objection stands: a second, hidden iteration construct with its own scoping rules.

**Why `#` rather than `$name` or `{}`.** A single character that is **not** valid in a Jairs identifier and not
already an operator, so a template containing it is unambiguous. `$` is taken by polymorphism, `{}` reads as a
block, and a word-shaped placeholder could collide with a real name in the generated text.

**Deferred with reasons:** a template referring to a note's *payload* as well as its name (wants two
placeholders and a decision about escaping); generating *declarations* rather than statements (`#insert` at
file scope is refused by ADR-0072 §5, which is a separate decision); a separator other than concatenation.

## W6 sub-wave 5 — `#run build()` build scripts: what a script can *say*

**The claim PLAN §2.1 makes** is that a build script replaces the makefile. What that needs, concretely, is a
way for a compile-time program to **tell the driver** something the driver then acts on — an output name, an
extra module path, a bounds-check setting. Today a `#run` can compute a value and splice code, but nothing it
computes reaches `jr build`.

**The fork: how does a compile-time value reach the driver?** Four options.

- **(a) Declared build options: a `#run`-evaluated constant the driver reads by name.**
  `BUILD_OUTPUT :: #run choose_name();` and `jr build` uses it unless `-o` overrides. The driver already has
  `file_consts`, so reading a named constant's interned value is a query it can make with no new machinery.
  Cost: the option set is fixed by the compiler, so a script cannot invent one. Benefit: every part exists,
  the precedence rule is obvious (an explicit flag wins), and it is genuinely the makefile's job — naming the
  artefact. **Chosen.**
- **(b) An intrinsic the script *calls* — `set_build_output("app")`.** Reads more like Jai's
  `compiler_set_build_options`. But a call has to *happen*, so its effect depends on evaluation order and on
  the script being reached at all; a declared constant is a fact about the file. Order-dependent
  configuration is the failure mode makefiles are notorious for.
- **(c) A whole `Build_Options` struct returned from `#run build()`.** The most Jai-like, and it needs
  `Type_Info`'s field walking to read generically — or a hard-coded field list, which is ADR-0075 §2's
  validated-declaration dance for a much larger struct. Worth doing once there are enough options to justify
  a struct; two is not enough.
- **(d) The driver runs the script as a separate program that prints a manifest.** No language surface at all,
  and it is how many build systems work — but it makes the build a two-phase process with a text protocol
  between the phases, which is a build-system design, not a language feature.

**Why an explicit flag wins over the declared constant.** A person at a terminal is overriding on purpose,
and a build script that could silently defeat `-o` would make the flag untrustworthy. The reverse precedence
would also make a script's own output name unpredictable from the file.

**Deferred with reasons:** a script *adding* a module path (wants a list-valued constant and a decision about
whether it appends or replaces); a script setting `--no-bounds-check` (it is a *safety* setting, and letting a
file silently disable checks for its own build deserves its own argument); plugin hooks; workspaces.

## W7 sub-wave 1 — `String`, and where it lives

**Why `String` is W7's first module.** ADR-0099 §4 refused `==` on two strings because "same storage" and
"same contents" are both plausible, and said comparing contents needs a byte loop — *which is `String`'s job*.
So the previous wave named this module as the fix for a refusal it raised. Nothing else in W7 has a caller
already waiting.

**The fork: a new `modules/String` or more of `modules/Basic`?**

- **(a) A new `modules/String/module.jr`, imported separately.** Cost: a second `#import` line in any program
  that wants it, and `String` cannot use `Basic`'s private helpers. Benefit: a program that only prints does
  not pay for string machinery, the module boundary is where a reader expects it, and it **dogfoods
  cross-module use** — every existing module test imports `Basic` only, so a second module is the first real
  exercise of ADR-0014's flat merge with two modules in play. **Chosen.**
- **(b) Add to `Basic`.** Cheapest, and `Basic` already declares `string`-taking procedures (`print`). But
  `Basic` is the module every program imports, so everything in it is a tax on every program, and it would
  never be tested that two modules can be imported at once.
- **(c) A `#scope_export` section inside `Basic` with a comment saying "string things"** — organisation
  without a boundary, which is the worst of both: the tax stays, and nothing is proven about modules.

**What goes in, and why exactly this much.** The set is decided by *what a refusal or an existing gap asks
for*, not by what a string library usually has:

- `equal(a, b)` — **the reason this module exists** (E0278's help says "compare `.count`, or compare fields
  one at a time"; this is the real answer).
- `compare(a, b)` → `s64` — needed for sorting, which W7 also wants, and it is the same loop as `equal`.
- `starts_with` / `ends_with` — the two most common predicates that are *not* expressible by `equal`.
- `find(haystack, needle)` → `s64` (or `-1`) — the smallest search that is not a predicate.
- `byte_at(s, i)` — because `s.data[i]` **does not work** (a `*u8` is not indexable, E0234), so reading a byte
  currently takes `(s.data + i).*` and a cast. A named accessor is the honest fix until pointer indexing
  arrives.
- `is_empty` — trivially `count == 0`, and included because it reads at the call site and costs nothing.

**Deliberately out:** anything that **allocates** (`concat`, `substring`, `to_upper`, `split`). Those need an
allocator argument and a decision about who frees, and `context.allocator` exists (ADR-0057) — but choosing
between "always the context allocator", "an explicit parameter", and "temporary storage" is its own decision
with real consequences for a caller. A **non-allocating** module is a complete, useful thing on its own, and
shipping it first means the allocation decision is made with a working baseline to compare against.

## W7 sub-wave 2 — `Sort`, and how the ordering is supplied

**What was probed first, because the whole module depends on it.** Three things had to be true and all three
are: a `[]T` **view parameter is mutable** through the callee (so an in-place sort is expressible), a `$T`
parameter **infers through a view** (`xs: []$T`, ADR-0084), and a **procedure pointer** can be passed and
called (ADR-0059). Writing the module without checking those would have been guessing.

**The fork: how does a caller say what "less than" means?**

- **(a) A comparison *procedure pointer* parameter: `sort(xs, less)`.** Cost: every call names a comparison,
  even for `s64`. Benefit: it is the only form that works for **both** a scalar and a struct without the
  language having anything it does not have — no operator overloading resolution at a polymorphic call site, no
  trait system, no `#modify` predicate deciding which body to use. And it composes with `String.compare`, which
  W7 sub-wave 1 shaped for exactly this. **Chosen**, with `sort_ints` as a named convenience so the common case
  is one word.
- **(b) Require `<` on the element type and use it directly.** Reads best (`sort(xs)`), and `operator <` exists
  (ADR-0048). But resolving an *operator* inside a `$T` template against the instantiated type is a lookup
  polymorphic instantiation does not do today — it would be a real feature (call it "operator-bounded
  polymorphism") and it belongs to whichever wave decides how a template states its requirements. `#modify`
  can *reject* an instantiation (ADR-0095) but cannot *select* an implementation.
- **(c) A `Comparable` interface/trait.** The language has no such construct, and inventing one for `Sort`
  would be deciding the whole generic-bounds question inside a library.
- **(d) Sort only `s64` and duplicate the module per type.** No new language surface, and no polymorphism at
  all — which would waste exactly what W5 was built for.

**Which algorithm, and why it is not a performance argument.** **Insertion sort**, with a comment saying so.
It is `O(n²)`, and the honest reasons to choose it here are: it is **stable** (equal elements keep their order,
which a caller can rely on and which quicksort does not give), it needs **no extra storage** (a merge sort
does, and allocation is exactly what ADR-0103 §3 declined to decide), and it is **short enough to read**, which
matters for the first sorting routine in a language whose test suite compares two engines. A faster algorithm
is a later decision with a benchmark behind it, not a guess now — W8 owns performance.

**Deferred with reasons:** a stable merge sort (wants allocation); `sort` returning whether it changed
anything (no caller); binary search (wants a sorted-ness precondition nothing can check); sorting by a *key*
extractor rather than a comparison (two forms where one suffices today).

## W7 sub-wave 3 — a growable array, and the blocker that decides its shape

**Probed first, and one probe failed — which is the whole design input.**

Working: a **polymorphic struct with an array field** (`items: [4]T`) and a `count`; a **pointer to a
parameterised instance** (`b: *Fixed(s64)`) mutating it; `malloc`/`free`.

**Not** working, and each is a *documented* deferral rather than a surprise:

- **`cast(*s64, p)` is refused (E0232, ADR-0045 §1)**, so a `malloc`'d region cannot be *typed*. That kills a
  genuinely heap-backed array: `data: *T` can be declared, but nothing can produce a `*T` from an allocator
  that returns `*u8`. The refusal is right — a general pointer cast makes a wrong pointee type a silent wrong
  read — so the fix is a **typed allocation** primitive, not a weaker cast, and that is its own decision.
- **`b: *Fixed($T)` is E0212** — inference *through* a parameterised struct, deferred in ADR-0085 §5. So every
  routine has to name a concrete instance (`*Fixed(s64)`), which means the module is **per element type**
  today.

**The fork: what ships now?**

- **(a) A fixed-capacity array (`Array` with `[N]T` storage), per element type, with `$N` capacity.** Cost:
  it cannot grow, so it is not the dynamic array W7's list names. Benefit: everything it needs works *today*,
  it is genuinely useful (a bounded buffer is most of what a compiler's own data structures are), and it
  makes the two blockers **concrete and demonstrated** rather than predicted — the module's own docs point at
  the exact refusals. **Chosen.**
- **(b) Wait for typed allocation and cross-instance inference.** Both are real decisions worth making
  properly; neither is a W7 *library* decision, and blocking the whole stdlib on two language features would
  leave W7 empty while they are debated.
- **(c) A `*u8`-backed array with hand-computed byte offsets.** Expressible today — pointer arithmetic works —
  but every read needs the element size as a literal and every write reinterprets bytes, which is exactly the
  "silent wrong read" ADR-0045 §1 refused a cast to prevent. Shipping it would route around a deliberate
  refusal in the standard library, which is the worst possible place to do that.
- **(d) Hard-code an `Array_s64` with no polymorphism.** No new language surface, but it abandons the
  polymorphic struct W5 built, and would need copying per type forever.

**Why `$N` for the capacity.** `Array(s64, 16)` would be the natural spelling, but a parameterised struct takes
*type* arguments only. `$N` on the **procedures** does not help either — the capacity has to be in the
*struct*'s type. So capacity is a compile-time constant in the declaration, and a caller who wants a different
one declares their own struct. That is a real limitation and the corpus file says so.

**Deferred with reasons:** growth (wants typed allocation); `remove_at` preserving order (trivial once `pop`
exists, and no caller yet); a `[]T` view *of* the used prefix (wants slicing a struct field, worth checking
separately); iteration via `for` (wants the view).

## W7 sub-wave 4 — typed allocation, the first unblocker `Array` named

**What ADR-0105 established.** Heap storage is unreachable because `malloc` returns `*u8` and `cast(*s64, p)`
is E0232: a general pointer cast makes a wrong pointee type a *silent wrong read* (ADR-0045 §1). That refusal
is correct and should stay. What is missing is a way to get a **typed** pointer to fresh memory *without* a
general cast.

**The fork: what shape does typed allocation take?**

- **(a) An intrinsic `alloc(T, n)` returning `*T`.** The compiler knows the element type and its size, so it
  can compute `n * size_of(T)` itself and hand back a `*T` with **no cast anywhere** — the unsound conversion
  never appears in a program or in a library. Cost: it is compiler-known rather than library-defined, so it
  bypasses `context.allocator` unless it is taught to consult it. Benefit: it is the only form where the
  *type* comes from the language rather than from a caller's assertion, which is exactly what E0232 refuses to
  let a caller assert. **Chosen**, allocating through `context.allocator` so ADR-0057's installed allocator
  still governs.
- **(b) Relax `cast` for `*u8` → `*T` only.** Smallest change, and it is the wrong one: a `*u8` may point at
  anything, so the relaxation permits exactly the wrong-pointee read ADR-0045 §1 refused. The narrowness of the
  hole does not change what goes through it.
- **(c) A `#foreign`-style `typed_malloc :: ($T: Type, n: s64) -> *T`** written in `Basic`. Attractive because
  it keeps the standard library in charge — but a `Type`-taking parameter returning a *pointer to that type* is
  a dependent return type, which the signature phase does not have. It would be a bigger language feature than
  (a).
- **(d) `Any`-based allocation** — allocate, wrap in an `Any`, read back with `any_as`. Every piece exists
  (ADR-0076), and it costs a runtime type check per access plus a pointer indirection, for a facility whose
  entire purpose is being the fast path. Wrong tool.

**Amended while building: (a) split in two.** MIR has **no way to reach `malloc`** — a `#foreign` procedure is
resolved by name in *its own file's* signatures, and the builder has no channel for "call this library
procedure I invented". So an intrinsic that allocates *and* types would have to synthesise a cross-file call
from nothing.

The split that follows is better than the original anyway: **the library allocates, and only the *retyping* is
an intrinsic.** `size_of(T)` gives a caller the byte count, `Basic.malloc` returns `*u8` as it already does,
and `typed(T, p)` converts that `*u8` to a `*T`. The unsound *general* cast stays refused; what is permitted is
one intrinsic whose target type comes from the language, at a boundary — which is exactly the shape ADR-0076 §1
used for `Any` and for the same reason. It also means `size_of` — asked for by nothing until now — arrives with
a caller.

**Why `free` needs no new form.** `free` takes a `*u8` and a pointer to any type can already be *passed* where
`*u8` is expected only at an `Any` boundary (ADR-0076 §1) — so releasing needs the *reverse* conversion.
`untyped(p)` is that: `*T` → `*u8`, which `Basic.free` already accepts. Symmetric with `typed`, and the same
three lines, rather than leaving a caller unable to release what they allocated.

**Deferred with reasons:** reallocation (`realloc` wants a size the caller must remember, and a growable array
is the caller that will ask for it — so it belongs with the dynamic array, one sub-wave later); alignment beyond
the type's own (`malloc` guarantees suitable alignment for any scalar, and an over-aligned request has no
caller); allocating an *aggregate* whose fields need initialisation (a zeroing decision of its own).

## W7 sub-wave 5 — a genuinely growable array

**What typed allocation unblocked, probed and confirmed.** A struct holding `data: *s64` allocated via
`typed(s64, malloc(n * size_of(s64)))`; growth by **allocate, copy, free**; indexing through pointer
arithmetic. All three work in both engines.

**The fork: how does growth report failure, and how much does it grow by?**

*Failure.* `malloc` can return null, so `push` can fail for a reason that is nothing to do with the caller.

- **(a) `push` keeps returning `bool`, now meaning "allocation failed" rather than "full".** No signature
  change from `Int_Array`, and a caller who already handles `false` handles this. Cost: the two reasons are
  conflated — but a caller can do nothing different about either, and `is_full` is gone since the array is no
  longer bounded. **Chosen.**
- **(b) Trap on allocation failure.** Jai's default posture, and wrong here: ADR-0058 §4's line is that a trap
  is for a *program* error, and running out of memory is not one. A library that aborts the process removes the
  caller's only chance to recover.
- **(c) Return `(bool, Error)`.** There is exactly one failure mode; an enum with one variant is a worse `bool`.

*Growth factor.* **Doubling**, from a capacity of 4 on first push.

- Doubling makes `n` pushes cost `O(n)` amortised, which is the property that makes a growable array worth
  having at all — a fixed increment makes it `O(n²)` and would be a bug disguised as a policy.
- Starting at 4 rather than 1 avoids three reallocations for the common small array, and rather than 16 because
  an array that stays small should not hold 128 unused bytes.

**`free_data` rather than `clear`, and why the module is honest about ownership.** `Int_Array.clear` was a
one-liner because its storage was inline. Heap storage must be **released explicitly**: there are no
destructors (a design value, ADR-0008's neighbours), so a caller who allocates must free. The module names the
routine `free_data` rather than `clear` so a reader cannot mistake it for "forget the elements" — and `clear`
survives as the count-only operation, since resetting to reuse the buffer is a real and different thing to want.

**Deferred with reasons:** `realloc` (the platform's may extend in place; using it would make `grow` depend on
an allocator behaviour the VM does not model, so the two engines could diverge in *timing* — not in results,
but a difference worth not introducing casually); shrinking on `pop` (a caller who pops then pushes would
thrash, and nothing asks for the memory back); a `[]T` view of the used prefix (wants a view built from a
pointer and a count, which no expression can spell — a real gap, and its own decision).

## W7 sub-wave 6 — reporting an imported module's own diagnostics

**The gap, as ADR-0107 §5 recorded it.** `file_diagnostics(root)` reports **one file**. A root whose *imported
module* is broken therefore checks clean and fails at run time — `List` calling `malloc` without importing
`Basic` gave `no routine for file 2 proc 0` for a program `jr check` had just approved. Resolution is correct;
the *reporting* is not.

**The fork: where does the extra reporting happen, and whose diagnostics are they?**

- **(a) The CLI reports every reachable file, each attributed to itself.** `jr check`/`run`/`build` already
  compute the reachable set (`reachable_files`, used to assemble MIR), so this is one loop over an existing
  list, and every diagnostic keeps its own file and span — a reader is told the module's line, which is where
  the fix goes. Cost: `jr check foo.jr` can now report errors in a file the user did not name. Benefit: that is
  *correct* — the errors are real, and the alternative is a program that fails at run time. **Chosen.**
- **(b) A new `program_diagnostics` query in `jr-db` that folds every reachable file's.** Same result, and the
  right shape eventually — but it adds a salsa query whose only consumers are the three CLI commands that
  already have the reachable set, and it would make `file_diagnostics`'s meaning ("this file") ambiguous by
  proximity.
- **(c) Attribute a module's errors to the `#import` line in the root.** Reads as "your import is broken", which
  is what a *user of a third-party module* wants — but it discards the module's own span, so a person who can
  fix the module (which, for the standard library, is us) loses the line number. A diagnostic that is true and
  useless is ADR-0043's lesson.
- **(d) Refuse to compile a module with errors, with a new code.** A new refusal for something already
  diagnosed — the module's own E0201 *is* the diagnostic, and a second one saying "that module had an error"
  would be noise.

**Deduplication matters and is nearly free.** The reachable set includes the root, and two roots may share a
module, so the loop must not report the same file twice. Reachability is already a seen-set walk, so the list is
distinct by construction — but `jr check a.jr b.jr` checks several roots in one run, so the *command* dedupes by
file across roots.

**Why a warning stays a warning.** An unused import in a module (E0231) is reported too, and it is still a
warning: a module's diagnostics are reported *as they are*, not re-graded by distance. Re-grading would mean the
same code meant different things depending on which file you compiled.

**Deferred with a reason:** ordering. Diagnostics come out root-first then per reachable file in load order,
which is deterministic but not source-ordered across files. Sorting by file and line would be nicer and needs a
decision about whether the root's own errors should still come first (they should, and that is the argument for
leaving it).

## W7 sub-wave 7 — a view of a list's used prefix

**The gap, as ADR-0107 named it.** `Int_List` cannot hand its contents to `Sort` or `String`, because building a
`[]s64` from a pointer and a count is not something any expression can spell — a slice takes an *array*. So a
growable array and a sorting routine exist side by side and cannot be combined, which is a poor advertisement for
a standard library.

**A stale reason found while probing.** ADR-0044 §4 refused `view.data` because it "would hand out an unbounded
`*T` one wave after the bounds check was added, and there is no pointer arithmetic to use it with." **Both halves
have expired**: pointer arithmetic arrived in ADR-0064, and typed allocation in ADR-0106 means a `*T` is now an
ordinary thing to hold. A refusal whose stated reason has expired is worth revisiting rather than inheriting.

**The fork: how does a `[]T` come into existence from a pointer and a count?**

- **(a) An intrinsic `view(p, count)` returning `[]T` where `T` is `p`'s pointee.** The element type comes from
  the *pointer*, so nothing is asserted — the same property that made `typed` acceptable while `cast` stayed
  refused (ADR-0106 §1). A view is `{data, count}` (ADR-0044), so lowering is building a two-field aggregate,
  which MIR already does for `Any` and for a string literal. Cost: the count is unchecked, so `view(p, 99)` on a
  three-element allocation is a lie the compiler cannot catch. Benefit: it is the only form that needs no new
  type rule, and *every* alternative has the same unchecked count. **Chosen.**
- **(b) Expose `view.data` and let a caller build views by struct literal.** Needs a view to be constructible as
  an aggregate literal, which would make `[]T` a nominal-ish struct rather than a built-in — a much larger change
  — and it hands out the `*T` ADR-0044 §4 worried about *without* giving the caller the thing they actually
  wanted.
- **(c) A slice syntax over a pointer: `p[0 .. n]`.** Prettier, and it is what a later wave should have. But
  slicing is currently defined over arrays with a *known* bound (ADR-0044), so this would either weaken that
  definition or introduce a second slicing rule keyed on the base's type. Syntax is the expensive part to get
  wrong and the cheap part to add later; an intrinsic can be replaced by syntax without changing semantics.
- **(d) Give `List` its own `sort_list`/`find_in_list` wrappers instead.** No language change, and it multiplies:
  every algorithm would need a per-container copy, which is exactly what a view exists to prevent.

**Why the count stays unchecked, stated rather than hidden.** The pointer's *allocation size* is not tracked
anywhere — `malloc` returns a bare address, and no shadow table records what was asked for. So a checked
`view` would need an allocation registry, which is a much bigger decision (and one the native back end could not
share with the VM). The intrinsic is therefore in the same honest category as `typed`: it does not make the
operation safe, it makes it **visible and searchable**.

**Deferred with reasons:** `p[0 .. n]` syntax (above); a view of a *sub*range of a list; a checked view (wants an
allocation registry); `view` on a `*u8` producing `[]u8` is allowed and is how a caller would build a byte view.

## W7 sub-wave 8 — the allocating half of `String`, and the convention it needs

**The decision ADR-0103 §3 deferred, verbatim:** `concat`, `substring`, `to_upper` and `split` each need somewhere
to put a result, and the *mechanism* is not missing — `context.allocator` is a real protocol (ADR-0062: two
procedure pointers and a state word) and `talloc` is a real arena (ADR-0065). What was missing was the **choice**
between "always the context allocator", "an explicit parameter", and "always temporary".

**Probed first:** `context.allocator` is reachable from a callee and travels with the call (ADR-0057 §2), and
`talloc` reads the context so a callee's `talloc` uses its caller's arena. Both hold.

**The fork.**

- **(a) Always `context.allocator`, and the caller frees.** The result is an ordinary allocation a caller releases
  with `context.allocator_free`. Cost: every caller must free, and a forgotten free leaks — but that is already
  true of `Int_List` and is the honest cost of explicit memory in a language with no destructors. Benefit: the
  allocator is **installable**, so a caller who wants arena behaviour installs an arena and gets it for every
  `String` routine at once, with no second API. That is what a context is *for* (ADR-0001), and it means the
  module needs no allocator parameter and no second spelling. **Chosen.**
- **(b) Always temporary storage (`talloc`).** No caller ever frees, which is genuinely pleasant — and it makes
  every result *silently invalid* after `reset_temporary_storage()`, with nothing to warn a caller who kept one.
  A library whose returns expire on an unrelated call is a trap. It is also strictly less capable: a caller who
  wants this installs `talloc` as the context allocator and gets exactly it.
- **(c) An explicit allocator parameter on every routine.** Most explicit, and it doubles every signature for a
  choice a caller almost always makes once. The context exists to carry precisely this.
- **(d) Return a `[]u8` the caller supplies (`concat_into(dest, a, b)`).** No allocation at all and no ownership
  question — genuinely the right shape for a *hot* path, and it is additive later. But it cannot be the only form:
  a caller who does not know the length in advance would have to compute it separately, which is `concat`'s job.

**What ships:** `concat`, `substring`, `to_upper`, `to_lower`, `free_string`. Each returns a `string` allocated
through `context.allocator`, and `free_string` releases one — named so a reader cannot mistake it for anything
else, and symmetric because a facility that can allocate and not free leaks by construction (ADR-0106's argument
for `untyped`).

**A failed allocation returns `""`**, not a trap: ADR-0058 §4's line is that a trap is for a *program* error, and
running out of memory is not one. An empty result is distinguishable from every non-empty one, and a caller
concatenating two empty strings has nothing to detect.

**Deferred with reasons:** `split` (wants a container of strings — an `Int_List` of what? a list of strings needs
`List($T)`, which cross-file parameterised structs still block); `concat_into` (option (d), additive, no caller
yet); `trim` (wants a definition of whitespace, which is a table, not a decision).

## W7 sub-wave 9 — the allocating half of String, delivered

ADR-0111 records the convention chosen (sub-wave 8's fork, now built): `concat`, `substring`, `to_upper`,
`to_lower` allocate through `context.allocator` and the caller frees with `free_string`. `talloc`-always and an
explicit-parameter form were both rejected in that fork; this sub-wave is the implementation. `split` stays
deferred (wants a list of strings, which needs `List($T)`, which cross-file parameterised structs still block).

## W7 sub-wave 10 — `Math`, and the FFI-float refusal that shapes it

**Probed first, and it changed the whole plan.** The obvious `Math` wraps libm: `sqrt`, `sin`, `pow` are
`#foreign` declarations. But **a float cannot cross the FFI boundary yet** — `sqrt :: (x: float64) -> float64
#foreign libc "sqrt"` is refused, "passing FloatType to a foreign procedure arrives with a later wave". So a
libm-wrapping `Math` is not writable, and the module must be **pure Jairs**.

**The fork: what is a pure-Jairs `Math`?**

- **(a) The exact, closed-form functions — `abs`, `min`, `max`, `sign`, `clamp`, `floor`/`ceil`/`round` on a
  `float64`, integer `pow`, `gcd`.** Every one is expressible with arithmetic and comparison the language
  already has, exactly and deterministically, so both engines agree bit-for-bit. Cost: no `sqrt`, `sin`, `log` —
  the transcendentals a `Math` is often reached for. Benefit: what it *does* have is correct, and correctness is
  the only thing a differential-tested library can offer. **Chosen.**
- **(b) Approximate the transcendentals in Jairs (a Taylor/CORDIC `sqrt`).** Writable, and *wrong* in a specific
  way: an approximation's last bits depend on the evaluation order, and the comptime VM and native Cranelift may
  round a fused multiply-add differently — so the two engines could disagree on the last ulp, which is the one
  thing this project's harness treats as a failure. A transcendental belongs behind the FFI boundary (libm is
  correctly rounded) or behind a decision about ulp tolerance, and neither is a W7 library call.
- **(c) Wait for FFI floats.** That is a real language sub-wave and it unblocks the libm wrap — but it blocks
  *all* of `Math` on it, when half of `Math` needs nothing. Ship the exact half now.

**Why `floor`/`ceil`/`round` are in and `sqrt` is out**, since both are "float functions": `floor` is exact and
closed-form (truncate toward zero via an integer cast, adjust by sign), so both engines compute the same bits.
`sqrt` is not expressible exactly without a loop whose rounding the two engines need not share. The line is
**exactness**, not difficulty.

**Deferred with reasons:** the transcendentals (`sqrt`, `sin`, `cos`, `log`, `exp` — want FFI floats or a ulp
decision); `is_nan`/`is_inf` (want bit inspection of a float, which is `transmute`, deferred); a `float32` set
(the same functions, and additive once the `float64` set proves the shape).

## W7 sub-wave 11 — `Random`, and where the state lives

**Probed first:** `u64` xorshift arithmetic (`^`, `<<`, `>>`, `%`) agrees bit-for-bit between the two engines,
which a random generator's whole value depends on — a PRNG that differed between engines would fail the harness
on its first call.

**The fork: where does the generator's state live?**

- **(a) An explicit `Random` struct the caller threads: `next(*rng)`.** The state is a value the caller owns and
  passes by pointer, so a sequence is reproducible from its seed and two generators are independent. Cost: the
  caller declares and threads it. Benefit: it is the only form that is **deterministic and testable** — a
  differential harness needs the same seed to give the same sequence in both engines, which a hidden global
  makes awkward and a time-seeded one makes impossible. **Chosen.**
- **(b) A hidden global generator, `random()` with no argument.** Convenient, and untestable: a global's state
  is shared across a whole program, so a test cannot get a clean sequence, and seeding it from the clock (the
  usual reason for a global) makes every run different — the opposite of what a differential harness needs.
- **(c) The generator in `context`, like the allocator.** Defensible — it travels with the call — but the
  context is for things a *callee* needs without being handed them (an allocator, a logger), and a random
  sequence is usually something a caller owns deliberately. It also makes two independent sequences in one scope
  impossible. A caller who wants context-carried randomness can put a `*Random` in their own context struct.

**xorshift64, not a better generator.** It is **exact, tiny, and deterministic** — three lines of shift-and-xor,
which is what a differential-tested library needs, since every bit is reproducible and both engines compute the
same one. A higher-quality generator (PCG, xoshiro) is a later decision with a statistical-quality argument
behind it; xorshift64 is the one whose correctness is *obvious*, and obvious correctness is what a standard
library's first generator should have.

**A zero seed is replaced by a constant**, because xorshift is stuck at zero — `0` xor-shifts to `0` forever. So
`seed(0)` silently becomes `seed(GOLDEN)`, which is a defined non-degenerate sequence rather than a stream of
zeros a caller would take a while to notice.

**Deferred with reasons:** a float in `[0, 1)` (wants a float from a `u64`'s bits, which is a `transmute` or a
divide, and the divide is exact so it is additive later); a better generator (statistical-quality decision);
seeding from the clock (wants a time source, and is the *non*-deterministic thing the explicit struct exists to
avoid making the default).

## W7 sub-wave 12 — floats across the FFI boundary

**Why now.** Two sub-waves named this as their unblocker: `Math` (ADR-0112) cannot wrap libm's `sqrt`/`sin`
without it, and any future numeric FFI needs it. The refusal is explicit — "passing FloatType to a foreign
procedure arrives with a later wave" — so this is that wave.

**What it takes, both engines.** A float is passed in a **floating-point register**, not an integer one, on
both the SysV (x86-64) and AAPCS (arm64) ABIs — so it is not enough to pass the bits as a `u64`. The VM's
libffi path must describe a `float`/`double` argument and return with `Type::f32`/`Type::f64` (libffi then
places it correctly), and the native path must give a `#foreign` procedure's Cranelift signature an
`F32`/`F64` `AbiParam` rather than an integer one.

**The fork was small, because the representation already exists.** A float's bits already live in
`Value::Scalar(u64)` (the VM stores everything as bits), and `FloatKind::decode`/`encode` already convert. So:

- **(a) Teach `marshal`/`return` the float case, keyed on `FloatKind::of`.** The bits are in hand; libffi's
  `arg` takes an `f32`/`f64`, so decode the stored bits to the host float, hand it over, and re-encode the
  return. Native adds the `AbiParam` width. **Chosen** — it is the whole change, and it reuses `FloatKind`.
- **(b) Pass the bits as an integer and let the callee reinterpret.** Wrong on every real ABI: `sqrt` reads its
  argument from `xmm0`, not `rdi`, so passing the bits in an integer register calls `sqrt` on uninitialised
  float state. The bug would be silent — a plausible-looking wrong number — which is the worst kind.

**A `float32` narrows at the boundary.** libffi's `float` is 32-bit, and a Jairs `float32` holds its value in
the low 32 bits of the `u64` (ADR-0040 §3), so `marshal` decodes with the *parameter's* `FloatKind`, not a
blanket `f64`. Getting this wrong would pass a 64-bit pattern where 32 bits were expected — caught by keying on
the declared parameter type.

**Scope, kept honest:** this passes and returns a float; it does not add libm to `Basic` (that is a set of
`#foreign` declarations, a separate additive change) nor lift `Math`'s transcendentals (they can now be a libm
wrap, which is `Math`'s next sub-wave). What ships is the *capability* and a corpus file that calls `sqrt` and
checks it in both engines.

**Deferred:** an aggregate-of-floats by value (the struct-ABI decision, still deferred for integers too); a
`float` *variadic* argument (`printf("%f")` promotes `float` to `double`, its own rule).

## W7 sub-wave 14 — a hash table (`Int_Map`)

**Probed first:** a heap array of *structs* (`typed(Slot, malloc(n * size_of(Slot)))`), with field access through
pointer arithmetic (`(slots + i).key`), works in both engines. That is everything a hash table's storage needs.

**Concrete `Int_Map`, for the reason `Int_Array` and `Int_List` are concrete** (ADR-0105, ADR-0107): cross-file
parameterised structs are deferred (E0269, ADR-0085 §5), so a `Map($K, $V)` in a module would be unusable by
every importer. `s64 -> s64` is the useful concrete instance, and the name says so.

**The fork: open addressing or chaining?**

- **(a) Open addressing, linear probing, in one heap array.** One allocation, no per-entry allocation, and the
  probe sequence is simple arithmetic — so both engines walk it identically, which the differential harness
  needs. Cost: deletion needs a tombstone or a backshift, and a full table must grow. Benefit: it is the layout
  whose behaviour is *obvious* and whose storage is one `typed` allocation, exactly what the language makes easy
  today. **Chosen.**
- **(b) Separate chaining (a linked list per bucket).** Needs a per-node allocation and a `List`-like pointer
  chase, which is more allocation and more to get wrong, for no benefit at these sizes.

**Load-factor growth at 3/4**, doubling, rehashing every live entry — the same amortised-`O(1)` argument as
`List`'s doubling. A linear-probing table degrades sharply past 3/4 full, so growing there rather than at 1 is
correctness-adjacent, not just speed.

**Deletion by tombstone**, not backshift: a backshift is correct but fiddly to get right under a probe sequence,
and a tombstone (a slot marked deleted-but-was-used, which a probe skips over but an insert may reuse) is the
textbook simple answer. Tombstones accumulate, which a rehash-on-grow clears — stated, not hidden.

**The hash is `Basic`-free arithmetic**: a `u64` multiply by an odd constant and a xor-shift (a Fibonacci-style
mix), so it agrees bit-for-bit between engines and needs no FFI. A key of any `s64` maps to a bucket, including
negative keys (cast to `u64` first, so the sign bit participates rather than breaking the modulo).

**Deferred:** a generic `Map($K, $V)` (cross-file parameterised structs); a string-keyed map (wants the key
hash to walk bytes, additive); iteration (wants a view or a cursor, its own shape).

## W7 sub-wave 16 — converting the containers to generic structs

**What ADR-0117 unblocked, and what it did not.** A `struct($T)` in a module now works for an importer. But
**inference through a parameterised struct is still deferred** (ADR-0085 §5): `push :: (a: *Array($T), v: T)` is
E0212, because `T` is not in scope there. So the *struct* can be generic while the *procedures* cannot.

**Probed:** a module declaring `Holder($T)` and exporting `push_int :: (a: *Holder(s64), v: s64)` works from an
importer — struct generic, procedure concrete.

**The fork: convert now, or wait for inference?**

- **(a) Convert the structs to `struct($T)` and keep the procedures concrete, one set per element type.** The
  storage declaration is written **once** instead of per type, so a second element type needs procedures but not
  a new struct — genuine progress, and it puts the modules in the shape the eventual inference lift will complete
  rather than a shape it will have to undo. Cost: the procedure names keep an `_int` flavour, and the honest
  reading is "half converted". **Chosen**, with the module docs saying which half and why.
- **(b) Wait for inference through a parameterised struct.** Then `push` is generic too and the conversion is
  one step. But that is another language sub-wave, and leaving three modules declaring storage they no longer
  need to declare concretely is leaving the language's new capability unused in the very place that asked for it.
- **(c) Convert and add `$T`-inferring procedures anyway.** They do not compile. Not an option, listed because
  it is the obvious wrong turn: the refusal is real, not a lint.

**Deliberately scoped to `Array` first**, not all three at once. `List` and `Map` own heap memory and their
`grow` paths are where a conversion could go subtly wrong; `Array`'s storage is inline, so it is the one where a
mistake is visible immediately. If `Array` converts cleanly the other two follow in the same sub-wave; if it does
not, the ADR says so and they stay concrete.

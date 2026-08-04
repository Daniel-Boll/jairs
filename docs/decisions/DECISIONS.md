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

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

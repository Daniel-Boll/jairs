# ADR-0180: The target operating system as a compile-time value

- **Status:** Accepted
- **Date:** 2026-09-02
- **Deciders:** dboll
- **Group B of the Simp-shaped-graphics plan.** The graphics restructure needs a portable library, and a
  library cannot be portable if it cannot ask which platform it is on. Before this, the compiler had **no
  notion of an operating system anywhere** — not in the type system, not in the salsa inputs, not in a
  build flag.
- No design fork was put to the decider beyond the plan's; the rejected alternatives are recorded at their
  point of decision.

## Context

### The gap, and where the library recorded it

There is no `#if`, no `OS` constant, and no build setting a module can read. The only directives the parser
accepts at top level are `#import`, `#run`, `#scope_module` and `#scope_export`. The compiler's **whole
notion of a target** is `TargetLayout { pointer_size, pointer_align }` — two numbers, one value, `LP64`.

The consequence was written down in the library rather than in the compiler.
`modules/Time`'s `CLOCK_MONOTONIC` was `6`, macOS's number, under this comment:

> **This is a real portability gap and it is named rather than hidden**: the value below is macOS's, because
> that is the only target this project has ever run on. A Linux build needs 1, and the day the CI matrix
> runs is the day this needs a `#if`-shaped answer — which this language does not have either.

### Why not conditional compilation

Two forms were considered and both rejected.

**Item-level `#if`** changes the item tree — which declarations exist — and ADR-0072 §5 deferred that
deliberately. Nothing in the plan needs it: every case in this library is *"this number differs"*, not
*"this declaration exists on one platform only"*. A mechanism that reshapes the item tree, added for cases
that only need a value, is the largest possible answer to the smallest question.

**A per-OS library *name*** — `gl :: #system_library OS_LIB;` — is not merely unimplemented but
**circular**. Library resolution happens inside `file_signatures`, and `file_consts` *depends on*
`file_signatures`: so a computed `#system_library` operand cannot be evaluated before the library must
already be known. That is why the graphics work is built on SDL2, whose link name is `-lSDL2` on all three
targets, rather than on OpenGL, which is `OpenGL.framework` / `GL` / `opengl32`.

## Decision

### §1 — The enum is declared in Jairs, not in the compiler

`Basic.Operating_System`, with `MACOS`, `LINUX` and `WINDOWS`. In `modules/Basic` for exactly the reason
ADR-0075 §2 put `Type_Info` there: a caller has to be able to **name** the type to store the value, and a
type the compiler owns and no source file can spell is unusable.

An enum rather than an integer, so a `switch` over it is exhaustiveness-checked (ADR-0067) — which makes
"this program does not handle Windows" a compile error instead of a silently wrong branch. Nothing reads
the members by ordinal: the compiler resolves the one it wants **by name**, the way `jr-db`'s
`type_info_kind_name` does, so a member inserted mid-list renumbers the others harmlessly.

A fourth host is therefore a library edit plus one arm in `TargetOs`, which is the point of the
declaration living where it does.

### §2 — `os()` is an intrinsic folded in sema, and the value is a compiler constant

`os()` takes no arguments — the only intrinsic that does — and its type is the library enum, looked up by
name and validated as an enum (`library_enum`, the counterpart to `library_struct`). It is folded with
`record_fold`, the mechanism `size_of` uses, so the value is a constant by the time const-eval reads it and
**neither back end ever sees a call**.

**The value is `jr_pool::TargetOs::host()`, a `cfg!`-derived constant, not a `BuildConfig` field — and this
overrules the plan.** The plan called for a salsa input, citing ADR-0058 §2's reason for `bounds_checks`:
configuration from outside the source files must invalidate every query that read it. The reason does not
transfer, and the cost was measured:

- **Nothing can change it within a process.** There is no `--target` flag, `jr-link` shells out to the
  host's `cc` and emits only `-L` and `-l`, and `jr-vm` resolves a foreign symbol out of the compiler's own
  process image. A cross-compile is not unimplemented; it has **no path through the driver**. Invalidation
  for a value that cannot change buys nothing.
- **The cost is a fourth query parameter on `file_signatures`, which has ≈50 call sites across six
  crates** — `jr-db`, `jr-lsp`, `jr-mir`, `jr-sema`, and two test harnesses. `BuildConfig` reaches
  `optimized_file_mir` as a *query parameter*, so there is no way for sema to read it without one.

So `TargetOs` lives beside `TargetLayout` in `jr-pool`, which is where the compiler's notion of a target
already was, and `TargetOs::host()`'s own documentation records that the salsa input is owed **the day a
`--target` flag exists** — which is the same day the host and the target stop being the same thing.

An unrecognised host is `Linux` rather than a panic: every remaining `target_os` this compiler could
plausibly be built for is a Unix, and a compiler that refuses to start on a platform nobody has tried is
worse than one that guesses the common case and is corrected by a member and an arm.

### §3 — The signature phase's folds were computed and thrown away

`HERE :: os();` at file scope was E0230, *"a name failed to resolve at file scope"*, reported against the
**callee** — because `jr-mir`'s thunk found no value for the call and then resolved `os` as a name, which it
is not.

**The plan named the wrong cause, and the right one is one field rather than one arm.** It said
`crates/jr-mir/src/build.rs:2551` consults `consts.run` for a call in a body while
`crates/jr-mir/src/thunk.rs:403` never does at file scope. That is true and it is not sufficient: making
the thunk consult the channel changed nothing, because **nothing had put a value in it**.

The real cause is which phase types what. `check_file`'s own comment says it:

> Unnamed items. A named item's initialiser was typed by the signature phase; a top-level `#run` has no
> name and so has no signature.

So the fold inside `HERE :: os();` happens in the **signature** phase — and `SignatureOutput` had no
`folded_calls` field. The value was computed and dropped. `CheckOutput` has carried one since `has_note`
needed it (ADR-0099 §2), and `file_consts` already copied *that* map into the `run` channel; the signature
phase's needed the same three lines.

Both halves landed: the thunk now consults the channel (it must, or the value would be recorded and
unread), and the signature phase now fills it.

**This closed a gap two library modules had documented and worked around.** `size_of` of a struct in a
file-scope constant was E0230 for the same reason, so `Window` and `Socket` had both moved a layout
assertion into a *procedure* and said so in a comment. `Window.LAYOUT_IS_SDL2` and
`Image.SURFACE_LAYOUT_IS_SDL2` are constants now, which is the test that the gap is really closed.

**And `file_consts`'s early-out needed a fourth entry.** That condition is a list of features — a `#run`, a
`type_info`, a fold, an `any_of`, a `pointer_view`, an atomic — and **nothing enforces it**; AGENTS.md
records it biting three times, always as "a name failed to resolve" on an obviously fine program. It did
**not** bite for `os()`, because `os()` reuses `size_of`'s existing `folded_calls` channel, which was
already in the list. It bit for the *signature phase's* folds, which are a new map. Fourth entry, third
distinct reason, still nothing enforcing it.

### §4 — A second cause, in resolution, found by probing the neighbour

With §3 fixed, `HERE :: os();` worked and `N :: size_of(s64);` still did not — `error[E0201]: unresolved
name s64`. A **different** cause, and the more interesting one.

`resolve_all` walks the top-level expression arena **flat**, by index. A body's expressions are reached from
its statements, so an intrinsic's type argument is only ever visited *through* the call — with
`in_type_info_argument` already set, which is what withholds E0201 for a builtin type name (the builtin
names are ordinary identifiers, not keywords). The flat walk has no statements to start from, so it visited
`s64` as an expression in its own right, reported the error, and only *afterwards* reached the call and
re-resolved it correctly. **The resolve map ended up right and the diagnostic was already pushed.**

Fixed by *skipping* the subtrees an intrinsic call's arguments span, rather than by widening the E0201
suppression. Nothing is left unresolved — the call resolves them — and a builtin name written anywhere else
at file scope keeps its error, which is the asymmetry ADR-0071 §3 argues for: a missed legal position is a
visible false error, a missed illegal one is silent.

The marking is **transitive**, matching `in_type_info_argument`'s stickiness for ADR-0119 §2's reason:
`size_of(Slot(s64, s64))` has a call as its argument and every name below it is a type.

**Worth generalising: a phase whose walk order differs from another phase's will disagree about context,
and the disagreement shows up as a diagnostic rather than as a wrong answer.** `size_of` in a body and
`size_of` at file scope were the same code reached two ways.

### §5 — Two silent `#system_library` holes, closed with E0293

Both type-checked clean and emitted **no `-l`**, so the symbol failed at *link* time — `ld: symbol not
found` — with nothing pointing at the declaration that caused it:

1. `x :: #library "SDL2";` — `foreign_library_of` compares the directive's *name* against
   `"system_library"` and returns `None` for anything else, silently.
2. `x :: #system_library;` — `check_directive` accepted `arg: None` while the same resolver bailed on
   exactly that.

One code, **E0293**, two messages, raised in `check_directive` — which is where both are visible, where the
span is, and where the *declaration* is. At the declaration rather than at a `#foreign` that names it: a
`#library` nobody calls is still wrong, and reporting per use would say it once per binding.

**E0293, not the E0294 the plan allocated.** The plan assumed Group A would spend both E0292 and E0293; it
spent only E0292, because the second refusal it drafted had no reachable condition (ADR-0179 §4).
`jr-cli`'s `codes.rs` is what makes the "first free code" claim checkable, and it is what caught this.

### §6 — One more diagnostic that fired on poison

`switch os()` in the `jr-sema` corpus harness — which runs deliberately **without** module resolution, so
`Operating_System` cannot be found and `os()` is `PoolId::ERROR` — reported E0244, *"the enum a bare `.`
member belongs to cannot be inferred here"*, three times.

The cause was `let want = (scrutinee != PoolId::ERROR).then_some(scrutinee);`, presumably written to avoid
an E0214 mismatch against `ERROR`. But `expect` is already silent for `ERROR`, and `check_bare_member` has
an explicit `ERROR` guard — so passing `None` was the one input that **routed around** the guard the
poison rule already had. One mistake, two diagnostics, and the second misdirects: nothing is wrong with the
arm.

Found by the harness whose own contract says *"sema must stay silent about them rather than inventing type
errors on poison"*. It is the second time that harness has caught a real defect by running a corpus file
with no imports.

## Consequences

- A library module can select a per-OS value. ADR-0181 is the first one that does.
- An intrinsic works at file scope, in a constant and in a `#run` alike. Two library modules' documented
  workarounds are gone.
- **`os()` is a value, not a conditional.** A declaration that exists on one platform only is still not
  expressible, and nothing in this plan needs one. When something does, it is item-level `#if` and its own
  wave.
- **A per-OS *library name* is still out of reach**, and the reason is the query-order cycle in §"Why not
  conditional compilation" rather than effort. Any plan that wants one has to break that cycle first.
- The MIR corpus snapshot churns, because `modules/Basic` gaining a declaration shifts the `PoolId`s a
  `type_info(T).id` prints. That is the same churn any edit to `Basic` causes and the same shape AGENTS.md
  warns about for `FileId`; nothing here made it worse.

## Verification

- **`tests/corpus/valid/134-target-os.jr` exits 249** on macOS, and its low bits *name the host* — 1, 2 or
  4 — so a reader of a failure on another platform knows what the total should have been. Seven independent
  bits: `os()` in a body; `os()` through a file-scope `#run`; `os()` **directly** in a file-scope constant
  (§3); `size_of` of a builtin in one (§4); `size_of` of a struct in one; a `#run`'s answer agreeing with a
  `switch`'s; and the `switch` itself, which only compiles because it is exhaustive. Both engines agree.
- **`Window.LAYOUT_IS_SDL2` and `Image.SURFACE_LAYOUT_IS_SDL2` are constants**, and the two integration
  tests that read them are unchanged apart from dropping the call parentheses — an unchanged assertion over
  a construct that could not previously exist.
- **`tests/corpus/type-errors/082-…` and `083-…` each report E0293.** Split into two files rather than one,
  because that directory's harness asserts a file reports *exactly* the codes it declares, once — met by
  splitting the file, not by weakening the rule.
- All seven gates green; `jr fmt --check` over every corpus directory and `modules/`; tree-sitter
  regenerated and the whole corpus parsed with no `ERROR` node.

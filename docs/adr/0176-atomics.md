# ADR-0176: Atomics as language operations — and the passes that must not touch them

- **Status:** Accepted
- **Date:** 2026-09-02
- **Deciders:** dboll
- **W11's second half**, and the one §8.3 named correctly: *atomics as language operations*.
- No design fork was put to the decider; the recommended approach was taken and the rejected
  alternatives are recorded below, per the session directive.

## Decision

### 1. A MIR `Rvalue::Atomic`, not a library call

**Rejected: `#foreign` calls to libatomic**, which is the cheapest option and would have needed no compiler
change at all. It fails for two independent reasons, and the second is the decisive one:

- A libatomic call is opaque to the mid-end, so it would be *conservatively correct* — and it would also be a
  real function call per increment, which is not what a counter is for.
- **The comptime VM cannot make a `#foreign` call to a procedure it has no address for** and would therefore
  refuse every program using one (ADR-0175 §4). An atomic in a `#run` is single-threaded and has exactly one
  right answer; a library call would make it unrunnable.

**Rejected: operators** — `a atomic+= 1`, or making `+=` atomic on some marked type. An operator makes the
*ordering* invisible at the call site: `a += 1` and `atomic_add(*a, 1)` mean very different things to another
thread, and someone auditing a concurrent program must be able to find every synchronising operation by
searching for it.

So: four intrinsics, one MIR variant, three engine implementations.

**An `Rvalue` and not a `Statement`**, because three of the four *produce* a value. A store yields `void`,
which is a storable value here (ADR-0015 §3), so making it the odd one out would cost every consumer a second
arm and buy nothing.

### 2. No pass may move, duplicate or elide one — and the type system was made to ask

**This is the whole reason atomics are a MIR variant.** Every mid-end pass here was written for a
single-threaded program, and each was wrong about an atomic in its own way:

| Pass | What it would have done |
|---|---|
| `forward_stores` | forwarded a store *across* one, reordering a plain write past a synchronisation point |
| `constprop` | folded a load of a location another thread writes |
| `dce` | deleted a compare-exchange whose result nobody reads — deleting the lock, keeping the critical section |
| `inline` | (correct, but refused today by `is_inlinable`, so translated faithfully rather than panicked on) |

The exhaustive-match rule made every one of these a **compile error** at the site that had to decide, which is
precisely the argument `AGENTS.md` gives for banning `_` arms. Nine sites had to answer, and each answer is a
line of reasoning rather than a `{}`.

**Renaming an atomic's operands is allowed** and necessary — it is bookkeeping, and refusing it would leave a
dangling value id whenever const-prop renamed one. The distinction that matters is *move, duplicate, elide*
versus *rename*.

**The MIR verifier checks the shapes** — pointer operand, the right operand set per operation, the right result
type — because a wrong lowering would otherwise hand an engine a `bool` where it wants an address and produce a
wrong *store* rather than a type error.

### 3. Four operations, `s64` only, sequentially consistent

`atomic_load`, `atomic_store`, `atomic_add`, `atomic_compare_exchange`.

**Not fewer**: a counter needs `Add`, a flag needs `Load`/`Store`, a lock needs `CompareExchange`. **Not more**:
`and`/`or`/`xor`/`min` are mechanical once these work and have no caller yet.

**`atomic_add` yields the value *before*** the addition, which makes it a ticket dispenser — two threads adding
one each get distinct numbers. Returning the new value makes that use impossible to write correctly.

**`atomic_compare_exchange` is the strong form and yields a `bool`.** A weak version fails spuriously, which is
a trap for a caller who does not expect it. The value it *found* is deliberately not returned: a caller who
wants it can `atomic_load`, and a two-result intrinsic makes the common case pay for the rare one.

**`s64` only, and E0291 says so.** A width parameter means deciding what an atomic `u8` means on a machine whose
smallest atomic is a word, and whether a `*Point` may be exchanged. Both are real decisions; this wave makes
neither and the diagnostic tells a caller the boundary rather than letting them find it.

**Sequentially consistent, with no way to ask for less.** Offering `relaxed` before the memory model is written
down would be selling a guarantee nobody had described. §5 records what *is* guaranteed.

### 4. The interpreter implements them non-atomically, which is correct

Nothing in the comptime VM can spawn a thread (ADR-0175 §4), so there is no concurrency to be atomic against
and the plain read-modify-write **is** the sequentially consistent answer.

**Rejected: refusing them in the VM.** A `#run` computing a value with an `atomic_add` is single-threaded and
has one right answer, and refusing it would make the corpus differential unable to cover atomics at all — so
the three engines' *evaluation* would never be compared.

`atomic_add` **wraps** rather than trapping, matching the machine instruction: an atomic add is one instruction
with no overflow check, so trapping in the interpreter would make it disagree with both back ends about a legal
program. `valid/132` asserts the wrap.

**A store writes `Value::Void`, not nothing.** Leaving the destination register alone left it
`Value::Undefined`, and the next read trapped with "read a value that was never assigned" on a program whose
store had succeeded.

### 5. What is guaranteed, stated plainly

- Every operation is **sequentially consistent**: `MemFlags::trusted()` in Cranelift, `SequentiallyConsistent`
  in LLVM, and the interpreter is single-threaded.
- An atomic is **never moved, duplicated or removed** by any pass here (§2).
- Everything else is unspecified. A plain (non-atomic) read racing a write is a data race with no defined
  outcome, exactly as in C — and this project has *measured* it: the same three-thread program with
  `shared.* = shared.* + 1` produced **1000 instead of 3000** on one run of three. Two thousand increments
  lost, no diagnostic.

That measurement is the reason this section exists rather than a paragraph promising to write one later.

### 6. The `file_consts` early-out is a feature list nothing enforces — third time

An atomic's callee resolves to nothing, so `scan` refuses the body unless told the call is an intrinsic. The
telling goes through `ConstValues`, and `file_consts` **returns early with an empty one** when a file has no
`#run`, no `type_info`, no folds, no `any_of` and no `pointer_views`.

The comment directly above that condition records this exact trap being hit once before, ending *"Found by
running the feature's own probe."* The list was not extended, so it happened again — same symptom ("a name
failed to resolve" on an obviously fine program), same diagnosis route, same probe.

**Recorded rather than merely fixed**, because the condition is a list of features that every new feature must
be added to and **nothing enforces it.** A reader who adds a fifth intrinsic family will hit this a fourth
time, and the comment now says so.

## Consequences

- **Atomics work in all three engines**, verified by `valid/132-atomics.jr` — eleven assertions across the four
  operations, including a failed compare-exchange that must not write and an add that must wrap.
- **Real concurrency works**: three threads, 3000 atomic increments, exactly 3000 every time, five runs per
  test invocation. The non-atomic version loses increments, which is what makes the test meaningful.
- **1069 tests**, 255 corpus files.
- **E0291 refuses a non-`*s64` atomic**, owned by `jr-sema`; `jr-hir` owns E0290 for `$$` in a return type, and
  `codes.rs` caught the collision when this wave first reached for 0290.
- **`AtomicOp` and `jr-sema`'s wire codes round-trip**, asserted, because `jr-sema` cannot name `jr-mir`'s type
  and two hand-written lists would drift into a wrong operation rather than a compile error.
- **Owed**: wider types, other operations, weaker orderings, and a fence. Each wants a caller first.

# ADR-0155: `Time`, a bucket array, a stable sort — and four polymorphism defects the sort found

- **Status:** Accepted
- **Date:** 2026-09-02
- **Deciders:** dboll
- **PLAN §8.3 steps 1-3 of W7's nine remaining modules**, in the order §8.3 recommended: the two that
  needed nothing new first, then the one whose *policy* had to be decided.
- No design fork was put to the decider; the recommended approach was taken and the rejected
  alternatives are recorded below, per the session directive.

## Context

Three modules were scheduled. Two landed as written. The third — a stable merge sort — did not compile,
and chasing why turned up **four separate defects** in how polymorphic instantiation works, one of which
PLAN's known-defects list had already recorded and three of which were unknown. All four are fixed here.

The wave is therefore mostly a compiler wave wearing a library wave's clothes, which is the outcome
`AGENTS.md`'s habit predicts: *confirm a wave's premise by writing the thing before planning around it*.
The premise "a merge sort needs only an allocation policy" was false. This is the seventh time that habit
has caught a false schedule (ADR-0067 §0, ADR-0070 §0, ADR-0072 §5, ADR-0073 §0, ADR-0075's closing
claim, ADR-0140's dump, and now this).

## Decision

### 1. `Time` — nanoseconds, two clocks, and no formatting

`modules/Time` offers `monotonic()`, `wall()`, and truncating conversions, all in **nanoseconds as an
`s64`**.

One integer unit throughout, so a duration is a *number* and arithmetic on it needs no library. An `s64`
of nanoseconds spans about 292 years either side of the epoch and is **exact**; a `float64` of seconds
loses nanosecond resolution in the 2030s and would make two runs of one benchmark differ in their last
digits for no visible reason.

**Rejected: a `Duration` struct** with a seconds/nanos pair, which is what C's `timespec` is. It cannot be
subtracted without a helper, cannot be compared without another, and its only advantage — range — is one
this unit does not need. **Rejected: milliseconds or microseconds as the base unit**, because a coarser
unit cannot express what a finer one measures and every timing question here is sub-millisecond.

**Both clocks are offered** because using the wrong one is a real and quiet bug. `monotonic` never goes
backwards and is the only correct thing to *measure* with; `wall` is what a *timestamp* wants. A benchmark
built on `wall` reports a nonsense number occasionally rather than always, which is the hardest kind to
notice.

**No formatting, deliberately.** Rendering a timestamp needs a calendar, and a calendar needs leap
seconds, time zones and a locale — none of which this project has decided anything about. A module
offering a *wrong* rendering would be worse than one offering none, and PLAN §2.1's `Time` entry is about
measurement. **No sleeping**: `nanosleep` blocks, and a blocking call in the comptime VM means
compilation that pauses, which is a decision about compile-time execution rather than about time
(ADR-0121 gave comptime a step budget precisely so it cannot run away).

**One portability gap is named rather than hidden.** `CLOCK_MONOTONIC` is 6 on macOS and 1 on Linux, and
the constant carries macOS's value with a comment saying so, because macOS is the only target this project
has ever run on (PLAN §1.5: no CI run has happened). `CLOCK_REALTIME` is 0 on both.

### 2. `Bucket_Array` — a growable sequence whose element addresses never move

`List` doubles its storage and **copies** on growth, so every pointer into it dies at the next `push`.
That is the right trade for a sequence you iterate and the wrong one for a sequence you hold references
into — an entity pointing at its parent, a UI element remembering its child, an interner handing out a
pointer to a stored value. W10 will want the second.

`Bucket_Array` keeps a `[..]` **spine** of fixed-size buckets and only ever appends a bucket. The spine
may move; the buckets may not, and nothing outside the module holds a pointer to the spine. `push` returns
the stable pointer, rather than leaving it to a following `get`, so the promise is visible in the signature.

**The bucket size is a constant, not a parameter.** The only reason to vary it is a measurement this
project cannot take (ADR-0146 §4 measures its own throughput and deliberately not the programs it
compiles), so a parameter would be a knob with no evidence behind it — the same refusal ADR-0104 §3 made
of a "faster" algorithm chosen without a benchmark. Sixteen, because a bucket is one allocation and
sixteen `s64`s is 128 bytes: small enough that a two-element array wastes little, large enough that a
thousand elements is 63 allocations rather than 1000.

**No removal.** The promise is that an address stays valid, and removal has to answer what happens to the
hole: compacting moves elements and breaks the promise, while a tombstone makes every read check one and
stops `get` being pointer arithmetic. Both are real designs; neither is decidable without knowing what the
caller wants, so this offers append and read — which is what the stability promise is *for*.

**Two shapes the language forced, both recorded because they read as arbitrary otherwise.** A `[..]T`
cannot be **indexed** (`only a fixed-size array [N]T and a view []T can be indexed`), so the spine is read
through `view(data, count)` — the same route `List.elements` takes, and the reason `view` exists
(ADR-0109 §1). And a bucket is a **named one-field struct** rather than a bare `*s64`, because `size_of`
and `typed` refuse a *structural* type argument (`size_of(*s64)` is E0261, ADR-0071 §5's still-deferred
structural type argument). The named type reads better anyway: the spine is a list of buckets, not a list
of pointers-to-integers.

The spine's own `push`/free are written **inside** the module rather than imported from `List`, because
`List` operates on `[..]s64` and an imported template cannot be instantiated at a second element type
(E0268, ADR-0104 §5). Thirty concrete lines, which is the shape ADR-0118 used inside `List` itself before
genericity was available across modules.

### 3. `stable_sort` — the arena is the allocation policy

`sort` (insertion) is stable and `O(n²)`; `heap_sort` is `O(n log n)` and **not** stable. So before this
there was no way to sort a large sequence by one key and keep an earlier ordering by another — the
two-pass "sort by name, then stably by department" every table view needs. ADR-0146 discharged ADR-0104
§3's *faster* debt with `heap_sort`, chosen by a comparison count rather than a clock. This is the *stable*
half, and stability is a property rather than a speed, so it needs no benchmark to justify: a caller either
requires it or does not.

**The scratch comes from temporary storage**, making the arena its first real customer (ADR-0065). Three
reasons a caller can check: the scratch is dead the moment the sort returns, which is exactly the lifetime
an arena models; it costs no `free` and cannot leak, so a sort whose comparison traps leaks nothing; and
the arena is in the **context**, so a caller who wants the allocation elsewhere already has the mechanism
— `push_context` with a different arena — and needs nothing from this module.

**Rejected: `malloc`/`free` per call**, which pays an allocator round trip per sort and leaks on a trap.
**Rejected: a caller-supplied buffer** — the fastest option, and this wave wrote it, and then took it out:
it makes every call site carry a parameter that is always the same expression, and the one caller who wants
a different arena can already say so through the context. **Rejected: an in-place merge**, `O(n log² n)`
or rotations — real algorithms, both slower than a scratch buffer for no benefit this project can measure.

**Falls back to `sort` when the arena has no room**, rather than trapping or leaving `xs` unsorted.
Insertion sort needs no scratch, so the fallback is a real sort rather than a degradation — slower on a
large input, and correct, which is the right way round. Both paths are stable, so the *answer* never
depends on whether the arena had room; a program that sorted differently under memory pressure would be
the worst kind of bug to chase.

**Bottom-up over doubling widths, in one procedure.** The textbook recursion would be a second procedure
taking `less`, and every extra procedure taking `less` is another instantiation to get right — which §4
explains is not free. Bottom-up is the standard iterative form, needs no call stack, and the width loop
reads as clearly as the recursion it replaces.

**The comparison is `less(right, left)`, not `less(left, right)`**, and that single choice is what makes
the sort stable: taking the left element unless the right is *strictly* less keeps an equal pair's left
element first, which is the left run, which is the earlier position. Written the other way it is a correct
sort that is not stable — and **no test of sortedness can tell the difference**. Only a test that checks
where equal elements ended up can, which is why `valid/125` sorts by one key and inspects another.

### 4. Four polymorphism defects, all found by writing the sort

Each was a *silent* failure that reached an engine as `no routine for file N proc M` — the eleventh through
fourteenth occurrences of this project's most-recorded shape, and the first four to arrive through a
template's body specifically.

1. **`typed(T, …)` refused a bound type variable while `size_of(T)` accepted one.** `size_of` and
   `type_info` already *withhold* E0261 for an unbound `$T` of the enclosing template (ADR-0092 §1),
   because `size_of(T)` inside a `$T` body is correct code that each instantiation resolves for real.
   `check_typed` did not, so `typed(T, malloc(n * size_of(T)))` — the way a template allocates — was legal
   in one half of the expression and illegal in the other, an asymmetry no caller could predict.
2. **An instantiation's `typed`/`untyped`/`view` calls had no recorded pointer type.** `file_consts` records
   them from the *base* check, where a template's `T` is unbound and the call was withheld — so the clone's
   call had no entry, `scan` refused the clone as "a name failed to resolve" (the `typed` callee names no
   procedure and is exempted from the resolution check *only* by having a recorded view), and the call
   reported no routine when reached. Fixed exactly as ADR-0092 §1 fixed the same problem for
   `type_info_calls`: re-record from the instantiation's check, which is where `T` is bound.
3. **E0268 refused a template calling a template**, even when every variable was bound at the eventual
   instantiation. Inside `stable_sort :: (xs: []$T, …)` the argument `xs` carries the *unbound* `T`, so
   nothing can pin the callee's variable — and the call is nonetheless correct code. Now withheld when the
   body being checked is a template's own copy (variable *names* present, *bindings* absent). A clone has
   both, so a genuinely uninferable call inside an instantiation is still refused, with the instantiation
   backtrace attached — which is where a real mistake shows up.
4. **`check_polymorphic_call` removed a shadowed type binding instead of restoring it.** PLAN's
   known-defects list recorded this one. A clone already has its own `T` bound; a call *inside* it to
   another template whose variable is also spelled `T` shadowed that binding and then **deleted** it, so
   every `size_of(T)` or `typed(T, …)` *after* the inner call reported E0261 inside a clone where `T` was
   bound all along. It stayed latent because the two existing callers happened to put the inner call last.
   Order-dependent invisible breakage is the worst kind, so the fix is a save/restore rather than a rule
   about where to put a call.

`valid/126` exercises 1, 2 and 4 on their own, so a regression names which one broke; `valid/125` needs
all four at once and cannot.

### 5. One gap left open, and why it is not worked around

A template *body* that fails to lower is only **E0245, a warning** (PLAN's standing item), so the four
defects above all reached an engine rather than a diagnostic. That gating is its own change and would have
*masked* these four rather than exposing them, which is the same argument PLAN already records — now with
four more data points behind it.

### 6. `Sort` had no imports at all, and an unresolved name inside an intrinsic is invisible

The sort's first failure was neither of the four: `modules/Sort` had **no `#import`**, so `talloc` did not
resolve. Every earlier routine in that module is pure computation over a caller's view, so nothing had ever
needed one.

What made it cost an hour is that the failure surfaced as E0245 "a local has an error type" on the *whole
body* — because `check_typed` returns silently when its operand did not type, on the reasonable ground that
the operand's own error was already reported. It was not reported, because the module's diagnostics are not
shown when a *root* file is checked. Recorded here rather than fixed: the fix is either per-file diagnostic
surfacing for modules or a note on E0245 naming the inner expression, and both are their own decision.

## Consequences

- **`modules/Time`, `modules/Bucket_Array`** are new; `modules/Sort` gains `stable_sort`,
  `stable_sort_ints` and `ints_sorted_by`, plus its first `#import`.
- **Four `jr-sema`/`jr-db` fixes**, all four load-bearing for `valid/125` and three isolated by `valid/126`.
- **`Type_Info` ids move in the MIR snapshot**, as they do whenever corpus files are added: they are pool
  indices, and three new files add pool entries. Not `FileId`s, which `AGENTS.md` forbids snapshotting.
- **W7 is three of nine modules further on.** `JSON`, `File`, `File_Utilities`, `Process`, `Socket`,
  `Thread` remain; §8.3's order puts the two that need only `#foreign` scalars next.

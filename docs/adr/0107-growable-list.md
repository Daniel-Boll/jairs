# ADR-0107: `List` is a genuinely growable array — and the corpus differential caught its first real divergence

- **Status:** Accepted
- **Date:** 2026-08-04
- **Deciders:** dboll
- **W7 sub-wave 5.** ADR-0105 shipped a fixed-capacity buffer and named three refusals as the reason; ADR-0106
  lifted the first. This is the array that follows. Writing it exposed a **VM miscompile** that made the two
  engines disagree — the first time the corpus differential has caught a divergence rather than a shared
  answer, which is the thing it exists for.

## Context

Typed allocation (ADR-0106) made heap storage reachable. Probed and confirmed before writing: a struct holding
`data: *s64`, growth by **allocate–copy–free**, indexing through pointer arithmetic. All three work.

## Decision

### 1. A new module, not a rewrite of `Array`

The two have genuinely different **contracts**, and collapsing them would hide that. An `Int_Array` needs *no
cleanup* — its storage is inline — while an `Int_List` **owns heap memory** and a caller must call `free_data`.
There are no destructors in Jairs, a design value rather than a missing feature, so ownership is something a
caller reads in a type's name and docs or never learns.

`Int_Array` also remains the better choice when a bound is known: no allocation, no failure mode, no cleanup.
Replacing it would trade a simpler thing for a more capable one, which is not an improvement when both are cheap
to keep.

Still `Int_`, because **cross-file parameterised structs** and **inference through them** are both still deferred
(ADR-0085 §5). Typed allocation lifted the storage blocker; those two are what stand between this and `List($T)`.

### 2. The VM reclaimed heap memory on return — a real divergence, found by the differential

`valid/088` exited **247 in the VM and 255 natively**. Bisected to thirteen lines: a callee that allocates,
writes, and stores the pointer into its caller's struct. The write **succeeded inside** the callee and read back
**zero** outside.

The cause is in `jr-vm`'s memory model. Frames are a bump mark restored on return — correct for *slots*, since a
local's bytes should die with its frame — and `malloc` was **satisfied from the same cursor** (ADR-0061 §1
routes it into the VM's own region so a pointer stays a bounds-checked offset). So heap memory allocated inside
a callee was released when that callee returned, and the next frame reused the bytes. It read back **zero**
rather than garbage precisely because `release` zeroes for determinism — so the symptom was a clean wrong answer
rather than a crash.

**The fix: the heap grows downward from the top of the region.** A second cursor, `heap_next`, that no frame
release touches; the two regions meet in the middle and either running into the other is `Exhausted`, the same
diagnosable limit as before. No free list, because `free` is still a no-op — a comptime allocator still leaks
within the VM, which was already true and is bounded.

**Why this had never been caught.** Nothing before this sub-wave allocated in a callee and used the memory in
the caller. `talloc` (ADR-0065) is a per-context bump arena reset explicitly, and every prior `malloc` in the
corpus allocated and used within one frame. A growable array is the first construct whose *whole point* is
memory outliving the call that made it.

**And it is the differential's first real catch.** Every previous one found a construct both engines got wrong
together, or a leaked internal error. This is a case where one engine was right and the other was wrong, which
is the failure mode two independent implementations exist to expose — and it is why the corpus asserts *exit
codes* rather than agreement (the lesson already recorded as "assert behaviour, not agreement").

### 3. Growth doubles; a failed allocation is `false`

**Doubling from 4.** `n` pushes then cost `O(n)` amortised, which is the property that makes a growable array
worth having — a fixed increment makes it `O(n²)`, a bug wearing a policy's clothes. Four rather than 1 avoids
three reallocations before a small list settles; rather than 16 because a list that stays small should not hold
128 unused bytes.

**`push` returns `false` on a failed allocation, and does not trap.** ADR-0058 §4's line is that a trap is for a
*program* error, and running out of memory is not one — the program is correct and the machine said no, so
aborting would remove the caller's only chance to recover. It is the same `false` `Int_Array` returned when full:
a caller can do nothing different about the two reasons.

**`allocate–copy–free` rather than `realloc`.** The platform's `realloc` may extend in place, so using it would
make growth depend on an allocator behaviour the comptime VM does not model — the two engines could then differ
in *timing*. Not in results, but §2 is a fresh reminder of what a divergence costs to find.

### 4. `clear` and `free_data` are different routines on purpose

`clear` forgets the elements and keeps the buffer; `free_data` releases it. Reusing a buffer a caller has already
paid for is a real thing to want, and naming both makes the choice visible. `free_data` is safe on a list that
never grew (`data` is null) and safe twice, because it resets `data`.

`get` bounds on `count` for a **sharper** reason than `Array`'s: there the unused slots were *zeroed*, so a read
gave a real number merely indistinguishable from an element, while here they hold whatever the allocator
returned. The bound is not a matter of taste.

### 5. An imported module's own errors are not reported, and that is the next sub-wave

The module called `malloc` without importing `Basic`, and the program **checked clean** and then failed at run
time with `no routine for file 2 proc 0`.

The diagnosis is *not* what it first looked like. Resolution is per-file and entirely correct: checking the
module on its own gives `E0201: unresolved name malloc`, exactly right. What is missing is that
**`file_diagnostics` reports one file's diagnostics and nothing else** — so a root file whose *imported module*
is broken is reported as clean, and the failure surfaces from an engine instead.

That is a real gap with a real fix (report every reachable file's diagnostics), and it is deliberately **not**
done here: it changes what `jr check`, `jr run` and `jr build` all report, it needs a decision about whether a
module's errors are attributed to the module or to the import, and bundling it into a library sub-wave would hide
an outward-facing behaviour change inside a data structure. `List` imports `Basic` explicitly, which is correct
regardless, and the gap is W7's next sub-wave.

**The lesson worth keeping** is how it presented: the module was *both* wrong and unreported, and the run-time
message named a `FileId` rather than the missing import. Two of this project's five leaked internal errors have
now been a cross-file body that never got compiled.

## Consequences

- **A real dynamic array exists.** `valid/088` pushes ten elements through a four-element first allocation — two
  reallocations — and checks every element survived both copies, in both engines.
- **A VM miscompile is fixed** that would have hit *any* program allocating in a callee. It was reachable before
  this sub-wave and nothing exercised it.
- **No new diagnostic code**; E0280 is still the first free one.
- **The next sub-wave is reporting an imported module's diagnostics** (§5), which this found and deliberately
  left alone.
- **What still blocks `List($T)`**: cross-file parameterised structs, and inference through them.
- **What the module still cannot do**: hand out a `[]s64` **view of its used prefix**, which is how a caller
  would pass a list to `Sort` or `String`. Building a view from a pointer and a count is not something any
  expression can spell — a slice takes an *array*. That is a real gap and the next thing this module wants.

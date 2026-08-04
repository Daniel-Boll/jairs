# ADR-0104: `Sort` orders a view given a comparison — and two leaks writing it found

- **Status:** Accepted
- **Date:** 2026-08-04
- **Deciders:** dboll
- **W7 sub-wave 2.** The project's **third** module, and the first library code that is *polymorphic* — so the
  first that depends on W5 rather than merely coexisting with it. Writing it found **two** leaked internal
  errors, both in cross-file polymorphism, and both are fixed here rather than filed.

## Context

`String.compare` was shaped for sorting in the previous sub-wave, so `Sort` is what asks for it. Three language
facts had to hold and **all three were probed before a line was written**: a `[]T` view parameter is **mutable**
through the callee (so an in-place sort can exist), a `$T` parameter **infers through a view** (ADR-0084), and a
**procedure pointer** can be passed and called (ADR-0059). Writing the module first and finding one missing
would have meant designing around a gap instead of knowing there was none.

## Decision

### 1. An imported procedure used as a **value** now lowers (a leaked gap report, fixed)

`sort(xs, less_int)` passes `less_int` — an imported procedure — to a procedure-pointer parameter, and that
reported **"this compiler has a gap — please report it"** for a program the language allows.

The value was representable all along: `ImportedProcs` had already resolved the name to a `ProcRef`, and a
`DeclId` is a `(FileId, index)` pair, so the other file's procedure is nameable in the pool exactly as a local
one is. What was missing was a **three-line bridge** — `imported_proc_value` — plus the matching `scan` arm.

The local arm's own comment said a cross-file one "resolves to `Res::Imported` and is refused by that arm", so
this refusal was **known and undocumented** rather than unnoticed. That is worse than an oversight: it was
recorded as intended behaviour in a comment while surfacing to users as a compiler bug report.

### 2. A call to an **imported template** is now refused with E0268 (a leaked ICE, fixed)

Cross-file instantiation is deferred (ADR-0082 §5) — old news. But `callee_poly`'s documentation claimed an
imported template then *"reports an honest mismatch"* on the ordinary call path, and **that claim was false**: a
`$T` parameter's type is `PoolId::ERROR`, and `ERROR` matches anything, so the call **type-checked** and the
missing instantiation leaked out of whichever engine ran first as `no routine for file 2 proc 0`.

It survived because **nothing in the corpus had ever imported a polymorphic procedure**. `modules/Generic.jr`
now does, so the case is reachable, and `imports/invalid/017` pins the refusal.

`FileSignatures::template_names` carries the fact across the boundary, shaped exactly like `macro_names`
(ADR-0091 §3) and recorded by the same pass — because an importer has another file's *signatures*, not its HIR.
Separate from `macro_names` because the refusals differ: a macro cannot be **spliced** across a file, a template
cannot be **instantiated** across one, and a reader hitting either should be told which.

**The diagnostic names the workaround** — wrap the template in a non-polymorphic procedure *in the module that
declares it* — and `imports/valid/017` checks that the workaround works, because a refusal is only as good as
its escape route.

### 3. The caller supplies the comparison

`sort(xs, less)` rather than requiring `<` on the element type, and that is a language fact rather than a taste:
resolving an **operator** inside a `$T` template against the instantiated type is a lookup instantiation does
not do. `operator <` exists (ADR-0048) and `#modify` can *reject* an instantiation (ADR-0095), but nothing can
*select* an implementation per instantiated type. That is operator-bounded polymorphism — a real feature,
belonging to whichever wave decides how a template states its requirements.

A comparison parameter is also the only form that serves a scalar **and** a struct with nothing the language
lacks, and it composes with `String.compare`.

### 4. Insertion sort, and that is not a performance argument

`O(n²)`, said plainly. The reasons to choose it *here*: it is **stable** (equal elements keep their order, which
quicksort does not give and which a caller can rely on), it needs **no extra storage** (a merge sort would
allocate, and allocation is what ADR-0103 §3 declined to decide), and it is **short enough to read**, which
matters for the first sorting routine in a language whose test suite compares two independent engines.

A faster algorithm is a later decision with a benchmark behind it. W8 owns performance, and guessing now would
be choosing an algorithm without the measurement that justifies one.

### 5. The `_ints` wrappers are not conveniences

`sort_ints` and `ints_sorted` exist because §2's refusal means an importer **cannot** instantiate `sort` or
`is_sorted` at all. The wrapper lives where the instantiation can happen, so today it is the only way to use
this module — and it becomes a convenience when cross-file instantiation arrives. Saying so in the module docs
is better than letting a caller discover it as a diagnostic.

## Consequences

- **Two leaked internal errors turned into working code and a diagnostic** — the fourth and fifth such fixes
  this project has made, and both were found by *writing a library in the language* rather than by reading the
  compiler. That is the argument for a standard library written in Jairs, stated in PLAN.md decision #5 and now
  paying out twice in one sub-wave.
- **A stale comment was the bug's hiding place**, twice: one comment recorded the refusal as intended, the other
  promised a mismatch that could not happen. Both are corrected in place, and both said something checkable
  that nobody had checked.
- **`valid/085` exits 63**, six groups of one bit each, including the check that keeps the others honest: an
  *unsorted* view must be reported unsorted, because without it a `sort` that did nothing would satisfy every
  assertion that only reads a sorted array.
- **What `Sort` still owes**: a stable merge sort (wants allocation); binary search (wants a sortedness
  precondition nothing can check); sorting by a key extractor rather than a comparison (two forms where one
  suffices). And the language owes **cross-file instantiation**, which would delete §5's wrappers.

# ADR-0138: `..T` variadic parameter — declaration surface

- **Status:** Accepted
- **Date:** 2026-08-17
- **Deciders:** dboll
- **Wave 8 of eight.** This is the last of the ADR-0127-programme waves; ADR-0128 was wave 1, and
  waves 2–7 shipped as ADR-0129 through ADR-0137.
- No design fork was put to the decider. PLAN §7's table decided the shape: *the only language
  gap is the variadic parameter; spill args to slots per ADR-0077*.

## Context

### What Wave 8 promises

`print(fmt, ..Any)` — a procedure that takes a format string and any number of arguments. The
Jai/Odin form is: mark the last parameter `..T`, and the callee sees the trailing arguments as a
view of `T`. At the call site, the caller writes `print(fmt, a, b, c)` and the compiler packs
`{a, b, c}` into a stack array + view before making the call.

Two halves: the **declaration** (parameter shape, callee-side semantics), and the **call-site
packing sugar**. This wave delivers the declaration half.

## Decision

### 1. The declaration surface is delivered here; packing is deferred to a follow-up

`args: ..T` parses and lowers. HIR wraps the parameter's type as `[]T` — a view — so the callee
sees an ordinary view and can `for x: args { … }` or pass `args` to a `[]T` parameter of any
other procedure. The parameter's `variadic: bool` marker rides on `Param` and `ProcSig` for
consumers who need it.

**Rejected: ship the whole feature in one wave.** The call-site packing needs MIR to allocate a
stack array of the exact trailing-arg count, populate each slot, and build a view over it. That
is real MIR work (`Statement::Store` per arg, a synthesized `Rvalue::Slice`), and it is not the
critical piece for `print(fmt, ..Any)` to *type-check* — a caller writing `print(fmt, view)` with
an explicit view works today. Splitting keeps this wave's blast radius small and the follow-up's
scope focused on one MIR change.

**Rejected: parse `..T` and refuse the *declaration* pending the packing.** The type-check side
of the callee is ordinary — `..T` is a view — and refusing the declaration would refuse a
program the compiler can otherwise handle. The refusal belongs at the *call* site where the
sugar would be applied, not at the definition.

### 2. Refusal at the call site, with a specific message

When the fixed-arg count differs from the given-arg count (i.e., the caller is trying to use the
sugar rather than passing a view), sema reports E0216 with:

    packing trailing arguments into a variadic `..T` parameter is not implemented yet
    = the declaration surface is delivered by ADR-0138; automatic packing is a follow-up wave
    = help: pass an explicit `[]T` view for the variadic slot (e.g. build a `[N]T` and take a view)

**One reused code (E0216 for arity mismatch), one new message.** Reusing the arity code keeps
the diagnostic-code registry stable while the phrasing does the work — and the help is
actionable, so a reader unblocking themselves does not need this ADR.

### 3. The AST holds the marker as a `DOT_DOT` token child of `PARAM`

Same discipline as `$N`'s `DOLLAR` marker and `$$T`'s doubled dollars in ADR-0137: a token in
the CST captures the surface distinction, and the typed AST reads its presence rather than
carrying a separate node kind. `Param::is_variadic()` on the AST, `Param::variadic: bool` on
the HIR, `ProcSig::variadic_params: Vec<bool>` on the signature. Each layer holds the flag in
the shape appropriate to it.

**Rejected: a `VARIADIC_PARAM` node kind.** Cleaner in isolation and it would carry the flag in
the tree kind rather than through a token count. Rejected because the marker is a scalar
property of an otherwise-ordinary `PARAM`, and every downstream match on `PARAM` would have to
grow.

### 4. jr-fmt emits the `..` explicitly

Round-trip discipline (ADR-0027's lossy-CST trap this file guards against **seven times now**).
Without this, jr-fmt would silently drop the `..` and turn a variadic parameter into an
ordinary one — a change in what the program means. Corpus `valid/111` pins the round-trip:
declaring a variadic `sum :: (args: ..s64)`, passing an explicit view, and getting the right
answer through both engines.

## Consequences

- **The eight-wave programme is 8 of 8 done.** All six of ADR-0127 §3's unkept promises are
  kept; the two extras (ADR-0129 enum-member-from-constant, ADR-0135 range-with-index) are
  deferrals closed on the way.
- **1010 workspace tests unchanged; 223 → 224 corpus files.** `valid/111` exercises the
  declaration side and pins that an explicit `[]T` view works — no test file for the
  refusal yet, deferred with the packing itself.
- **`ProcSig` gains `variadic_params: Vec<bool>`**, parallel to `comptime_params`. `Param`
  gains `variadic: bool`.
- **Deferred, and this is what "wave 8" leaves owed**: the call-site packing. That is a
  focused MIR change (allocate a stack array, store each trailing arg, build a view) plus the
  `variadic_calls` sink already scaffolded in `ConstValues::variadic_call`. The follow-up ADR
  writes those two pieces and lifts the E0216 refusal.
- **`print(fmt, ..Any)` is now half-callable**: a caller building an `[]Any` view by hand can
  call it today. The sugar is what the follow-up delivers.

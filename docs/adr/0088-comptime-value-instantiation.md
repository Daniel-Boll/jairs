# ADR-0088: A comptime-value call is instantiated by evaluating the argument and baking it into a clone

- **Status:** Accepted
- **Date:** 2026-08-04
- **Deciders:** dboll
- **W5 sub-wave 6, second half — design of record.** ADR-0087 delivered the `$N: s64` surface and refused
  a call (E0271). This ADR fixes the design of the *second half*, which makes a call run: it evaluates each
  `$N` argument to a compile-time constant, appends a concrete procedure with that value baked in, and lifts
  E0271 — the value-side counterpart of ADR-0082's `$T` instantiation. **It is written now, at the point of
  decision, and its implementation is deferred**, exactly as ADR-0085 was for polymorphic structs: the build
  is larger than a single edit and spans check → `jr-db` → `jr-hir` → MIR → both engines, and a partial
  version is unsafe in a specific way — removing ADR-0087's E0271 refusal before the whole pipeline exists
  makes a comptime call fall through to the template (which has no MIR), a miscompile. So the surface stays
  as ADR-0087 shipped it (E0271 refuses the call) until the second half lands atomically on this design.

  **One design point this ADR under-specified, recorded for the build to resolve first:** `instantiated()`
  re-resolves the expanded HIR, so a body's bare `N` re-resolves against the clone's parameter list. If §3
  drops the comptime parameter, `N` becomes unresolvable unless the value is substituted *before*
  re-resolution — which means `expand_instantiations` must rewrite the `Expr::Name` for a comptime parameter
  into a literal during the clone (threading the base resolve map through), rather than relying on the MIR
  substitution §4 describes alone. The build must settle whether the substitution happens at HIR-rewrite
  time (before re-resolve) or whether the clone keeps the parameter resolvable and only MIR substitutes; §3
  and §4 as written assume the latter but the re-resolve makes the former necessary. Resolve this before
  writing code.

## Context

A `$T` instantiation keys on a **type** the checker already knows: `check_polymorphic_call` infers it and
records it, and `jr-db`'s `instantiated()` expands the HIR. A `$N` instantiation keys on a **value**, and a
value is not known at check time — const-eval lives in `jr-db`'s `file_consts` over the bytecode VM,
*downstream* of the checker (ADR-0018 §3). So the `$T` mechanism cannot be copied wholesale: the value has
to be produced by the same acyclic const-eval pre-pass `#insert` uses (ADR-0073), keyed by the call's
**span** (a two-pass lowering must key cross-pass results by span, not by an id that shifts when the tree
expands), and only then can the instantiation be built.

## Decision

### 1. The checker records a comptime call's argument *expressions*, not values

`check_call` recognises a comptime-template callee (the `callee_comptime_template` of ADR-0087) and, instead
of refusing with E0271, records `(scope, call) → (proc, [argument ExprId per comptime parameter])` in a new
`comptime_calls` side table — the argument *expressions*, because their values are not known here. The
non-comptime arguments are type-checked as usual, and each comptime argument is checked against its
parameter's (known) type. The return type is the template's, concrete already.

### 2. A `jr-db` pre-pass evaluates each comptime argument to a constant

A new `comptime_call_values` query (shaped exactly like `insert_operands`, ADR-0073) walks
`comptime_calls`, evaluates each recorded argument `ExprId` through the *same* `file_consts` evaluator via a
new `Wanted::ComptimeArg` target, and returns `(call span) → [PoolId value per comptime parameter]`. Reusing
the evaluator rather than writing a second is what keeps the two from disagreeing, and keying by span is
what survives the HIR expansion that follows. The pre-pass is acyclic for the reason `insert_operands` is:
it depends on signatures and const-eval, never on `file_mir`.

### 3. The instantiation clone **drops** the comptime parameters and bakes their values

`instantiated()` keys an instantiation on the tuple of `(template, [argument values])` — the value-side
analogue of ADR-0005's structural key over types — and `expand_instantiations` appends a clone whose
parameter list **omits the comptime parameters**, recording each dropped parameter's baked value in a new
`FileHir::param_values: Vec<(ProcId, ParamId, PoolId)>` side table.

**Why drop rather than keep-and-bind.** A comptime parameter has no runtime existence in the instantiation —
the caller passes no value for it — so leaving it in the parameter list would make the instantiation's ABI
disagree with its call. Dropping it keeps the instantiation an ordinary procedure whose parameter count is
its *runtime* arguments, which both engines already handle. The cost is that a body's `Res::Param` indices
shift when a parameter is removed, so the clone **remaps** them: this is done in `instantiate.rs`, which
already deep-copies the body, so the remap is one pass over the copied `Res::Param` references.

### 4. MIR substitutes the baked value for a reference to a comptime parameter

Lowering a `Res::Param(p)` where `p` is a dropped comptime parameter emits the baked constant from
`param_values` rather than a parameter load — the same substitution `type_info` uses for a folded value.
Every other reference is unchanged. Neither engine's back end changes: an instantiation is an ordinary
procedure with ordinary parameters and some constants in its body.

### 5. `[N]T` over a comptime parameter resolves once the value is baked

An array type `[N]s64` whose length names a comptime parameter reads the baked value at resolution time —
`constant_array_length` (ADR-0070 §1) gains a source: a `$N` parameter bound to a value. This is the case
that meets polymorphic structs (`buf: [N]T`), and it falls out of §3–§4 because by the time the
instantiation's signature is resolved, `N`'s value is in `param_values`.

## Consequences

- **E0271 is lifted** for an instantiable call, exactly as ADR-0082 lifted `$T`'s E0268. A call that cannot
  be instantiated — a comptime argument that is not a compile-time constant — is refused with a *reworded*
  E0271 naming that reason, the way a non-literal array length is E0233.
- **The structural-key discipline extends to values** (ADR-0005): `make(4)` and `make(4)` share one
  instantiation, `make(4)` and `make(8)` are two. The key is the tuple of interned value `PoolId`s, deduped
  the way the type key is.
- **A comptime argument must be a compile-time constant**, and the pre-pass is where that is judged — a
  runtime value there is refused, not silently read, the same rule an array length has.
- This is the const-eval-at-a-call the sub-wave split deferred, and it reuses the acyclic pre-pass rather
  than reopening the sema↔VM recursion PLAN §5 named as W4's top risk.

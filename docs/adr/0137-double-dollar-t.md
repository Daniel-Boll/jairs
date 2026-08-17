# ADR-0137: `$$T` — polymorphic and comptime, together

- **Status:** Accepted
- **Date:** 2026-08-17
- **Deciders:** dboll
- **Wave 7 of eight.** ADR-0128 was wave 1, 0129 wave 2, 0130–0132 wave 3, 0133/0135 wave 4,
  0134 wave 5, 0136 wave 6. Wave 8 (`print(fmt, ..Any)`) remains.
- No design fork was put to the decider. PLAN §7's table had already decided the shape:
  *`$T` inference plus required-constant baking (ADR-0087's `$N` mechanism)*.

## Context

### The two mechanisms this wave combines

- **`$T`** (ADR-0081): the parameter's *type* is a variable inferred from the argument's type.
  Bindings are recorded per call and the template is cloned per distinct binding tuple.
- **`$N`** (ADR-0087): the parameter's *value* must be a compile-time constant, and that constant
  is baked into the clone. The parameter type is concrete (e.g. `s64`); only the value varies.

`$$T` is the union of both: the type is a variable inferred from the argument (like `$T`),
**and** the value is required to be a compile-time constant and baked into the clone (like `$N`).

### Why the pure-either callee-check dispatch missed the mixed case

`callee_poly` selected a template with `!sig.poly_vars.is_empty() && !sig.comptime_params.any()`;
`callee_comptime_template` selected one with `has_comptime && sig.poly_vars.is_empty()`. A mixed
template — `$$T` — falls through both, and the ordinary call path then compares the argument
against an `ERROR`-typed parameter (because `$T` doesn't resolve without inference). That is the
`an expression has an error type` MIR gap this wave closes.

## Decision

### 1. Parser: two `$`s in the type mean "comptime-required"

The parser produces a `POLY_TYPE` node whose children include one or two `DOLLAR` tokens. The
AST's `PolyType::is_comptime()` counts the tokens: two means the parameter is a `$$T`. The
same node kind serves both, because everything downstream that reads the node's *name* stays
identical — the only difference is whether the parameter is also comptime.

**Rejected: a new `POLY_COMPTIME_TYPE` kind.** Cleaner in isolation and it would carry the flag
in the tree kind rather than the token count. Rejected because every match on `POLY_TYPE`
(sema, MIR, jr-fmt, resolver) would have to be extended, and the flag is a *scalar* property
of the same node — the analogous carry for `is_expand` on `Proc` is via a child token, and this
wave follows that precedent.

### 2. HIR: the `$$T` marker flips `Param::comptime`

`Param::comptime` was ADR-0087's field for `$N: T` parameters. HIR lowering now sets it for a
parameter whose *type* is a `PolyType` with `is_comptime() == true`. One flag, two syntactic
routes to it. Downstream code that reads `comptime` does not need to know which route was
taken.

### 3. Sema: `callee_poly` accepts mixed; `check_polymorphic_call` also records comptime args

`callee_poly`'s condition drops the `!comptime` restriction. A template that has *any* poly
variable — with or without comptime parameters — goes through `check_polymorphic_call`. That
function was already inferring `T` from the argument's type; it now *also* records the
comptime arguments in `comptime_calls` when the sig has comptime params, so the pre-pass
evaluates each `$$T` argument to a constant and the instantiation clones with both the type
substitutions **and** the baked values.

**Rejected: extend `callee_comptime_template` to accept mixed and rewrite
`check_comptime_call` to also infer types.** Structurally the same change from the other side.
Rejected because `check_polymorphic_call` already runs first and does the inference work, so
teaching it the comptime side is smaller than teaching `check_comptime_call` the inference.

**Rejected: introduce a third `check_mixed_call` and a third callee-check.** It reads well as
a third variant of the same three-case dispatch, and it duplicates most of
`check_polymorphic_call`. The one-function version keeps the type-binding logic in one place.

### 4. Refusals are inherited unchanged

- A `$$T` argument that is not a compile-time constant is E0271 (the existing `$N` refusal).
- A `$$T` parameter that no argument position pins is E0268 (the existing `$T` refusal:
  "cannot infer every `$T` from the arguments of this call").
- A `$$T` inside an operator overload's parameter list — reserved for future exploration — is
  not attempted here; overloads' polymorph is a separate deferral (ADR-0104 §3, unchanged).

## Consequences

- **The eight-wave programme is 7 of 8 done.** Wave 8 (`print(fmt, ..Any)`) is next.
- **1010 workspace tests unchanged; 223 → 224 corpus files.** `valid/110` exercises
  inference over `s64` and `bool` and the value-is-baked mechanic (a different literal at the
  same call site produces a distinct instantiation with a distinct result).
- **`callee_poly` and `check_polymorphic_call` now handle the mixed case**, so a future wave
  can add `$$T` to a signature that also has runtime `$T` parameters without further code
  changes. `#modify` predicates and `#expand` macros are unaffected — their callee-arms come
  after `callee_poly` and only run when the mixed case is not selected.
- **Rejected refusals reused rather than invented**: E0271 (comptime not constant) and E0268
  (cannot infer $T). The wave adds no new diagnostic code.
- Deferred: `$$T` in an operator overload (see §4); a `$$T` argument passed to a runtime `$T`
  parameter of the same template (a legitimate use once both are declared — same-name variable
  shared between the two — still owed).

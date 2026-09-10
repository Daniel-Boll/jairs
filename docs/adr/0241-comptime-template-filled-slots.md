# ADR-0241: Comptime templates carry declaration-ordered argument slots

- **Status:** Accepted
- **Date:** 2026-09-10
- **Deciders:** dboll
- **Scope:** Named arguments and literal defaults on local `$N` and mixed
  `$T`+`$N`/`$$T` procedure calls. Imported value-parameterised specialisation,
  non-literal defaults, and defaults that would infer `$T` remain separate work.
- **Amends ADR-0236 §3:** the ordinary filled-argument binder now feeds every
  local template call, including calls with compile-time value parameters.

## Context

ADR-0236 gave pure `$T` calls the ordinary declaration-ordered argument binder,
but retained a narrower representation for a `$N` call:

```text
(procedure, [source ExprId for each comptime parameter])
```

That representation cannot describe an omitted literal default. The value is
already interned on the declaration, and there is no caller expression to name.
It also forced the checker to reject every named `$N` or mixed call before the
ordinary binder could diagnose duplicate names, omissions, or positional
arguments following named ones.

The distinction is not semantic. A named argument still fills a declaration
position, and a literal default is still a value fixed by that declaration.
Only downstream compile-time evaluation cares whether a slot contains a caller
expression or an already-known value.

## Decision

### 1. Every local comptime-template call records one slot per parameter

`CheckOutput::comptime_calls` carries the existing `ArgSlot` vocabulary in
declaration order:

- `Given(expr)` names the caller expression that filled the parameter;
- `Default(value)` carries the declaration's already-interned literal.

The vector covers every parameter, not only `$N` positions. The procedure
signature remains the canonical comptime mask, so consumers select exactly the
slots whose parallel `comptime_params` entry is true. Keeping the full vector
preserves the binder's declaration-order result and avoids a second compressed
index convention.

### 2. The ordinary binder owns names, omissions, and defaults

Both pure `$N` and mixed `$T`+`$N` calls use `fill_arguments` whenever a name or
default is present. Positional calls with no defaults keep their allocation-free
fast path.

Every supplied expression is checked against its declared or concretised
parameter type. For a mixed template, only `Given` slots contribute `$T`
inference evidence. A default whose type mentions `$T` remains E0252 because
omission must not invent a type binding.

### 3. Only supplied comptime expressions become evaluation targets

`file_consts` creates a `Wanted::ComptimeArg` only for a comptime slot containing
`Given(expr)`. A `Default(value)` creates no thunk and cannot fail const
evaluation: signature checking already interned and type-checked the literal.

When specialisation keys are assembled, a supplied slot reads its evaluated
value from `ConstValues`, while a default slot contributes its `PoolId`
directly. The final key is still ordered by comptime parameter declaration.

### 4. A literal may default the comptime parameter itself

This is legal:

```jr
width :: ($N: s64 = 4) -> s64 {
  return N;
}

answer := width();
```

It specialises exactly as `width(4)` does. A fixed ordinary parameter on the
same template may also have a literal default. Defaults remain literal-only,
and a parameter whose type contains `$T` remains ineligible.

### 5. Imported `$N` and mixed templates remain E0268

This wave changes call evidence, not owner-file specialisation. ADR-0230's
root-scoped imported `$T` machinery still has no cross-file value-evaluation
channel for `$N`, so imported value-parameterised templates remain explicitly
refused rather than partially instantiated.

## Rejected alternatives

### Manufacture an expression for an omitted default

Rejected because the expression would belong to neither the caller nor its
scope. It would corrupt source locations and make compile-time evaluation redo a
value signature checking already knows.

### Store only comptime slots

Rejected because it introduces a second compressed index space beside parameter
order. The signature already supplies the mask; preserving all slots lets every
consumer use the same declaration positions.

### Keep a separate named/default binder for comptime calls

Rejected because it would duplicate ADR-0053's rules and eventually disagree on
duplicates, omissions, suggestions, or diagnostic order.

### Infer `$T` from a default

Rejected for ADR-0236's reason: a declaration default must not silently turn an
otherwise polymorphic parameter into an implicit concrete specialisation.

## Consequences

- Local `$N` and mixed calls may reorder supplied arguments by name and omit
  literal-defaulted fixed or comptime parameters.
- `width()` and `width(N = 4)` produce the same value-specialisation key.
- Compile-time evaluation runs only for supplied `$N` expressions.
- `ArgSlot` becomes the shared semantic boundary for ordinary, pure `$T`, mixed,
  and pure `$N` argument binding.
- MIR and all three execution engines are unchanged; they consume the concrete
  instantiated call exactly as before.
- The first free global diagnostic code remains E0301; the first free parser
  code remains E0137.

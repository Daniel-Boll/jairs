# ADR-0235: `name := literal` infers a parameter's fixed type from its default

- **Status:** Accepted
- **Date:** 2026-09-10
- **Deciders:** dboll
- **Scope:** The inferred spelling of a defaulted procedure parameter. Richer default expressions,
  result defaults and procedure-pointer parameter metadata remain separate work.
- **Amends ADR-0053 §2:** a default may still be written `name: T = literal`; when the annotation is
  omitted as `name := literal`, the compiler infers the one fixed parameter type from that literal.

## Context

The pinned *Way to Jai* guide writes:

```jai
hello :: (a := 9, b := 9) { ... }
```

Jairs already has every semantic piece below that surface:

- a default is interned while the procedure signature is resolved;
- a call is rebound into one positional slot per parameter;
- local, imported and `#run` calls consume that same resolved slot list;
- literals already have the natural no-context types needed here: `s64`, `float64`, `bool` and
  `string`.

What is missing is the declaration spelling and one answer to “where does the type live?” A
parameter's explicit type is a `TypeRef`, but `amount := 9` has no type syntax to lower. Inventing a
synthetic `s64` type reference would make the HIR claim the user wrote an annotation they did not
write, and it would duplicate sema's existing literal-typing rule.

The guide also says a default can be a symbol or a call. ADR-0053 refused that for a concrete query
ordering reason: signature resolution runs before the constant evaluator that would have to compute
such a value. This surface wave does not overturn that dependency.

## Decision

### 1. A parameter may be written `name := literal`

Both forms remain legal:

```jr
typed :: (amount: s64 = 9) -> s64 { return amount; }
inferred :: (amount := 9) -> s64 { return amount; }
```

The inferred form contains a default by construction. It does not introduce a general untyped
parameter and it does not infer from a caller: `amount` is `s64` for every call, including one that
supplies the argument explicitly.

The literal determines the fixed type exactly as it does without another context:

| Default | Inferred parameter type |
|---|---|
| integer literal | `s64` |
| `#char` literal | `s64` |
| float literal | `float64` |
| boolean literal | `bool` |
| string literal | `string` |

A negative integer is already lowered as one `Literal::Int`, so `amount := -9` follows the integer
row rather than introducing a unary-expression exception.

`null` has deliberately never had a fallback pointer type (ADR-0060 §1). Therefore `p := null`
reuses E0257, with help to write an explicit pointer annotation such as `p: *u8 = null`. A new E0300
would give the same source mistake two codes depending only on whether it occurred in a body or a
parameter list.

### 2. HIR records that inference was written; sema owns the inferred type

`jr_hir::Param` gains `inferred: bool`. Its `ty` remains `None` and its `default` remains the
faithfully lowered expression.

The signature phase:

1. validates that the default is a literal under ADR-0053's existing rule;
2. chooses the natural type above when `inferred` is true;
3. interns the literal as a value of that type;
4. records both in the ordinary `ProcSig.params` and `ProcSig.defaults` vectors.

That makes `ProcSig` the one resolved answer for bodies, ordinary calls, imported calls, MIR and
`#run`. No consumer needs a second inferred-type side table.

This first slice refuses an inferred default anywhere on a `$T`, `$N` or `$$T` template with E0252.
Template calls currently dispatch before the ordinary argument binder and require exact written
arity, so accepting the declaration would advertise a default the caller could not omit. Refactoring
template calls to consume the same bound slots is separate default-argument work; a defaulted
compile-time position additionally needs to carry an interned value where that path currently carries
only an argument `ExprId`.

### 3. Existing default restrictions remain unchanged

- A non-literal default is still E0252, whether written with `=` or `:=`.
- A `#foreign` parameter still cannot have a default.
- A polymorphic or compile-time-parameterised procedure cannot use the inferred spelling in this
  slice; its call path does not yet consume defaults.
- `name: T = literal` retains its contextual fit checks, including narrow integer types and typed
  `null`.
- Defaults may remain non-trailing, and the existing positional-before-named call rule is unchanged.
- Result defaults, implicit result bindings, names/defaults through procedure pointers, aggregate
  literals and caller-location/context defaults are outside this ADR.

### 4. Tooling preserves the distinction

The formatter emits `name := literal` rather than manufacturing `name: T = literal`. Tree-sitter
accepts the same two parameter alternatives, and its generated parser remains tracked. Documentation
and the chapter-17 compatibility probe show both the inferred spelling and calls that omit, override
and name its values.

Adding the second parameter alternative changed Tree-sitter's otherwise-equal GLR reduction order:
an ordinary `-> (s64, bool)` result list began parsing as an optional-arrow procedure-pointer type
with no `ERROR` node. The Neovim shape verifier, not gate 6's clean-parse check, caught it. The
declared conflict still keeps both readings alive; a dynamic preference chooses `result_list` only
when no following `->` has already made the procedure-pointer reading unambiguous. Both sides are
asserted: ordinary multiple results and a returned `(s64, s64) -> s64` pointer.

## Rejected alternatives

### Synthesise a `TypeRef::Name("s64")` during lowering

Rejected because lowering would turn inferred source into explicitly annotated HIR and duplicate the
natural literal-type policy. The signature already has the right resolved-type slot.

### Infer from each supplied argument

Rejected because one declaration would then have several procedure types. A default is part of the
signature, not a fallback inference hint.

### Admit constants and calls while adding the spelling

Rejected because it would quietly reverse ADR-0053's query-order decision without deciding
declaration scope, call-site scope, evaluation timing or side effects.

### Give `null` a default pointer type

Rejected for ADR-0060's reason: there is no distinguished pointer type to choose, and guessing
`*u8` would make a type claim the source did not make.

## Consequences

- The inferred spelling composes automatically with named arguments, imported ordinary calls and
  `#run` because those paths already consume `ProcSig` and `FilledArgs`.
- The common typed spelling and every back end are unchanged.
- `Param` construction sites must state whether inference was written, making omissions compile
  errors rather than silent defaults.
- The editor verifier again demonstrated why a zero-error parse is weaker than a correct tree shape:
  the first generated grammar accepted the whole corpus and assigned the wrong node kind.
- The first free global diagnostic code remains E0300; the first free parser code remains E0136.
- Non-literal Jai defaults remain the next default-argument design problem rather than being hidden
  by surface parity.

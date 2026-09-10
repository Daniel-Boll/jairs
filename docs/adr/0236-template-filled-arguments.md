# ADR-0236: Pure `$T` calls consume the ordinary filled-argument binder

- **Status:** Accepted
- **Date:** 2026-09-10
- **Deciders:** dboll
- **Scope:** Named arguments and literal defaults on pure type-polymorphic (`$T`) procedures.
  Defaulted `$N`/`$$T` positions and non-literal defaults remain separate work.
- **Amends ADR-0082 §1 and ADR-0235 §2:** type-polymorphic dispatch still owns instantiation, but it
  first consumes ADR-0053's declaration-ordered argument slots. An inferred literal default is legal
  on a fixed-type parameter of a pure `$T` procedure.

## Context

Template calls dispatch before the ordinary procedure path because the source signature contains
`ERROR` placeholders for `$T`. That was correct for type checking and wrong for argument binding:
`check_polymorphic_call` compared source arity and zipped source-order arguments directly with
declaration-order parameters, never consulting `arg_names` or `ProcSig.defaults`.

The omission is already a silent wrong answer, not only a missing convenience:

```jr
fixed_default :: (value: $T, scale: s64 = 2) -> T {
  return value;
}

answer := fixed_default(scale = 4, value = 8);
```

The call compiles today and returns `4`, because `scale = 4` is treated as the first positional
argument and is used to infer `T`. The same procedure called as `fixed_default(8)` reports E0216
instead of applying `scale`'s declared default. MIR already has the correct mechanism:
`FilledArgs` rewrites a call into declaration order and materialises omitted defaults, but sema never
records that map for template calls.

There is a second false promise at the declaration. `value: $T = 3` parses and lowers, but the
signature cannot intern `3` before `T` is inferred, so the default silently becomes absent and an
omitting call reports only that its arity is short.

## Decision

### 1. A pure `$T` call binds names and defaults before type inference

For a local or imported pure `$T` procedure, sema calls the existing `fill_arguments` when the call
uses a name or the signature has any default. The result has exactly one slot per declared
parameter:

- `Given(expr)` identifies the caller expression for that declaration position;
- `Default(value)` identifies the already-interned literal from the signature.

Only `Given` slots participate in `$T` inference. A default is a value already checked against a
fixed declared parameter type; it does not invent or refine a type binding. Once bindings are known,
the given expressions are checked against the concrete parameter types as before.

The same filled slots are recorded in `CheckOutput::filled_calls`. MIR already consults that map
after redirecting a template call to its concrete instantiation, so named order and omitted defaults
need no MIR or back-end change.

### 2. Defaults are legal only on parameters whose type is fixed before the call

These pure `$T` declarations are supported:

```jr
pick :: (value: $T, scale: s64 = 2) -> T { return value; }
labelled :: (value: $T, label := "item") -> T { return value; }
```

The inferred `label` parameter is fixed as `string` by ADR-0235 and does not depend on `T`, so
ADR-0235's blanket template refusal is narrowed to templates with compile-time parameters.

A default on a parameter whose annotation contains a polymorphic type variable is E0252:

```jr
bad :: (value: $T = 3) -> T { return value; }
```

The declaration is refused rather than retaining a default that cannot be called. A supplied
argument may still bind such a parameter by name; only its default is invalid.

### 3. `$N` and mixed `$T`+`$N` calls remain outside this slice, honestly

The comptime instantiation path records source `ExprId`s so `jr-db` can evaluate them later. An
omitted default is already a `PoolId` value and has no source expression to record. Therefore:

- any default on a `$N` or mixed `$T`+`$N` procedure is E0252 at the declaration;
- a named call to a local comptime or mixed template is E0252 rather than being silently interpreted
  positionally;
- imported `$N`/mixed templates retain E0268 from ADR-0230.

A later wave may generalise comptime call evidence from “argument expressions” to
“given expression or interned value”. This ADR does not fake an expression for an omitted value.

### 4. Existing argument rules and diagnostics are shared, not copied

`fill_arguments` remains the only implementation of:

- positional arguments preceding named ones;
- unknown-name suggestions;
- duplicate position/name detection;
- non-trailing default completion;
- “no argument and no default” diagnostics.

Template calls therefore use the same E0252 wording and ordering as ordinary calls. E0216 remains
the fast-path arity diagnostic for a positional template call whose signature has no defaults.

## Rejected alternatives

### Infer `$T` from an omitted literal default

Rejected because a declaration like `value: $T = 3` would silently make `T = s64`, turning a
polymorphic parameter into an implicit specialisation rule. Type variables remain inferred from
caller-supplied evidence.

### Reimplement named/default binding inside `check_polymorphic_call`

Rejected because two binders would eventually disagree about duplicate names, non-trailing defaults
or diagnostic ordering. The existing `ArgSlot` is already the right boundary.

### Support `$T`, `$N` and `$$T` in one change

Rejected because type inference consumes types available during sema, while comptime instantiation
consumes values evaluated downstream. Their missing plumbing is different, and conflating them would
either widen the query dependency or manufacture source expressions that do not exist.

### Refuse all named arguments on templates

Rejected because the language already accepts the spelling and ordinary calls define its meaning.
More importantly, refusal would repair the silent wrong answer by deleting a useful feature whose
correct implementation is the existing binder plus one aligned-slot traversal.

## Consequences

- Reversed named arguments on a pure `$T` call select the declared positions and can no longer alter
  which argument infers a type variable.
- Ordinary local/imported calls to pure templates may omit fixed literal defaults, including
  ADR-0235's inferred fixed defaults. Calling a pure template from `#run` remains a separate
  specialization/lowering gap: the filled slots are correct, but the compile-time VM has no
  instantiated routine to execute.
- Defaults never provide type-variable evidence; an otherwise uninferable `$T` remains E0268.
- Unsupported comptime-template names/defaults become explicit E0252 diagnostics instead of wrong
  positional execution or unusable declarations.
- MIR, the VM, Cranelift and LLVM are unchanged; they already consume declaration-ordered
  `FilledArgs`.
- The first free global diagnostic code remains E0300; the first free parser code remains E0136.

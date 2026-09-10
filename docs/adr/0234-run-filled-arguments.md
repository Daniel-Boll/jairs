# ADR-0234: `#run` consumes sema's resolved named and default arguments

- **Status:** Accepted
- **Date:** 2026-09-10
- **Decider:** dboll

## Context

Ordinary calls already have one owner for argument binding. `jr-sema` matches names to a
procedure's parameter names, inserts omitted literal defaults, checks every supplied value, and
records the resulting positional list in `CheckOutput::filled_calls`. `jr-db` translates that list
to `jr_mir::FilledArgs`, and procedure-body MIR consumes it without knowing which arguments were
named or omitted.

The standalone thunk used to evaluate a `#run` did not receive that map. It lowered the source
argument order directly and rejected only an arity mismatch:

```text
add :: (a: s64, b: s64 = 3) -> s64 { return a + b; }

A :: #run add(4);              // refused for being one operand short
B :: #run add(b = 3, a = 4);   // two operands, but in the wrong order
```

The refusal and ADR-0053 attributed this to const-eval running before checking. That premise has
expired. `file_consts` now begins by requesting `checked`, already reads its operator and filled
argument maps when lowering procedure bodies, and introduces no new query edge by handing the same
map to a thunk. The gap is one missing input, not a phase-order constraint. ADR-0196's consequence
that a `#run` could already use a default was therefore too broad: it was true inside a procedure
body lowered with that map, but not of the standalone thunk's own call.

## Decision

### 1. A thunk receives the same `FilledArgs` as ordinary MIR lowering

`jr_mir::lower_const` takes `&FilledArgs`, and the thunk stores that reference beside its types,
resolved names, constant values and imported procedures.

For a call expression:

- if `FilledArgs` contains the `(ExprScope, ExprId)` key, each `FilledArg::Expr` is lowered from the
  call's own expression arena and each `FilledArg::Default` becomes that already-interned constant;
- otherwise the written positional arguments are lowered in source order, as before;
- the implicit context remains the leading argument for a Jairs-convention callee.

The resulting operand list is checked against the declared parameter count before MIR is emitted.
This keeps the existing honest refusal if a caller ever invokes `lower_const` without the checked
evidence it needs, while no longer rejecting a valid omitted default.

Sema remains the only implementation of name matching and default insertion. The thunk does not
inspect `arg_names`, procedure parameter names or default declarations.

### 2. The rule applies at file scope and inside a procedure body

`FilledArgs` is keyed by both `ExprScope` and `ExprId`, exactly as the thunk's type and constant maps
are. A file-scope `#run` and a body `#run` whose call expressions share an arena index therefore
cannot collide.

An imported callee uses the same rule. The importing file's checked call owns the positional list;
the imported procedure's body is still lowered in its declaration file.

### 3. This wave changes call placement, not default-value semantics

Defaults remain the literal values ADR-0053 accepts. This ADR does not add:

- inferred parameter defaults such as `amount := 9`;
- arbitrary expressions, constants, procedure calls, `context` values or caller-location defaults;
- defaults or names on indirect procedure-pointer calls;
- parameters after a variadic parameter; or
- defaulted named results.

Those features need their own decisions about type inference, declaration versus call-site scope,
evaluation timing and side effects. Letting `#run` consume an already-decided literal does not settle
them.

## Rejected alternatives

- **Bind names and defaults again in the thunk.** This would duplicate sema's unknown-name,
  duplicate, ordering and omission rules in MIR. A disagreement could silently call a procedure
  with different operands at compile time and run time.
- **Desugar calls during HIR lowering.** HIR lowering does not yet know which declaration a callee
  resolves to, and replacing the written argument order would make the source model less useful to
  the formatter and LSP.
- **Support omitted defaults but not named reordering.** Both are represented by the same positional
  evidence. Consuming only one shape would preserve a same-arity compile-time miscompile.
- **Broaden defaults to arbitrary expressions in this wave.** That is a separate semantic feature;
  it must decide where and when an omitted expression runs rather than being smuggled in as thunk
  plumbing.

## Consequences

- Local and imported `#run` calls use the same literal defaults and named reordering as ordinary
  calls, at file scope and inside procedure bodies.
- No new diagnostic code, MIR operation, VM instruction or backend ABI is added.
- The stale capability and language-guide claims that imported defaults are absent are corrected.
- Inferred defaults and richer default expressions become the next default-argument parity work.
- Because `jr-mir` changes, this wave runs gate 7 as well as the six ordinary gates.

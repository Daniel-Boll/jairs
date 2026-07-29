# ADR-0046: `xx` and bare `.RED` are one idea — the context supplies what the source omits

- **Status:** Accepted
- **Date:** 2026-07-28
- **Deciders:** dboll
- **Amends:** ADR-0041 §2, whose five-step plan for bare `.RED` is executed here. The plan is
  followed rather than revised; step 5 (`.RED` in a `switch`) stays deferred because there is
  still no `switch`.
- **Depends on:** ADR-0037 §2 for the conversion `xx` performs, and ADR-0016 §1 / ADR-0040 §5
  for the context-typing machinery both features extend.

## Context

`PLAN.md` §2.1 lists `xx` autocast and operator overloading as the last of W1. `PLAN.md` §7
records that **`xx` should be designed together with bare `.RED`**, on the grounds that both
are "the context knows the type, so the source need not say it" and deciding them separately
would produce two mechanisms for one idea. This ADR takes that instruction seriously: the two
features share a design, a diagnostic shape, and a rule about where context comes from.

Operator overloading is **not** in this ADR — see §6.

Six facts were established by reading and running the code before this ADR was written, and
three of them shaped the decisions.

- **`XX_KW` has been reserved since the slice**, refused in expression position with "`xx`
  arrives in wave W1". So this wave removes a refusal, and `union` has just walked the same
  path — the tree-sitter reserved-keyword match is the thing to check (ADR-0045's Consequences,
  three for three).
- **`check_operands` already threads context in both directions.** With no expectation of its
  own it types the non-literal side first and passes *that* as the other side's context. So
  ADR-0041 §2's step 4 — "`if c == .RED` should work" — is half-paid already, and the same is
  true for a call argument, which passes the parameter type. Confirmed by reading, not assumed.
- **`Rvalue::Convert` is the whole of `cast`'s lowering**, and it carries a `NumKind` for the
  source. `xx` needs no new MIR at all: it is a `cast` whose target came from the context
  rather than from the source, and by the time MIR is built the target is just a type.
- **`Expr::Cast` holds a `TypeRefId`**, which is a *syntactic* type reference. `xx` has no
  syntax for its target, so it cannot reuse that field — which is what forces §2's HIR shape.
- **A `.` followed by a digit is a float**, by the lexer's existing rule. So `.5` and `.RED`
  are unambiguous without a new lexer rule, exactly as ADR-0041 §2 step 1 predicted.
- **`describe` and `no_such_member` already exist** in `jr-sema`, with a near-name suggestion
  (ADR-0041 §4). A bare member's "no such member" diagnostic is therefore the same one, which
  is the point of doing these together.

## Decision

### 1. One rule, stated once: a **context type** is a type an expression may ask for

Both features consume the same thing — the `expected: Option<PoolId>` that `check_expr`
already threads — and both fail the same way when it is absent. That is the whole reason they
are one ADR:

| Form | What the source omits | What the context supplies |
|---|---|---|
| `xx expr` | the target type of a conversion | the type to convert **to** |
| `.RED` | the enum the member belongs to | the namespace to resolve **in** |

**Neither invents a fallback when the context is absent.** No "default to `s64`", no "search
every enum in scope". An expression whose context is unknown is an **error** naming that fact,
because the alternatives are worse in both cases: a defaulted `xx` would silently convert to
something nobody wrote, and a searched `.RED` would resolve differently as the program grows a
second enum with a `RED`.

This is a deliberate contrast with ADR-0016 §1, where an integer literal with no context
*defaults* to `s64`. The difference is that `1` has a meaning without a type and `xx n` does
not: the literal's default picks a representation for a known value, where `xx`'s would pick
the value itself.

### 2. `xx expr` is a prefix operator whose target type comes from the context

```jr
n: s64 = 1;
b: u8 = xx n;              // narrows, exactly as cast(u8, n)
f: float64 = xx n;         // int → float
takes_u8(xx n);            // the parameter type is the context
```

Syntactically a **prefix operator**, at the same precedence as `-`, `!`, `~` and `*`, so
`xx n + 1` is `(xx n) + 1`. Not a call-like `xx(expr)`: Jai spells it as a prefix word and
there is nothing to parenthesise, since it takes no type argument.

**HIR gets `Expr::Autocast { operand, span }`** — deliberately *not* `Expr::Cast` with an
optional `TypeRefId`. An `Option<TypeRefId>` would make every existing consumer of
`Expr::Cast` handle a case where the target is unknown, and the two differ in exactly the
question this ADR is about: where the type comes from. A separate variant makes each site
choose.

**Sema types it by delegating to the *same* conversion check `cast` uses.** `check_autocast`
resolves nothing and instead:

1. reads `expected`; with `None`, raises **E0242** — "the target type of `xx` cannot be
   inferred here" — with a help naming `cast(T, x)` as the explicit form;
2. with `Some(target)`, types the operand and applies **ADR-0037 §2's rule unchanged**: both
   sides must be numeric, or the source an enum and the target numeric.

So `xx` is legal exactly where `cast` is legal, and *nowhere else*. That equivalence is the
design: `xx` is sugar for a `cast` whose type was already written down somewhere else in the
statement, and a reader can always mechanically recover the `cast`.

**MIR needs no new node.** `Expr::Autocast` lowers through the existing `cast` path, because by
then the target is the expression's type and the source is the operand's. This is the payoff for
ADR-0037 §2 having put the conversion in `Rvalue::Convert` with an explicit source kind.

**Rejected: `xx` converts anything, including a pointer or a `bool`.** Jai's `xx` is
broader — it will do pointer conversions the language otherwise refuses. Rejected because
ADR-0037 §2 refuses those *for `cast`*, and a sugar that is more powerful than the thing it is
sugar for is not sugar: it would be the only way to write a pointer conversion, which makes
`xx` load-bearing for a feature nobody decided to add.

**Rejected: `xx` on a literal.** `x: u8 = xx 300;` is refused, because a literal already takes
its type from context (ADR-0016 §1) — so `xx` adds nothing and would *suppress* the fit check
that makes `x: u8 = 300;` an error. Sema reports it as an unnecessary `xx` rather than
silently allowing a wrong value through, and that is a separate diagnostic (E0243) because
"remove the `xx`" is a different instruction from "add a type".

### 3. Bare `.RED` resolves in the enum the context names

```jr
c: Colour = .RED;          // the annotation is the context
if c == .GREEN { … }       // the other operand's type is the context
paint(.RED);               // the parameter type is the context
```

Exactly ADR-0041 §2's plan, and its five steps are what this implements:

1. **`MEMBER_EXPR`**, a prefix form, with `EXPR_START` gaining `DOT` and `is_expr_kind` gaining
   the kind. The lexer needs no change: a `.` starts a fraction only before a digit, so `.5`
   and `.RED` are already distinguishable.
2. **`Expr::Member { name, name_span, span }`** in HIR, resolving to nothing at lowering time —
   resolution needs a type, and lowering has none.
3. **A sema rule on the `expected` path**: with `Some(ty)` where `ty` is an enum (of either
   form — plain or `enum_flags`), look the name up in that enum's members and reuse
   `no_such_member`'s near-name suggestion; with `None` or a non-enum type, **E0244** saying
   *why* — "the enum a bare `.RED` belongs to cannot be inferred here" — rather than
   "unresolved name `RED`", which would send the reader looking for a declaration.
4. **Every `expected` site audited**, which reading settled: `check_operands` passes the other
   operand's type, and a call argument passes the parameter type, so the two forms a Jai
   programmer tries first both work without new plumbing. Each has its own corpus case anyway,
   because "the context reaches it" is the claim being made.
5. **`.RED` in a `switch` stays deferred**, because there is still no `switch`. ADR-0041 §2 said
   W2 or later would answer this; that remains true and is not pre-empted here.

**MIR reuses the enum member fold.** `Colour.RED` already folds to a constant on the value path
(ADR-0041 §5); a bare `.RED` produces the same constant at the same type. The difference is
entirely in *how the enum was found*, which sema has already settled by the time MIR runs —
so `jr-mir` sees an expression whose type is the enum and whose member is known, exactly as it
does for the qualified form.

**Rejected: allow `.RED` to search all enums in scope when there is no context.** Convenient,
and it makes `n := .RED;` work. Rejected because it is a resolution rule whose answer changes
when an *unrelated* enum gains a member of the same name — a program that compiles today
breaking because a different type grew a `RED` is the worst kind of action at a distance.

**Rejected: `.RED` only in an annotated declaration**, refusing the comparison and call forms as
a smaller first step. Rejected because those two are precisely where the form is worth having
(`if c == .GREEN` is the idiom), and because the plumbing turned out to already exist — a
narrower version would have been more code, not less.

### 4. Both diagnostics name the explicit form, because that is the actionable part

- **E0242** (`xx` with no context): help is `cast(T, x)`.
- **E0243** (`xx` on a literal): help is to delete the `xx`.
- **E0244** (`.RED` with no context or a non-enum context): help is `Colour.RED`.

ADR-0043 established that a diagnostic can be accurate and useless. "Cannot infer" is accurate
and useless on its own; the recoverable fact is that an explicit spelling exists and always
will (ADR-0041 §2's "why it is safe to defer" argument, now paying off in the diagnostic).

For E0244 the context type is named when there *is* one: "expected `s64`, and a bare member
needs an enum" is a different problem from having no context at all, and conflating them would
misdirect the reader.

### 5. The two features compose, and that is checked rather than assumed

`xx` produces a value of the context's type; `.RED` resolves in the context's namespace. So
`f(xx n)` and `f(.RED)` work for the same reason, and a corpus program exercises both against
the *same* call site to prove the context reaches arguments once rather than twice.

They do **not** nest: `xx .RED` is refused, and it falls out rather than needing a rule.
`check_autocast` types its operand with **no expectation** — it must, because the operand's own
type is the conversion's *source* and taking it from the context would make every `xx` a no-op —
so the inner `.RED` finds no context and raises **E0244**.

That is the right refusal by a slightly different route than "the conversion is a category
error", and the difference is worth recording because the diagnostic a user sees is about the
member rather than about the `xx`. It is still the *actionable* one: naming the enum
(`xx Colour.RED`) is what makes the expression well-formed, and E0244's help says exactly that.

### 6. Operator overloading is **not** in this wave, and that is a scope decision

`PLAN.md` §2.1 lists it beside `xx` in W1. It is left out, and the reason is that it shares
nothing with these two: overloading is a *name resolution and dispatch* feature — it needs a
rule for which `operator +` is visible, whether it can be defined for a type you do not own,
how it interacts with the trapping/wrapping split (ADR-0002), and whether `==` on a view
(refused in ADR-0044 §5) becomes user-definable. Every one of those is a separate argument.

Bundling it here to "finish W1" would produce exactly the failure `AGENTS.md` names for plans:
a wave whose scope was set by a checklist rather than by what the decisions actually cost.
**W1 therefore ends with operator overloading outstanding**, recorded as its own ADR-to-be
rather than as a loose end.

### 7. A bare member reaches an *imported* enum, and the qualified form does not

Discovered by running it rather than reasoned about, and recorded because the asymmetry is
surprising in the direction nobody would predict.

```jr
#import "Shapes";           // declares Colour :: enum { RED; GREEN; }
c: Colour = .GREEN;         // works, and is 1
d := Colour.GREEN;          // internal compiler error
```

The bare form works **because** it resolves through the context type: sema takes the enum from
`expected`, and `jr-mir` folds the member out of the pool's member table keyed on `DeclId` —
neither step cares which file declared it. The qualified form goes through
`enum_member_of`, which handles `Res::Item` only and deliberately refuses `Res::Imported`
(ADR-0041 names this, on the grounds that an `EnumId` indexes another file's arena).

So this wave *widened* what works, accidentally and correctly. But the qualified form's refusal
surfaces as **`internal compiler error: no routine for file 0 proc 0`** rather than as a
diagnostic — a refused body reaching the runner. That is a second bug behind the same symptom
and it is worse than the first: a crash tells a user nothing. Both are recorded in `PLAN.md` §7
with the ICE named as the thing to fix first.

Nothing is changed here for it. Fixing the ICE is a `jr-db`/`jr-cli` concern about how a refused
body is reported, and widening `enum_member_of` needs the cross-file reasoning ADR-0018 §5
established for callees — neither belongs in an ADR about context typing.

## Consequences

- **`XX_KW` leaves the reserved block**, and the tree-sitter reserved match with it — the
  fourth keyword to make that trip after `cast`, `enum` and `union`. Checked in advance now
  that §7 lists it as a standing trap.
- **`DOT` joins `EXPR_START`**, which is the token-set predicate trap that has swallowed three
  features (`CAST_KW`, `L_BRACK`, `TILDE`). ADR-0045 found `TYPE_START` missing *three*
  keywords when only one was being added, so the neighbours get checked here too.
- **Three new diagnostic codes** — E0242, E0243, E0244 — making **E0245 the first free code**.
- **`jr-fmt` needs two emitter arms and two predicate entries.** `xx` is a prefix operator, so
  it joins the unary emitter's token list; `MEMBER_EXPR` needs its own arm. Five waves running
  now, so this is a checklist item rather than a discovery.
- **No new MIR, no new `Item`, no layout change.** Both features are resolved entirely in sema
  and lower through paths that already exist — which is the strongest evidence that they were
  the right pair to do together, and a marked contrast with the last four waves.
- **`Expr::Autocast` and `Expr::Member` are two new HIR variants**, so every exhaustive match
  over `Expr` gains an arm: `jr-hir`'s dump and resolve, `jr-sema`'s `check_expr`, `is_place`
  and `is_untyped_literal`, `jr-mir`'s `scan`, `expr`, `place` and `escape`, and `jr-mir`'s
  thunk. The compiler lists them.
- **`is_untyped_literal` must answer `false` for both.** An `xx` has a real type (the
  context's) and so does a `.RED`; answering `true` would make them take the *other* operand's
  type in a binary expression, which is a second context-typing rule fighting the first.

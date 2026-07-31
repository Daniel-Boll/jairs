# ADR-0052: `-> (s64, bool)` returns a structural results aggregate; `_` discards one

- **Status:** Accepted
- **Date:** 2026-07-30
- **Deciders:** dboll
- **Depends on:** ADR-0051, whose `sret` convention carries the results aggregate with **no
  back-end change**. This wave is only possible because that one happened first.
- **Amends ADR-0008 §"Follow-on work"**, which put multiple return values *and* `#must` in W2
  together. `#must` is deferred to its own ADR; §5 argues why and records the amendment rather than
  shipping half of a commitment silently.

## Context

`PLAN.md` §2.1 lists multiple return values in W2, and ADR-0008 chose Jai's error model — multiple
returns plus `#must` — over exceptions and over a `Result` type, so this is the feature that model
rests on. §7 called it "blocked on aggregate returns in the native back end" until ADR-0051 landed.

Six facts were established by reading the code before this ADR was written, and four shaped the
decisions.

- **`ProcSig::ret` is one `PoolId`**, documented as "always a real type; `PoolId::VOID` when the
  source omitted the arrow — never `None`, per ADR-0015 §3". **This is the fact that decides §1**: a
  procedure returning several values still has *one* return type, so nothing in the signature
  representation has to change shape.
- **ADR-0051 returns any aggregate through a caller-allocated `sret` pointer**, and
  `repr::returns_via_sret` keys off `Repr::is_aggregate` alone. So a results aggregate is carried by
  machinery that already exists and is already differentially tested.
- **A struct's identity is a nominal `DeclId`** (ADR-0015 §1), while `ViewType` and `PointerType`
  carry their element types directly and are *structural*. **This decides §1's spelling**: an
  anonymous results type has no declaration site, so it must be structural — `(s64, bool)` written
  in two files is one type.
- **`Stmt::Assign` has one `lhs`**, and `Stmt::Local` declares one local. **This decides §2**: the
  destructuring forms are new statement variants rather than a generalisation of these, because a
  list of targets is a different shape from a single target.
- **`jr-vm` already returns aggregates by value** and needed no change in ADR-0051. It needs none
  here either, which makes the differential harness the check again.
- **`Res` has no variant for a name that binds nothing**, so `_` needs one — or a rule that keeps it
  out of the resolve map entirely. §3 chooses the latter.

## Decision

### 1. `-> (T, U)` interns as a **structural results aggregate**

```jr
divide :: (a: s64, b: s64) -> (s64, bool) {
    if b == 0  return 0, false;
    return a / b, true;
}
```

`Item::ResultsType { elems: Vec<PoolId> }` — structural, interned on the element list, so
`(s64, bool)` written in two files is one type and no `DeclId` is invented for something with no
declaration site.

**Its layout and field access are a struct's**, reusing `jr-pool`'s existing computation with fields
named `0`, `1`, … . That is the whole reason this is cheap: `layout_of`, `Repr::of`, the VM's place
steps, `jr-codegen-clif`'s projections and ADR-0051's `sret` path all treat it as the aggregate it
is, and none of them needed teaching.

**Rejected: a real `Item::TupleType` with first-class tuple semantics.** A tuple you can pass, store
in a variable and index would be a *language feature* — `t := divide(7, 2); t.0` — and it is a
different one from "a procedure may return several values". Jai has the second without the first, and
adding the first here would mean deciding tuple equality, tuple literals and whether a tuple is a
valid field type. `ResultsType` is deliberately **not** spellable as a variable's type (§4).

**Rejected: one hidden out-pointer per result.** `f(out0: *s64, out1: *bool)` needs no aggregate at
all, and it makes the ABI's parameter count depend on the result count — so the hidden-parameter
offset that ADR-0051 warned shifts every argument becomes variable rather than 0-or-1. It also makes
the result never a single value, which forecloses ever writing `x := f()` for a one-result procedure
without a second mechanism.

**A one-element `-> (T)` is exactly `-> T`.** Interning normalises it, so there is no
distinction to explain and no way to write a "1-tuple" that behaves differently from the scalar.

### 2. Two destructuring forms, both requiring exact arity

```jr
q, ok := divide(7, 2);      // declares both
q, ok = divide(9, 3);       // assigns to both, already declared
```

`Stmt::LocalTuple` and `Stmt::AssignTuple`, new variants rather than a generalised `lhs`, because a
*list* of targets is a different shape and every exhaustive match over `Stmt` should be forced to
consider it.

**Exact arity, checked in sema (E0251).** Two names for a three-result procedure is an error, and so
is one. The alternative — Jai's, which lets a caller take a prefix — was rejected for a specific
reason: it makes adding a result to a procedure silently change nothing at any call site, and
*reordering* results silently change what every caller binds. That is action at a distance, the same
objection ADR-0014 §3 made about import order and ADR-0048 §3 about `#import` redefining `+`.

**A multi-result call is not a subexpression.** `f(divide(7, 2))` and `divide(7, 2) + 1` are refused,
because a results aggregate is not a value the expression grammar can carry — §4 keeps it
unspellable, and this is what that costs. The right-hand side of a destructuring statement is the
only position a multi-result call may appear in.

### 3. `_` discards a result, and is not a binding

```jr
q, _ := divide(7, 2);       // the flag is discarded
_, ok := divide(7, 2);      // the quotient is discarded
```

**`_` is not in the resolve map and declares no local.** It is a *hole in a target list*, recognised
positionally by the destructuring statement, so it never becomes a name anything can refer to. That
is the cheap half of the decision and it is what keeps `Res` unchanged: there is no
`Res::Discarded`, because a discarded position resolves to nothing at all.

**Writing `_` as an expression is an error**, and it must be, or `x := _;` would ask for the value of
a hole. E0251 covers it.

**`_` is still available as an ordinary identifier elsewhere**, deliberately: it is a legal name in
Jairs today, and reserving it globally would be a lexical change breaking any program using it.
So `_` is special *only* in a destructuring target list, which is a positional rule rather than a
name rule — the distinction that keeps it out of the resolve map.

**Rejected: refusing discards entirely.** Simpler, and it forces a name for every result a caller
does not want, which is exactly the friction that makes people ignore an error flag by binding it to
`unused`. A discard that is *visible in the source* is better than a binding nobody reads.

**Rejected: allowing every position to be `_`.** `_, _ := divide(7, 2);` is legal under §3 as
written, and deliberately so — it is a call whose results are all discarded, which is what a bare
`divide(7, 2);` statement already means. Refusing it would need a rule distinguishing "all holes"
from "some holes", for no benefit.

### 4. A results type is not spellable, and not storable

`(s64, bool)` may appear **only** after `->`. There is no `t: (s64, bool)` variable, no parameter of
that type, no field of it, and no `cast` to it.

This is what keeps §1's "reuse the struct machinery" from turning into "tuples exist now". A results
aggregate is a *transport*: it comes into being at a `return` and is destructured at the call. Making
it storable would raise every tuple question §1 rejected, and none of them is answered here.

**The consequence is stated rather than discovered:** a procedure cannot pass several results
through to its own caller without destructuring and re-returning them. `return divide(7, 2);` is
refused even when the signatures match, because that would be a results aggregate flowing as a value.
Recorded as owed; it is a real limitation and the natural fix — allowing a bare multi-result call as
a `return` operand — is a small, separate decision.

### 5. `#must` is deferred, and this amends ADR-0008

ADR-0008's follow-on list says "**Into wave W2:** multiple return values and `#must` land with the
flow-and-scope wave". This ADR does the first and defers the second, in writing, because they are two
decisions:

- multiple returns is a **calling convention** — how several values travel;
- `#must` is an **error-handling policy** — when *not using* a value is a compile error, which needs
  its own answer to "what counts as using it". Does `_` satisfy `#must`? (Almost certainly not, and
  that is the interesting question.) Does assigning to a variable nobody reads?

Bundling them would put a convention and a policy in one ADR, which is the plan-contradiction shape
`AGENTS.md` names. ADR-0008's *choice* of error model is untouched — Jairs still has no exceptions
and no `Result` type — and `#must` gets a new ADR whose whole subject is the policy.

## Consequences

- **`Item::ResultsType` is the first structural aggregate**, so `layout_of` gains a case that
  delegates to the struct computation rather than duplicating it. A duplicated layout would be a
  silent wrong offset, which is why it delegates rather than repeats.
- **`Stmt` gains two variants**, so every exhaustive match over it changes: `jr-hir`'s dump and
  resolve, `jr-sema`'s checker, `jr-mir`'s `scan`, `stmt` and the escape walk. The compiler lists
  them, which is the mechanism ADR-0050 §2 relied on.
- **The parser needs `(` after `->`**, which is a token-set change to the return-type position —
  and `TYPE_START` has been missing a token in two previous waves (ADR-0045, ADR-0049). Checked
  rather than discovered.
- **One new diagnostic code, E0251**, covering arity mismatch, `_` as an expression, a multi-result
  call in an expression position, and a results type written as a variable's type — four refusals
  with distinct notes. **E0252 is the first free code**; the parser needs E0129 for a malformed
  results list.
- **`jr-fmt` needs the results list and both destructuring forms**, and the formatter has deleted or
  mangled a construct in **four consecutive waves**. A test must assert survival *and*
  canonicalisation, because the round-trip gate passes for a formatter emitting raw text (ADR-0049's
  lesson, restated because it keeps being the same trap).
- **`jr-vm` needs no change**, so the differential harness is again the only check that the two
  engines agree — which makes a corpus program returning several values and reading both back
  mandatory rather than optional (the ADR-0051 §3 obligation, inherited).
- **`_` costs nothing in the resolve map**, which is the payoff for making it positional. The cost is
  that `_` means something different inside a target list than outside one, and that must be in the
  spec chapter rather than only here.

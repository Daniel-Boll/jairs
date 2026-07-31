# ADR-0053: A named argument matches a parameter name; a default must be a literal

- **Status:** Accepted
- **Date:** 2026-07-30
- **Deciders:** dboll
- **Scope:** Named arguments at a call site and default values in a declaration. The last W2 feature
  besides `#scope_*`.
- **Corrects `PLAN.md` §7**, which said named arguments "interacts with overload resolution
  (ADR-0048 §4's exact-match rule)". It does not — see §5. The claim was inherited from one handoff
  to the next and was never true; it is corrected here and in the §7 this wave rewrites.

## Context

`PLAN.md` §2.1 lists named and default arguments in W2. Reading the code before writing this ADR
turned up six facts, and three of them changed the design.

- **`Item::ProcType` carries `params: Vec<PoolId>` — types only, no names.** **This is the fact that
  decides §1**: a call site matching `b = 2` against a parameter has nothing to match *against* in
  the interned type, so the names must come from somewhere else.
- **`ProcSig` is keyed by `ProcId` and already sits beside the interned type**, holding
  `params: Vec<PoolId>`, `ret` and `ty`. It is per-procedure rather than per-type, which is exactly
  the right grain: two procedures with the same signature have different parameter *names*, and
  interning them to one type is correct.
- **Const-eval runs upstream of checking** (ADR-0018 §3: it lives in `jr-db` over the bytecode VM,
  downstream of *signatures* but before `checked`). **This decides §2**: a default that is an
  arbitrary constant expression cannot be evaluated during the check that needs it without making
  signature resolution depend on const-eval, which already depends on signatures.
- **Operator overloads never reach `check_call`.** `find_operator` resolves them on
  `(op, lhs, rhs)` and records the answer in `CheckOutput::operator_calls`; a binary expression is
  not a call expression. **This is what falsifies §7's claim** and is why §5 exists.
- **`check_call` compares `args.len() != params.len()` and then zips.** So named arguments are a
  *reordering step* inserted before that zip, not a rewrite of call checking.
- **`jr-mir` lowers a call by mapping `args` in source order.** Reordering must therefore happen
  before MIR, or MIR would need to know about names — which it must not, because two passes deciding
  argument order is two chances to disagree.

## Decision

### 1. `name = value` at a call site, matched against `ProcSig`'s parameter names

```jr
draw :: (x: s64, y: s64, colour: s64 = 7, scale: s64 = 1) { … }

draw(1, 2);                   // colour and scale default
draw(1, 2, scale = 3);        // colour defaults, scale named
draw(x = 1, y = 2);           // all named
draw(y = 2, x = 1);           // order need not match
```

`ProcSig` gains `names: Vec<Symbol>`, parallel to its existing `params: Vec<PoolId>`. That is where
the names live because it is already the per-*procedure* record, while `Item::ProcType` is the
per-*type* one — and two procedures with identical types genuinely do have different parameter
names, so putting names in the interned type would either break interning or lie about one of them.

**Sema rewrites the argument list into positional order before anything else sees it.** The
reordered list is recorded in `CheckOutput`, and `jr-mir` reads it rather than the source order. One
pass decides argument order; MIR never learns what a name is. That is the same split ADR-0048 §5
made for overloads — "MIR must not re-run resolution … two implementations of one rule are two
chances to disagree" — applied to the same kind of problem.

**Rejected: desugaring named arguments during HIR lowering.** Lowering would have to know each
callee's parameter names, which means resolving the callee — and lowering runs *before* resolution.
It would also make the HIR no longer a faithful record of what was written, which is what hover and
goto-definition read.

### 2. A default must be a **literal**

```jr
f :: (a: s64, b: s64 = 10) { … }        // legal
g :: (s: string = "none") { … }         // legal
h :: (n: s64 = SIZE) { … }              // E0252
i :: (n: s64 = 2 + 3) { … }             // E0252
```

An integer, float, string or boolean literal, and nothing else. Sema reads the literal directly and
interns it, with **no const-eval involvement at all**.

The reason is a layering constraint rather than a preference. ADR-0018 §3 put const-eval in `jr-db`
over the VM, *downstream of signature resolution*, so that types are known before constants are
computed. A default that could be `SIZE` would make a signature depend on a constant's value, and
that constant's own type depends on signatures. The cycle is not hypothetical — it is the one
ADR-0018 §3's ordering exists to prevent, and ADR-0039 §3a already records the same shape for an
array length: "`jr-sema` has no constant evaluator … so sema cannot *compute* `COUNT`".

**The refusal names what would be needed**, rather than saying "unsupported": a reader who writes
`= SIZE` should learn that the value must be a literal *and* why, or they will try `= 2 + 3` next.

**Rejected: any constant expression, via a new `jr-db` query.** A `file_param_defaults` query
running before `checked` would make `= SIZE` work. Rejected because it inverts the dependency
ADR-0018 §3 established, and the payoff is small: a literal covers the overwhelming majority of real
defaults. Recorded as owed, with the note that lifting it is an ADR about *query ordering* rather
than about arguments.

**A default's type must match its parameter's**, checked as an ordinary literal-against-type fit
(ADR-0016 §1, ADR-0038's range check). `b: u8 = 300` is the existing E0204, not a new code.

**Defaults need not be trailing.** `f :: (a: s64 = 1, b: s64)` is legal, and `f(b = 2)` is the only
way to call it — which is fine, because §3's rules make that unambiguous. Requiring defaults last
would be a simpler rule and it would forbid a signature that means something perfectly clear.

### 3. Positional arguments first, then named; every parameter supplied exactly once

Four rules, all checked in sema, all E0252:

- **A positional argument may not follow a named one.** `f(a = 1, 2)` is refused. Permitting it
  would make a positional argument's meaning depend on which names preceded it, so reading a call
  would require knowing the signature and counting.
- **No parameter may be supplied twice.** `f(1, a = 2)` is refused where `a` is the first parameter.
- **No parameter may be left unsupplied** unless it has a default. This is the existing arity error
  generalised: "takes 4 arguments, 2 supplied" becomes "`y` has no value and no default".
- **A named argument must name a parameter that exists.** `f(colur = 1)` is refused, **with a
  near-name suggestion** — the same `did you mean` machinery E0212 and E0218 already use (ADR-0031
  §1), because a misspelled parameter name is exactly the case it was built for.

### 4. What is deliberately absent

- **No named arguments in an operator overload call.** `p + q` has no argument list to name into.
- **No `#must`-style marker on a parameter**, and no way to require that an argument be named.
- **No defaults on a `#foreign` procedure.** Its parameters are the C function's, Jairs does not
  control its call sites, and a default would be a Jairs-side fiction the FFI boundary cannot honour.
  Refused with that reason.
- **No named arguments through a procedure pointer.** `Callee::Indirect` is already refused
  everywhere, so this adds nothing new.

### 5. Why there is no overload interaction, correcting `PLAN.md` §7

§7 has said, for one wave, that named arguments "interacts with overload resolution (ADR-0048 §4's
exact-match rule)". Reading the code shows it does not:

- an **operator** overload is resolved by `find_operator` from `(op, lhs, rhs)` and never passes
  through `check_call`, because a binary expression is not a call expression;
- Jairs has **no procedure overloading at all** — ADR-0048 §1 overloads *operators* specifically, and
  ADR-0014 §3's flat name map gives one procedure per name, with two imports of one name being E0211
  at the use site.

So there is no candidate set for named arguments to disambiguate, and no ranking rule for them to
interact with. The claim was carried forward from handoff to handoff without being checked, which is
the rot `AGENTS.md` names — recorded here because the correction is more useful than the erasure.

**What named arguments *do* interact with** is `jr-mir`'s argument lowering (§1), and that is the
part worth being careful about.

## Consequences

- **`ProcSig` gains `names`**, so every construction site must supply them. That is a compile error
  at each one, which is the point.
- **`CheckOutput` gains a reordered-argument map**, and `jr-mir` reads it instead of the HIR's source
  order. A call with no named arguments has no entry, so the common path is unchanged and pays
  nothing.
- **One new diagnostic code, E0252**, covering five refusals with distinct notes: a non-literal
  default, a positional argument after a named one, a duplicate parameter, a missing parameter with
  no default, and an unknown parameter name (with a suggestion). **E0253 is the first free code**;
  the parser needs E0130 for a malformed named argument.
- **`jr-fmt` needs `name = value` in an argument list and `= literal` in a parameter.** The formatter
  has lost or mangled a construct in **five consecutive waves**, most recently deleting a whole
  result list and truncating a multi-value return. A test must assert survival *and* canonicalisation.
- **A default is evaluated at the call site, not once at the declaration.** For a literal there is no
  observable difference, which is a further argument for §2's restriction: the moment a default could
  be `f()`, "when does it run" becomes a question this ADR would have to answer.
- **`jr-vm` and `jr-codegen-clif` need no change at all.** By the time MIR exists the arguments are
  positional, which is the whole point of §1's reordering — and is worth stating as evidence the
  split is right rather than as a convenience.

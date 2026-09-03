# ADR-0192: `type_of(x)` — the type of a value

- **Status:** Accepted
- **Date:** 2026-09-03
- **Deciders:** dboll

## Context

Reading the three real Jai repositories for ADR-0185 counted `type_of` **14 times**, making it the
fourth most used construct Jairs lacked. It is the inverse of every other type argument: `size_of(T)`
takes a type and gives a number, `type_of(x)` takes a value and gives a type. Its main use is inside a
polymorphic procedure, where the parameter's type has no spelling at the call site and nothing else can
name it.

## Decision

### 1. One arm in `described_type`, so four intrinsics gain it at once

`described_type` is the single function every intrinsic asks for its type argument. `type_of` is
matched there, ahead of the parameterised form, and its operand is checked as a **value** — the one
type argument whose own argument is not a type. So `size_of(type_of(x))`, `type_info(type_of(x))` and
`any_as(a, type_of(x))` all work from one arm, and a nested `Slot(type_of(a), type_of(b))` does too,
because the parameterised arm resolves its arguments through this same function.

`type_of(s64)` is refused by answering `None`, which lets the enclosing intrinsic raise its own E0261.
No new code: the objection is "that is a type, not a value", which is what E0261 already says.

### 2. The obvious fix was unnecessary *and* worse, and writing it is how that was found

`in_type_info_argument` is sticky through a nested call — ADR-0119 §2's rule, because "a type
argument's own arguments are types all the way down". `type_of` looked like the exception, so the first
change cleared the flag for its operand.

**It was not the fix.** A local is resolved to `Res::Local` during *lowering*, so the flag never
decided anything for `type_of(n)`; the refusal was coming from MIR (§3). And clearing it was actively
worse: `type_of(s64)` then reported `unresolved name s64` — a name that is perfectly well known, which
is the exact complaint `described_type`'s own doc comment makes about its predecessor — *on top of* the
honest E0261. Left sticky, that case is one clean diagnostic.

Both the flag change and the `callee_is_type_of` helper written to support it were removed. The
sequence is worth recording: a plausible diagnosis, a change that made the probe pass, and a *second*
probe — the refusal case — showing the change was the wrong one.

### 3. MIR's exemption list became a property

`scan` refuses a body containing an unresolved name, and exempts the callee of a call that folds to a
constant. That exemption was a list of five features — `#run`, `any_of`/`any_as`, an instantiation,
`typed`/`untyped`, the atomics — each added by a wave that discovered the omission the same way:
`the compiler could not lower main (a name failed to resolve)` on an obviously fine program.

`type_of` does not fold to a *value*, so no entry would have helped. What is true of it is that **its
call denotes a type**, and the type map already knows that. So the condition gained
`types.expr_type(scope, call) == Some(PoolId::TYPE)` — a property that covers any future type-denoting
call without an entry, beside a `denotes_a_type` check the same function already applies to a bare type
*name*. `is_intrinsic_name` in `jr-hir` still needed its entry, and that list is the **sixth** such
addition; its own doc comment predicts this failure and has not prevented it.

### 4. Poison propagates rather than refusing, which is what makes the polymorphic case work

Inside a `$T` template the parameter's type is `PoolId::ERROR` until a call site binds it — measured,
not assumed. Every other operation on such a value is silently tolerated: `v.x` and `cast(s64, v)`
report nothing in a template body (ADR-0017 §4).

`type_of` now follows the same discipline and returns the poison, and `check_size_of` gained the
matching tolerance: a described type of `ERROR` returns quietly instead of reporting `size_of cannot
measure <unknown>`. That arm sits two lines below one that already does exactly this for the *name* `T`
inside a `$T` body — the same situation reached one level deeper, and the precedent was what said which
way to fix it.

Without both halves, `size_of(type_of(v))` reported an error on a template every instantiation types
perfectly: an error about the compiler's own placeholder, shown to the author.

### 5. A checksum that lands on zero proves nothing

`valid/143`'s total is 15060, and its first version exited `total % 251` — which is **exactly 0**,
because 251 × 60 = 15060. An exit code of 0 is what a program that did nothing also produces, so the
checksum would have passed a completely broken `type_of`. It now asserts the total and exits 77.

Recorded because every corpus file here uses this idiom: **check the modulus before trusting it.**

## Consequences

- `type_of(x)` works for any value — scalar, aggregate, `string`, pointer — and inside a `$T` template,
  which is where real Jai code uses it.
- The identity is the interned type, not an equivalent: `valid/143` asserts `type_of(n).id` equals
  `type_info(s32).id` **and** differs from `type_info(u32).id`, so an implementation keyed on width
  alone fails.
- `size_of` no longer reports an error on a poisoned described type, which is a general improvement
  beyond `type_of`.
- Still owed: `x : type_of(y);` in a *type annotation* position. That needs the parser to accept a call
  in a type, which is a `TypeRef` change rather than an intrinsic one, and nothing has wanted it —
  every counted Jai use is inside another intrinsic.

## Alternatives considered

**A dedicated diagnostic for `type_of` on a type.** Rejected: E0261 already says "a type used where a
runtime value is expected", which is exactly the objection. A second code would be a promise that
something else is checked.

**Making a `$T` parameter's type a type-variable `PoolId` rather than `ERROR`.** Rejected as out of
scope, and it is the wrong lever: the template pre-check is *meant* to be quiet about a type nothing
has bound, and poison is how every other operation there stays quiet.

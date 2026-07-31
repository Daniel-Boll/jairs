# ADR-0038: a leading `-` on a literal is folded during lowering

- **Status:** Accepted
- **Date:** 2026-07-28
- **Deciders:** dboll
- **Amends:** ADR-0016 §1, which fixed context typing for integer literals but did not say
  where the sign belongs.

## Context

`a: s8 = -128;` is rejected, by a diagnostic that prints the range it falls inside:

```
error[E0204]: integer literal does not fit `s8`
  = an integer literal takes its type from its context, which here is `s8`
  = the range of `s8` is -128 to 127
```

Every signed width has this, at exactly its minimum. It is a **wrong diagnostic on correct
code**, which is why it was taken ahead of the three W1 features `PLAN.md` §7 lists beside it.

The cause is that `-128` is not a literal today. It is `Unary(Neg, Literal(128))`, and
`jr-sema`'s `literal_fits` tests the *magnitude* 128 against `max_magnitude(signed: true,
bits: 8)`, which is 127. The two comments in `jr-mir` state the invariant plainly — "`value`
is a magnitude: `-1` is `Neg` applied to `1`, so no sign is reconstructed here" — so this is a
deliberate design that turns out to be wrong at one value per signed type.

**It is not a one-line fix, and checking that is what shaped this ADR.** Two more things break
at the same value:

- `jr_pool::int_negate` **traps** on negating 128 in an `s8`: the negation of the maximum
  magnitude is one past the maximum, and ADR-0002 makes that a trap. So a fix that only taught
  sema to accept `-128` would move the failure from compile time to run time — a *worse*
  outcome, and one the constant-folder would hit too, since `constprop` calls the same
  `int_negate`.
- The literal was recorded with `overflowed: value > i64::MAX`, which for a negated literal is
  the wrong bound: `-9223372036854775808` has magnitude `9223372036854775808`, which is
  `i64::MAX + 1`, so it is flagged as overflowing `s64` when it is exactly `s64::MIN`.

The bug predates wave W1 (reproduced on the commit before it) and was invisible because no
corpus file wrote a signed minimum.

## Decision

### 1. Lowering folds a leading `-` into the literal it applies to

`jr-hir`'s `lower_expr` recognises `Unary(Neg, Literal(Int))` and produces a single
`Literal::Int` carrying the negative value. Nothing downstream sees a negation of a literal:
sema's fit check receives −128 and compares it against `s8`'s *range*, MIR interns a negative
constant directly, and `int_negate` is never called on a literal at all.

This is what rustc and most compilers do, and the reason is the one above: the minimum of a
two's-complement type is not expressible as the negation of anything the type can hold, so the
sign has to be part of the literal or the minimum is unreachable.

**Rejected: pass a "negated" flag into `check_int_literal`.** Smaller — one parameter and one
branch — and it fixes the *diagnostic*. Rejected because it fixes only the diagnostic: the
constant still reaches MIR as `Neg` applied to 128, and `int_negate` still traps at run time
and in `constprop`. Trading a wrong compile error for a wrong run-time trap is not a fix, and
it is exactly the kind of half-change that leaves a plausible-looking green build.

**Rejected: require `cast(s8, -128)`.** No compiler change, and it makes a legal-looking
program need a cast that no other language asks for — while leaving the diagnostic wrong.

### 2. `Literal::Int` carries a signed value, not a magnitude

`value: u64` becomes `value: i128`. The width is the interesting part:

- **Signed**, because that is the whole point of this ADR.
- **128 bits**, because `u64::MAX` and `i64::MIN` must both be representable and no 64-bit
  integer type holds both. `i128` is what `jr-pool`'s `IntKind::min`, `max`, `decode` and
  `check` already use for exactly this reason, so this makes the literal agree with the
  arithmetic rather than introducing a new width.

The two `jr-mir` comments asserting "`value` is a magnitude" are deleted rather than reworded:
a comment describing the previous design is worse than none.

`overflowed` keeps its meaning — "this literal does not fit any Jairs integer type" — but is
now computed against the `i128` value rather than `i64::MAX`, so a negated minimum is not
flagged.

**Rejected: keep `u64` and add a `negative: bool`.** Two fields that must agree, with
`(negative: true, value: 0)` a second spelling of zero. A single signed value cannot
disagree with itself.

### 3. Only a literal *directly* under the `-` is folded

`-x`, `-(1)` and `-f()` are untouched: they lower to `Unary(Neg, …)` exactly as before, and
`int_negate` still applies with ADR-0002's trap. The fold is syntactic and one level deep.

`-(128)` therefore still fails for `s8`, because the parenthesised expression is a
`PAREN_EXPR` rather than a literal. That is a real inconsistency and it is accepted knowingly:
recognising a literal through arbitrary parentheses means the fold has to walk them, and
`-(128)` as an `s8` is not code anyone writes. Recorded so it is a known edge rather than a
discovery.

### 4. `is_untyped_literal` still answers `true` for a negative literal

It has to: ADR-0016 §1's context typing is what makes `a: s8 = -128;` take `s8` from its
annotation in the first place. Previously the predicate reached the literal through its
`UnOp::Neg` arm; now the literal *is* the node, and the `Neg` arm remains for the non-literal
cases §3 keeps.

## Consequences

- **The minimum of every signed type becomes writable**, which is eight values that were
  unreachable. `tests/corpus/valid/027-numeric-tower.jr` was written one step above each
  minimum *because* of this bug, with a comment saying so; it now sits on the minimums.
- **`type-errors/021-signed-minimum-literal.jr` is deleted, not updated.** It existed to pin
  the wrong behaviour and named itself as pinning a bug. A corpus file asserting a fixed bug is
  a test that fails for the right reason and then gets "fixed" by someone reading it as
  intent.
- **The MIR snapshots change**, because a negated literal is now one constant rather than a
  `Neg` of another. That is a smaller MIR for every negative literal in the corpus, which is
  incidental and not the point.
- **`int_negate` keeps its trap** and keeps its callers. Nothing about ADR-0002 changes; the
  fold means a *literal* no longer reaches it.
- **What is still not fixed:** `print_int` in `modules/Basic` still traps on `s64::MIN`,
  because it negates a runtime value rather than a literal. That is `-%`'s or an unsigned
  path's problem and the module says so.

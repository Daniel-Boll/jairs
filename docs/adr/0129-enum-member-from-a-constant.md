# ADR-0129: an enum member's value may name a literal-valued constant — generalising ADR-0070

- **Status:** Accepted
- **Date:** 2026-08-12
- **Deciders:** dboll
- **Wave 2 of eight.** Wave 1 was ADR-0128. **This is not one of ADR-0127 §3's six unkept promises** —
  those are `[..]T`, `it`/`it_index`, `$$T`, instantiation backtraces, `Math` vec/mat/quat and nested
  declarations. It is the *generalisation* ADR-0127 §2 recorded as owed while correcting E0237's note, and
  it is called out separately here because miscounting the project's own ledger is the failure ADR-0127
  existed to fix. No design fork was put to the decider, because the shape was fixed in advance: ADR-0070
  already decided what "a readable constant" means, and this wave moves a second caller onto that decision
  rather than making a new one. §4 records the one question that *would* have been a fork, left open
  rather than answered quietly.
- **Generalises ADR-0070**, which taught an array length to read a named constant. It does not amend it:
  ADR-0070's line — "is the value already a literal, one name away" — is unchanged, and this ADR moves a
  second caller onto the same side of it.

## Context

### The asymmetry was recorded, and it was not the evaluator's fault

E0237 refused this:

```jr
NOT_FOUND :: 404;
Status :: enum {
    MISSING :: NOT_FOUND;    // E0237: an enum member's value must be an integer literal
}
```

while ADR-0070 had made the exactly parallel `buf: [N]s64` legal a wave earlier. ADR-0127 §2 left the gap
recorded in the source itself, in E0237's own note: an array length had learnt the trick and an enum
member "has not learnt the same trick, which is a generalisation owed rather than a limit of the
evaluator".

That note is why this wave was cheap. ADR-0127 §3's six promises each needed a probe to locate — ADR-0128's
took one because the machinery *looked* present. This gap had a comment pointing straight at it, and the
cost of closing it was one arm of a `match`. A deferral that says what is owed, rather than when it
arrives, is the difference; that was ADR-0127 §2's whole argument, and this is the first evidence it
pays.

### What the ordering constraint actually forbids

ADR-0018 §3 puts const-eval in `jr-db`, downstream of sema. An enum's members are resolved during
*signatures*, so no computed value exists yet. That is true, and it is the reason arithmetic and `#run`
stay refused (§2). It says nothing about `NOT_FOUND :: 404`, where there is nothing to compute: the
literal is already in the HIR, and `jr-sema` already resolves names against the file scope one function
away. `jr-sema` still depends on neither `jr-db` nor `jr-vm`, which was checked rather than assumed.

## Decision

### 1. A member's value may be an integer literal, or a name for a constant whose initialiser is one

```jr
NOT_FOUND :: 404;
NEG_ONE :: -1;
Status :: enum {
    OK :: 200;
    MISSING :: NOT_FOUND;    // 404
    NEXT;                    // 405 — one past MISSING, not 2
    BAD :: NEG_ONE;          // -1
}
```

`NEXT` is the part worth stating explicitly. ADR-0041 §3 numbers each member one past the *previous
value* rather than its own index — C's rule and Jai's. A named value that resolved but did not feed the
auto-numbering would leave every later member silently wrong rather than rejected, which is the failure
class AGENTS.md names, so `valid/102` checks the 405 rather than assuming it.

### 2. One helper answers for both callers, and returns the value rather than a range-checked one

`constant_array_length` grew a second implementation's worth of logic — the `$N` comptime binding lookup
(ADR-0089 §1), the file-scope item lookup, the `ConstValue::Expr` unwrap, the `Literal::Int` match. All of
it is what "a readable constant" means, and none of it is about lengths. It is now
`Ctx::named_constant_int`, and `constant_array_length` is one line over it.

**The helper answers `i128` and leaves the range check to the caller**, because the two callers disagree
about range and that disagreement is real rather than incidental:

- an array length is a `u64` and **rejects a negative** — ADR-0039 §3's check, unchanged;
- an enum member is an `i64` and **accepts one** — `BAD :: NEG_ONE` is −1.

Returning the raw value is what lets both share the lookup without either inheriting the other's bounds.
Had the helper returned `u64`, the enum caller would have silently lost every negative constant; had it
returned `i64`, the length caller would have needed its own second check anyway.

**Rejected: a `constant_enum_value` beside `constant_array_length`.** It is the smaller diff, and it is
the duplication ADR-0070 §2 refused when it declined a second constant folder, ADR-0018 §2 refuses for
layout, and ADR-0020 §2 for trap messages. Two answers to "what is a readable constant" is precisely the
shape that lets the two drift, and the drift would be invisible: each would keep passing its own tests.
`valid/102` uses one constant as **both** an enum member's value and an array length, so a divergence
changes that file's arithmetic.

### 3. The diagnostic splits, because one message cannot be true for both readers

E0237's message was "an enum member's value must be an integer literal", which after §1 is false. It now
splits exactly as ADR-0070 §3 split E0233's:

- a value that **is a name** but not a usable one — "this enum member's value is not a usable constant";
- a value that is **not a name at all** — "an enum member's value must be a literal or a named constant".

A reader who wrote `X :: CHAIN` needs to hear that the *name* was not usable; a reader who wrote
`X :: 2 + 2` needs to hear that evaluation is missing. Both are pinned, one per file, because this
directory's contract is one diagnostic per file and only two files can pin two branches:
`type-errors/028` takes the arithmetic case and `type-errors/074` the named one.

`028` had to be **rewritten**, not added to: its whole construct was `X :: COUNT`, which §1 makes legal.
That is the second time a corpus file has been rewritten rather than deleted when a refusal lifted —
`023-array-length-not-literal` was the first, for ADR-0070 — and the reason is the same both times: the
file's comment is where the *line* is documented, and a reader who finds the file wants to know which side
they are on rather than that a file used to exist.

**No new code.** E0237 already means "this member's value is not usable". **E0282 is still the first free
code.**

### 4. What is deliberately absent

- **Arithmetic, `#run`, and cross-file constants** as member values, all refused for ADR-0018 §3's
  ordering reason, all arriving with the sub-wave that makes sema and comptime mutually recursive.
- **A constant naming another constant** (`A :: 4; B :: A; X :: B`). One level of indirection, not a
  chain, inherited from ADR-0070 §4 along with the mechanism: a chain needs a fixpoint and a cycle check,
  which is the evaluation machinery this avoids.
- **A member naming a *sibling* member** (`OK :: 200; ALSO :: OK;`). Legal C and legal Jai, and the data
  is available — the resolved member list is being built in order two lines away. **Left undecided
  because it is a fork, not an omission**: a sibling name and a same-named file constant can both be in
  scope, and which wins is a visible language decision with no obviously right answer. Deciding it inside
  a wave whose mandate was "generalise ADR-0070" would be exactly the quiet fork-picking AGENTS.md
  forbids. It goes to the decider on its own.
- **The cascade for an unresolved name.** `X :: UNKNOWN` reports E0201 *and* E0237, where the array-length
  path reports one diagnostic, because a length's name is not an expression and never reaches name
  resolution. Suppressing the second was **implemented, measured, and reverted**: `Expr::Name`'s `res`
  field is `Res::Error` for *every* enum member value, including ones that resolve correctly — resolution
  visits these expressions and reports on them but never writes the field back. Suppressing on
  `Res::Error` therefore silenced the valid case too, and `valid/102` compiled to wrong enum values with
  no diagnostic at all. That is a swallowed refusal, which is strictly worse than a duplicated one, so the
  cascade stays and the field carries a comment saying why it must not be trusted. Inferring "already
  reported" from the scope lookup instead would reintroduce the same risk one step further out.

## Consequences

- One place in `jr-sema` defines what a readable compile-time constant is, and two features consume it.
  A third — a struct field's default, a `#bake_arguments` value — extends by calling it.
- **1010 tests, unchanged.** Both new corpus files are *iterated* by harnesses that already exist, so
  coverage grew without a test case, which is why the corpus count is tracked separately: **214 → 216**
  (226 counting `tests/corpus/modules/`).
- The MIR snapshot gained `valid/102`, and its constants are the evidence the feature works at the far
  end: `404_Status`, `405_Status`, `-1_Status`, and `s0: [3]s64` for the shared constant used as a length.
- A latent trap is now documented rather than merely absent: anyone reaching for `Expr::Name`'s `res` in a
  signature-time context will find the comment before they find the bug.

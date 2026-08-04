# ADR-0079: A pointer or view in a compile-time aggregate is refused (completing ADR-0074 §2)

- **Status:** Accepted
- **Date:** 2026-08-03
- **Deciders:** dboll
- **W4 sub-wave 10, part 1.** Found by probing whether a constant aggregate could hold a view — the
  premise the `Type_Info` field list rests on. It could not, and the way it could not was a **silent
  miscompile that had shipped**. This fixes that before anything is built on the path.

## Context

### 0. What running found

```
H :: struct { p: *s64; n: s64; }
mk :: () -> H { v: s64; v = 42; h: H; h.p = *v; h.n = 7; return h; }
V :: #run mk();
main :: () { exit(V.p.*); }
```

- The VM exits **48**.
- The native binary **segfaults** (exit 139).
- Neither reports a diagnostic.

42 was never a possible answer. The two engines disagree, both are wrong, and nothing said so. This is
`PLAN.md` §5's named failure mode exactly: *a construct the grammar allows, no representation on the
lowering path, filled in with a placeholder that is a legitimate value.* Here the placeholder is an
integer — `reduce_element` listed `Item::PointerType` and `Item::ViewType` in its **scalar** arm, so a
pointer's eight bytes were interned as `Raw::Int`. An integer is a legitimate value, so no verifier and no
poison gate could object.

A view fails the same way and looks different: it is `{data, count}`, so interning it as one 8-byte scalar
keeps the *data* word and drops the count (or the reverse, per layout). The observed symptom was a trap —
`error: index out of bounds` — which reads as a bug in the user's program rather than the compiler's.

**The corpus differential is blind to it**, because no corpus file holds a pointer or a view inside a
compile-time aggregate. That is precisely the gap `AGENTS.md` names: "if a construct is legal in the
corpus, something must execute or snapshot it". The construct is legal and nothing executed it.

### The rule already existed and had not been extended

ADR-0074 §2 refused `string` as an `AggregateValue` element and gave the reason:

> a string's *contents* are its identity, its runtime form is a `{data, count}` pair pointing at bytes the
> back end emits separately … Folding it into `AggregateValue` would mean interning a pointer, which has
> no compile-time value at all.

That argument is about **pointers**, not about strings. It covers a raw `*T` and a view word-for-word. The
code simply never applied it to them: `string` got an explicit early return (by contents), and everything
else fell through to a scalar decode that happened to accept a pointer-shaped thing.

## Decision

### 1. A pointer or a view element in a compile-time aggregate is refused

`reduce_element` gains two arms, each with its own message naming why:

- a pointer — "the address is the compile-time evaluator's, not the program's";
- a view — "the view's data pointer is the compile-time evaluator's, not the program's".

Both surface as **E0230**, the code every other const-eval refusal uses, because the remedy from the
user's side is the same: this value cannot be computed at compile time.

`string` is unaffected. It is handled *above* this match by contents (`Raw::Str`), which is why it works
and a raw pointer does not — the distinction ADR-0074 §2 drew and this now enforces.

**Rejected: relocating the pointee into interned data.** Tempting, and it is what a `Type_Info` field list
will eventually need: copy what the pointer points at into data the back end emits, and rewrite the pointer
to address that. It is rejected *here* because doing it implicitly would **silently change what the program
points at** — `h.p = *v` would come to mean "a pointer to a copy of `v`", which is a different program from
the one written, and nothing in the source would say so. Relocation is a decision that needs a syntax or a
declared intent, not a quiet fixup inside a const-eval fallback.

**Rejected: leaving it and documenting.** A wrong answer with no diagnostic is what ADR-0017 §4 says must
refuse. Two *different* wrong answers across the engines makes it worse, not more tolerable.

### 2. Pinned by CLI exit-code tests, not corpus files

Two tests in `jr-cli`'s integration suite, for ADR-0074 §4's reason: E0230 is `jr-db`'s code and **no
corpus directory holds one** — `type-errors/` is `jr-sema`'s and `cfg-errors/` is `jr-mir`'s, so filing
either there would break that directory's stage contract.

Teeth-checked: restoring the scalar decode makes the pointer test fail.

## Consequences

- **A shipped silent miscompile is now a refusal.** It was introduced with `AggregateValue` (ADR-0074) and
  survived two waves, including one — ADR-0075 §1 — that *rewrote the same function* to make `Raw` a tree
  and still did not notice the scalar arm accepting a pointer.
- **The corpus gap is recorded rather than closed.** These two cases cannot become `valid/` corpus files:
  they are refusals, and the only directory for a `jr-db` refusal is nowhere. The lesson is the one
  `AGENTS.md` already states, now with a second instance: a construct that is legal but untested is where
  the next one of these lives.
- **The `Type_Info` field list needs the relocation decision.** §1 rejected implicit relocation, so a field
  list whose elements must outlive const-eval needs an explicit mechanism — which is the memory-ownership
  decision ADR-0075 §3 and ADR-0078 §4 deferred, now with a sharper reason for being a real decision rather
  than a detail.
- **`string` is the model for what *does* work.** A value whose identity is its *contents* interns cleanly;
  a value whose identity is an *address* cannot. That is the line, and it is now enforced rather than
  implied.

# ADR-0096: `#bake_arguments` is a partial application producing a specialised procedure — the surface

- **Status:** Accepted
- **Date:** 2026-08-04
- **Deciders:** dboll
- **W5 sub-wave 7g**, the last of the macro family. The surface: the directive parses with a *call-shaped*
  operand. Producing the specialised procedure is refused by design (E0276) pending the next sub-wave.

## Context

`#bake_arguments add(a = 5)` produces a procedure with some arguments built in, so `add_five(37)` means
`add(5, 37)`. It is the third and last of W5's macro pieces, after `#expand` (ADR-0090/0091) and `#modify`
(ADR-0093/0094/0095).

Premise verified by running: `add_five :: #bake_arguments add(a = 5);` was a parse error.

## Decision

### 1. A baked procedure is a clone with the baked parameters dropped — ADR-0088's mechanism, reused

The specialised procedure is a **clone** of the original with the baked parameters removed from its parameter
list and their values substituted into its body. That is *literally* what `$N` instantiation already does
(ADR-0088 §3): `append_one` in `instantiate.rs` drops the marked parameters, rewrites their `Res::Param`
name-uses into literals, and remaps the remaining indices — all three steps built and teeth-checked.

**Why this matters for finishing W5.** `#bake_arguments` is a *reuse* of the polymorphism machinery rather
than a new mechanism, which is what makes it the right piece to end the wave on. A wrapper procedure that
called the original would also work, but it adds a call layer the inliner would then have to remove; a
call-site rewrite would stop `add_five` being a value.

### 2. The baked value comes from const-eval, judged as a `$N` argument is

A baked argument is a compile-time constant by definition — the point is that the specialised procedure has
it built in — so it is evaluated by the pre-pass ADR-0088 §2 built and a non-constant is refused. Requiring a
*literal* would be needlessly narrower than `$N` already is.

The operand is a **call expression**, so its named-argument spelling (`a = 5`) is the ordinary one (ADR-0053
§1) rather than a second syntax invented here. The parser arm is shaped like `#insert`'s computed-operand arm:
a directive that parses a full expression, which the generic directive arm cannot express.

### 3. Refused by design (E0276), in *lowering*, and why that specific placement

Until the specialisation exists, `#bake_arguments` is refused with its own code, raised where a directive's
validity in expression position is already judged.

**This replaced a bad message rather than merely adding one.** Before it, the declaration lowered to a
poisoned expression and the *caller* reported:

> warning[E0245]: the compiler could not lower the body of `main` … this program is legal and this compiler
> has a gap — please report it

That wording is right for an *unknown* gap and wrong for a feature whose absence is known and named. The
refusal turns a bug report into a sentence a reader can act on — the same correction ADR-0069 and ADR-0079
made for leaked internal errors, here for a leaked *gap report*.

## Consequences

- **A new diagnostic code, E0276.** It will be lifted by the specialisation sub-wave, exactly as E0268,
  E0271's first meaning, E0272's first meaning and E0274 were — the fifth such refusal, and every one has
  named the sub-wave that removes it.
- `#bake_arguments` joins `DIRECTIVES_VALID_AS_EXPRESSIONS`, so its call-shaped operand is not rejected as
  "not valid here": the surface is real, only the specialisation is owed.
- **W5's macro family is otherwise complete**: `#expand` splices, `#modify` runs and can reject. What remains
  of W5 as a whole is this specialisation.

# ADR-0190: Typed constants — `name : T : value`

- **Status:** Accepted
- **Date:** 2026-09-03
- **Deciders:** dboll

## Context

A Jairs constant took its type from its initialiser, and an untyped integer literal lands on `s64`
(ADR-0016 §1). So a constant that describes a value of some *other* type could not say so, and every
use had to compensate: `modules/GL` carried **twenty `cast(u32, X)` for twenty-one constants that are
all `GLenum`**, and `modules/Window` and `modules/UI` had the same shape for `u32` flags and a
`float32` thickness.

Three consecutive handoffs named this the highest-value **small** item owed — ADR-0165 §5 first, then
the per-OS stretch, then ADR-0182 — each time with a fresh count of cast sites. It was never the
largest thing owed, and it was always the cheapest thing owed that nothing else was blocked on.

## Decision

### 1. The surface is `name : T : value`, and the parser decides after the type

`X : u32 : 5;` is a constant of type `u32`. The spelling is Jai's, and it is the only spelling that
composes: `name : T` is already a variable and `name :: value` is already a constant, so the typed
form is the intersection written out.

The **node kind is not known until the type has been read**, because `X : u32 : 5` and `x : u32 = 5`
differ only in a token that comes *after* the annotation. So the tree is built behind a `rowan`
checkpoint and wrapped as a `CONST_DECL` or a `VAR_DECL` once the answer is in — the technique
`parse_expr_or_assign_stmt` already uses for `=`.

**A third node kind was rejected.** A typed constant *is* a constant: it has a value at compile time
and no storage, and every consumer of `CONST_DECL` — `file_consts`, the signature phase, the
formatter, the LSP, the tree-sitter queries — already does the right thing with one. A
`TYPED_CONST_DECL` would teach a dozen places about a variant that behaves identically everywhere
except in reading one extra child.

### 2. The annotation is carried as a **field**, not a variant, and it is the expectation

`ConstValue::Expr(ExprId)` became `ConstValue::Expr { expr, ty: Option<TypeRefId> }`.

A `ConstValue::TypedExpr` variant would have been additive and cheaper — and wrong. Every one of the
**thirty** existing `ConstValue::Expr` sites would have kept compiling while silently not matching a
typed constant, which is the silent-skip failure mode ADR-0186 §3 caught in a `let-else` and which
this project has now been bitten by three times. Changing the *shape* makes all thirty a compile
error, so each one is read and decides. Twenty-eight wanted `{ expr, .. }` — the array-length lookup,
the foreign-library lookup, the LSP's four — and two mattered.

The one that mattered most is `jr-sema`'s signature phase, which read:

```rust
// No annotation exists on a `::` declaration, so the
// initialiser types itself and an untyped integer literal
// lands on the default (ADR-0016 §1).
let ty = self.check_expr(ExprScope::TopLevel, expr, None);
```

That comment was true when written and false the moment the parser accepted an annotation. It now
resolves the annotation through the **same `resolve_type`** the `Var` arm three arms below uses, so a
typed constant and a typed variable cannot disagree about what `u32` means, and passes it as the
expectation.

**The declared type wins where there is one**, matching the `Var` arm — which is the reason to do it
rather than a preference: two rules for one annotation would be a difference nobody could justify.
Nothing is hidden by the override, because a value that does not fit has already been reported;
recovering as the type the declaration asked for gives every later use one honest error instead of a
cascade.

### 3. `modules/GL` is the proof, and the surviving cast is the interesting one

Twenty-one constants are now `u32` or `s32`, and **all twenty casts are gone** — nineteen deleted and
one *kept*:

```jr
gl_tex_image_2d(TEXTURE_2D, 0, cast(s32, RGBA), …, RGBA, UNSIGNED_BYTE, pixels);
```

`internalformat` is a `GLint` while `format` is a `GLenum`, so the same constant crosses at two widths
in one call. That is exactly what a cast is *for* — and it could not say so while every constant was
an `s64` and every argument needed one. A cast that means something now looks different from a cast
that was noise, which is the real return on this wave rather than the character count.

The change **cascaded into two signatures**, and the cascade is the point: `GL.clear(mask: s64)` became
`(mask: u32)` and `GL.create_shader(kind: s64)` became `(kind: u32)`, because those parameters are a
`GLbitfield` and a `GLenum`. Each step made a signature say what it had always meant.

### 4. The formatter dropped the annotation, and the fix is discriminated on the token

**Fourteenth wave in sixteen**, and again the *unsound* direction: `THICK : float32 : 1.0` reformatted
to `THICK :: 1.0`, so the reformatted file no longer type-checks — a `float32` constant silently
becomes an `s64`. Round-trip and idempotence assertions both passed without the fix, because a
formatter that re-emits every other child verbatim satisfies both while deleting one.

The **first fix was wrong in an instructive way**: it asked whether any child was a type kind, and
`Array :: struct($T) { … }` has one — its *value* — so it emitted `Array : struct($T) {` and gate 5
caught it on the next run. The discriminator is the **token**: an ordinary constant carries one `::`
and a typed one carries two `:`, which is the only place the difference is recorded.

The tree-sitter grammar needed the alternative too (gate 6 reported four `ERROR` nodes), as one more
`choice` arm in `const_decl` rather than a new rule — for §1's reason, and so the highlight, fold,
indent and locals queries need no entry.

### 5. Two checks that were pinning prose rather than behaviour

Neither is caused by this wave; both were found by running `verify.lua`, which is verified and not
gated, and had gone stale during ADR-0189.

`hover on an imported procedure shows its module and its documentation` compared the **entire hover
card** against a literal, including `print`'s complete doc comment — so ADR-0189 broke it by
*documenting* the procedure better. A check that fails when prose improves is measuring the wrong
thing; it now asserts the four parts its own name claims. And `resolve supplies the same card the
hover shows` pinned the same text a second time, so it now compares against the hover card itself —
nothing in that file knows what `print`'s documentation says any more.

## Consequences

- A constant crossing a C boundary says its own type, and twenty casts in `modules/GL` are gone.
- A value that does not fit is refused **where the constant is written**, not at whichever use first
  wanted the narrower type — possibly in another file, possibly never.
- A `u64` constant above `2^63` is expressible for the first time.
- `GL.clear` and `GL.create_shader` take a `u32`, which is what their C counterparts take.
- Still owed: a typed constant cannot be a *type* alias with an annotation (`P : type : u8`), which
  nothing has wanted; and a builtin still cannot be aliased at file scope (`P :: u8;` is E0201), which
  is why `valid/141` asserts widths through parameter types rather than through `size_of`.

## Alternatives considered

**Inferring the width from the use site.** Rejected: a constant used at two widths would have two
types, and `RGBA` above is used at exactly two. The annotation is what makes the difference visible.

**A `TYPED_CONST_DECL` node, or a `ConstValue::TypedExpr` variant.** Rejected in §1 and §2, both
because they let existing code keep compiling while silently not handling the new case.

**Leaving the casts in place after typing the constants.** Rejected: the casts were the *cost* this
wave exists to remove, and keeping them would leave twenty places where a reader cannot tell a
meaningful narrowing from a compensating one.

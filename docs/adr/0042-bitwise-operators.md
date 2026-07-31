# ADR-0042: bitwise operators bind tighter than comparison, and an out-of-range shift traps

- **Status:** Accepted
- **Date:** 2026-07-28
- **Deciders:** dboll

## Context

`& | ^ ~ << >>` are the last of `PLAN.md` §2.1's W1 arithmetic, refused since the slice with
E0122 "bitwise operators arrive in wave W1". They are taken now because they *unblock*
something: `enum_flags` is meaningless without bitwise combination, so ADR-0041 §3 recorded it
as **blocked** rather than deferred.

Four facts were established by reading the code before this ADR was written.

- **All six tokens already lex.** `AMP`, `PIPE`, `CARET`, `TILDE`, `SHL`, `SHR` are real
  `SyntaxKind`s with text, distinct from `AMP_AMP` and `PIPE_PIPE`. Nothing in the lexer
  changes.
- **There is exactly one E0122 site**, and it is the *prefix* position in `parse_primary`.
  The binary positions do not error at all — they simply fall out of the Pratt loop's `_ =>
  break`, so `a & b` parses as `a` and then fails at the statement level. That is worth
  knowing: the refusal was never uniform.
- **The Pratt table is a clean `match` returning `(lbp, rbp)`**, so adding levels is data
  rather than structure. Levels are currently 1–10 in steps of 2.
- **`jr-pool`'s `IntOp` is where the arithmetic lives** (ADR-0022 §2), and it has no bitwise
  variants. Adding them there rather than in each evaluator is what keeps the folder and the
  interpreter from disagreeing.

## Decision

### 1. Bitwise binds **tighter** than comparison, not looser

```text
 1  ||
 2  &&
 3  == != < <= > >=
 4  |
 5  ^
 6  &   (binary)
 7  + - +% -%
 8  << >>
 9  * / % *%
10  prefix - ! ~ *
```

So `flags & MASK == 0` is `(flags & MASK) == 0`.

**C puts `&`, `^` and `|` below the comparisons**, which makes that expression
`flags & (MASK == 0)`. This is a famous design error — Dennis Ritchie described it as a
mistake retained only for backward compatibility with pre-`&&` C, where `&` served both
roles. Jairs has no such compatibility to keep, and Go, Rust and Zig all moved bitwise above
comparison for exactly this reason.

Worth stating precisely what C's ordering would cost *here*, because it is not a wrong
answer: `MASK == 0` is a `bool`, and `flags & bool` is a type error under ADR-0016's rules.
So Jairs would **catch** the mistake rather than miscompile it. The reason to reorder anyway
is that catching it means *refusing code that reads correctly* — the programmer would have
written what they meant and been told it was wrong.

`|` loosest, then `^`, then `&`, matching every language that has all three: it makes
`a & b | c & d` mean `(a & b) | (c & d)`, which is how a bit-manipulation idiom is written.

**Shifts sit between `+` and `*`**, following Go and Rust. `a + b << c` is `a + (b << c)`.
C puts shifts *below* `+`, so C reads it as `(a + b) << c` — another ordering Go and Rust
changed, and for the same reason: a shift is closer to a multiplication than to an addition.

**Rejected: require parentheses**, making `a & b == c` a syntax error. Safest in the abstract
and rejected as surprising: it refuses an expression whose meaning is unambiguous under any
sensible ordering, and no mainstream language does it.

### 2. `>>` is arithmetic for a signed type and logical for an unsigned one

```jr
x: s8 = -8;
x >> 1        // -4  — sign-extends
u: u8 = 240;
u >> 4        //  15 — zero-fills
```

The *type* decides, exactly as it already does for `/` (`sdiv` versus `udiv`). There is no
separate `>>>` operator, because there is no signed type whose shift should zero-fill: a
program that wants the bits without the sign casts to the unsigned type of the same width,
which is what `cast` is for.

`<<` needs no such split: shifting left fills with zeros regardless of signedness.

### 3. An out-of-range shift count **traps**

`x << 8` where `x` is an `s8` traps, and so does `x >> 8`. A count is out of range when it is
`>= the type's width` **or negative**.

This is ADR-0002's rule applied to a new operator, and the argument transfers exactly: a
shift by 8 of an 8-bit value produces a result the program did not ask for. Every alternative
is a *silent* wrong answer:

- **Masking to the width** — `x << 8` becomes `x << 0` — is what x86 does natively and is
  therefore free. Rejected because the masking is invisible: the program asked for 8 and
  silently got 0. This is C's undefined behaviour made deterministic, which is better than UB
  and still wrong.
- **Saturating to 0** (or to the sign for `>>`) is total and defensible — it is what a
  mathematician would expect. Rejected because it costs the same branch as trapping and turns
  a likely bug into an answer rather than a report.

A **negative** count traps for the same reason rather than being reinterpreted as a shift the
other way, which would make `x << -1` silently mean `x >> 1`.

The trap wording is `"shift count out of range"`, in `jr-pool`'s `IntTrap` beside the
overflow ones so that ADR-0020 §2's single formatter renders it and the two engines cannot
drift.

### 4. `~` is a bitwise complement on the type's own width

`~cast(u8, 0)` is 255, not `-1`. The complement is taken and then normalised to the type,
which is what `IntKind::wrap` already does for every other operation — so a narrow type
complements within its own width rather than at 64 bits and then truncating differently in
the two engines.

`~` is a **new prefix operator**, joining `-`, `!` and `*`. It is at the same precedence as
those, so `~a & b` is `(~a) & b`.

`~` on a `bool` is refused: `!` is the boolean negation and having both would make `~true`
mean something. A `bool` is one byte and its complement is 254, which is not a `bool` at all.

### 5. Bitwise operators are **integers only** — no floats, no enums

- **Floats.** `1.5 & 2.5` is refused. A float's bits are a sign, an exponent and a mantissa;
  ANDing two of them produces a bit pattern that is not the AND of anything meaningful. A
  program that wants a float's bits will want `cast` to an integer of the same width, which
  Jairs does not have yet — recorded as owed rather than smuggled in here.
- **Enums.** `Colour.RED | Colour.GREEN` is refused **in this wave**, and that refusal is
  exactly what `enum_flags` will lift. ADR-0041 §6 refused arithmetic on an enum because
  members are named alternatives rather than magnitudes; a *flags* enum is the case where they
  genuinely are combinable, and it is a different declaration form (`enum_flags`) precisely so
  that the two cannot be confused. Refusing here keeps that distinction available.

No wrapping variants (`&%` and friends) exist or are needed: `&`, `|`, `^` and `~` cannot
overflow, and `<<` traps rather than wrapping by §3.

### 6. Compound assignment for all five binary forms

`&=`, `|=`, `^=`, `<<=`, `>>=`, matching the existing `+=` family. The lexer already produces
`AMP`, `PIPE`, `CARET`, `SHL`, `SHR`, so these are five new *tokens* — and that is the one
place this wave touches the lexer.

Omitting them would be a gap a user notices immediately: `flags |= FLAG` is the commonest
line of bit-manipulation code there is.

## Consequences

- **`enum_flags` becomes unblocked**, which is why this wave came before it. §7's ordering
  reflects that.
- **`IntOp` grows five variants and `IntTrap` one.** Both are exhaustively matched in the
  interpreter, the folder and the verifier, so every site that must change is a compile error
  — the house style paying off again.
- **Cranelift has all five natively**: `band`, `bor`, `bxor`, `ishl`, and `sshr`/`ushr` chosen
  by signedness. The shift-count check is a compare-and-trap into the *existing* cold trap
  block, so it reuses `trap_if` rather than adding a mechanism.
- **`~` is the first new prefix operator since the slice**, so `UnOp` grows a variant and
  `EXPR_START` needs `TILDE` — the token-set predicate trap that has now swallowed two
  features (`CAST_KW` against `EXPR_START`, `L_BRACK` against `TYPE_START`). Checked
  explicitly rather than discovered.
- **The E0122 refusal is deleted, not reworded**, and the number is retired rather than reused
  — the rule ADR-0040 established for E0120.
- **A shift is the third trapping operation**, after overflow and division by zero. The
  differential harness gains a case, because a trap's wording *is* a failing program's output
  (ADR-0020 §2).

# ADR-0011: Dereference is postfix `.*`; address-of is prefix `*`

- **Status:** Accepted
- **Date:** 2026-07-25
- **Deciders:** dboll

## Context

C uses prefix `*` for both the pointer type (`int *p`) and dereference (`*p`),
and prefix `&` for address-of. Prefix dereference has two well-known problems:
`*p` is visually ambiguous with multiplication, and dereference chains read
inside-out — `**pp` and, worse, `*(*pp).field` force the reader to unwind the
expression from the middle. Jai resolves this by making dereference *postfix*.

Jairs reuses a single `STAR` token for the pointer type, address-of, and
multiplication; the parser disambiguates by position (see
`tests/corpus/valid/015-pointers.jr` and the lexer's `pointer_syntax` test).
Dereference gets its own postfix token.

## Decision

- **Address-of is prefix `*`:** `*x` takes the address of `x`, and `*T` is the
  pointer type. The `STAR` token is shared with multiplication; position
  disambiguates.
- **Dereference is postfix `.*`:** `p.*` dereferences `p`. It is a distinct token
  (`DOT_STAR`), lexed by longest-match so that `p.*` is `p` `.*` while `a.b` is
  `a` `.` `b` and `1..2` is `1` `..` `2`.

Chains therefore read left-to-right: `p.*.*` dereferences twice,
`ppp.*.*` reads as written.

## Consequences

### Positive

- No visual ambiguity between dereference and multiplication.
- Dereference chains read left-to-right and compose cleanly with field access
  (`origin` → `pp := *origin` → `pp.x`), where field access through a pointer
  auto-dereferences.
- The lexer's longest-match table cleanly separates `.`, `.*`, and `..`.

### Negative

- Postfix dereference is unfamiliar to C programmers and is a small up-front
  learning cost.
- Overloading `*` for type/address-of/multiply means the parser, not the lexer,
  carries the disambiguation.

### Follow-on work this forces

- The lexer must order `.*` and `..` ahead of `.` in its longest-match operator
  table (it does), and the parser must treat prefix `*` and postfix `.*` as
  distinct productions (`UNARY_EXPR` for address-of, `DEREF_EXPR` for
  dereference). Both are exercised by `tests/corpus/valid/015-pointers.jr`.

## Alternatives considered

- **C-style prefix `*` dereference with prefix `&` address-of.** Rejected: prefix
  `*` collides visually with multiplication and forces inside-out reading of
  pointer chains, which is exactly the readability problem postfix `.*` removes.

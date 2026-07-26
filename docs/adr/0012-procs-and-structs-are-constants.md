# ADR-0012: Procedures and structs are constants

- **Status:** Accepted
- **Date:** 2026-07-25
- **Deciders:** dboll

## Context

Most languages have distinct declaration forms for functions, types, and
constants: a `fn`/`func` keyword introduces a function, a `struct`/`class`
keyword introduces a type, and a separate `const`/`let` form introduces a value.
Jai takes a different view: a procedure and a struct are simply *values* that
happen to be bound to a name at compile time. `add :: (…) {…}` and
`Point :: struct {…}` are both instances of one rule — `name :: value` — where
the value is a procedure or a struct type.

The corpus reflects this uniformly:
`MAX_ENTITIES :: 4096` (`007-constants.jr`), `add :: (…) -> s64 {…}`
(`004-proc-params-return.jr`), and `Point :: struct {…}` (`008-struct.jr`) are
the *same* declaration form. The CST agrees: `kind.rs` has a single `CONST_DECL`
node whose value is a `PROC` or a `STRUCT_TYPE`, and its own doc comment states
"Procedures and structs are constants whose value is a `PROC` or `STRUCT_TYPE`,
exactly as in Jai."

## Decision

Procedures and structs **are constants**. `name :: value` is *the* compile-time
constant declaration form; when the value is a procedure literal `(…) {…}` the
constant is a procedure, and when it is `struct {…}` the constant is a struct
type. There are **no** separate `proc`/`func`/`struct`-declaration forms — there
is one uniform declaration rule, and the value's shape determines what was
declared.

## Consequences

### Positive

- One declaration rule in the grammar covers constants, procedures, and struct
  types (`CONST_DECL` in the CST), which keeps the parser and the spec small.
- Procedures and types are first-class compile-time values, which is the natural
  substrate for later waves: polymorphs (W5) take type *values* as arguments, and
  first-class `Type` values (W4) fall out of the same model.
- "Is this name a procedure?" is answered by inspecting the bound value, not by a
  different syntactic category.

### Negative

- Readers coming from `fn`/`struct`-keyword languages must learn that `::` alone
  introduces all three, distinguished only by the right-hand side.
- Tooling that wants to classify a declaration must look at the value, not the
  declaration form.

### Follow-on work this forces

- **Into the slice:** the grammar has a single `name :: value` constant
  declaration (`CONST_DECL`), and the `struct { … }` and procedure `(…) {…}`
  forms are *expressions/values* on its right-hand side, not top-level
  declaration keywords. This is already the shape of `kind.rs`.
- Tooling (LSP symbol kinds, `jr fmt`) inspects the bound value to decide whether
  a constant is a procedure, a type, or a plain value.

## Alternatives considered

- **Separate `proc`/`struct` declaration forms.** Rejected: it would multiply the
  declaration grammar, split what is conceptually one rule into three, and break
  the "procedures and types are just compile-time values" model that waves W4
  (first-class `Type`) and W5 (polymorphs over type values) are built on.

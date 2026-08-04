# ADR-0085: A polymorphic struct is a parameterised type, keyed on its declaration and its type arguments

- **Status:** Accepted
- **Date:** 2026-08-04
- **Deciders:** dboll
- **W5 sub-wave 5 — design of record.** This ADR fixes the design; the implementation is the sub-wave's
  own work and is larger than a single edit, because it changes how a *type's identity* is keyed — the most
  load-bearing invariant in the pool. It is written now, at the point of decision, so the build starts from
  a settled design rather than discovering it mid-change.

## Context

`Box :: struct($T) { value: T; }` used as `Box(s64)` is what the standard library needs — `Array($T)`, a
hash table, a bucket array (W7) are all parameterised structs. Procedure polymorphism (ADR-0081–0084)
handles the *value* side; this is the *type* side, and it differs in one hard way.

**A struct's identity is its `DeclId`** (ADR-0015 §1): `Item::StructType { decl }` carries nothing else,
the field side-table is keyed by `DeclId`, and `layout_of`/`field_offset`/both engines' field access all
look up by `DeclId`. `Box(s64)` and `Box(bool)` share one `DeclId` and must be **distinct types** with
distinct field types and distinct layouts. So the change is not additive the way procedure instantiation
was (which minted new `ProcId`s); it changes what a type's identity *is* for a parameterised struct.

## Decision

### 1. `Item::StructType` gains its type arguments; the key becomes `(decl, args)`

```rust
StructType {
    decl: DeclId,
    args: Vec<PoolId>,   // NEW — the type arguments; empty for an ordinary (non-parameterised) struct
}
```

An ordinary `struct { … }` has `args: []` and interns exactly as today (the empty vec changes no existing
key). `Box(s64)` interns as `StructType { decl: box_decl, args: [s64] }` and `Box(bool)` as
`{ decl: box_decl, args: [bool] }` — **distinct `Item`s, distinct `PoolId`s**, deduped and told apart by
the interner for free, the same way `ArrayType { elem, len }` distinguishes `[2]s64` from `[3]s64`.
`UnionType` and `VariantType` gain the same field, for the same reason.

**Why in the key rather than a side table.** A side table keyed by `DeclId` cannot hold two instances of
one declaration — which is the whole requirement. Putting `args` in the `Item` is what makes the two
instances two types.

### 2. Field types are computed per instance by substitution

The field side-table becomes keyed by the **instance** (the `StructType` `PoolId`), not the `DeclId`. An
instance's fields are the declaration's field `TypeRef`s resolved under the type-argument bindings — `Box`'s
`value: T` becomes `value: s64` for `Box(s64)` — the same substitution-by-binding (`type_bindings`)
procedure instantiation already uses. So `struct_fields(instance)` returns the concrete fields, and
`layout_of`/`field_offset` need change only to key on the instance rather than the `DeclId` they extract
from it.

### 3. A type reference `Box(s64)` is a *call-shaped* type

The grammar gains `struct($T) { … }` (a parameter list before the brace) and a type reference
`Name(args)` (a type applied to type arguments). `Box(s64)` in type position resolves the arguments,
instantiates the struct, and interns the `StructType { decl, args }`. This reuses the type-value machinery
(ADR-0071): a type argument *is* an interned type, so `Box(s64)` is "apply the `Box` type constructor to
the type value `s64`".

### 4. Layout and both engines follow the instance

`layout_of` and `field_offset` already take a `PoolId` and read `struct_fields`; keying that read on the
instance rather than the bare `DeclId` is the whole back-end change, because both engines compute offsets
through those two functions (ADR-0018 §2). No new MIR, no new codegen node — an instantiated struct is an
ordinary aggregate whose fields happen to have come from a substitution.

### 5. What the sub-wave will defer

- **A polymorphic struct as a `$T` procedure parameter inferring the struct's argument** — `f :: (b:
  Box($T))` binding `T` from a `Box(s64)` argument. That is nested inference (ADR-0084) through a nominal
  parameterised type, a further step.
- **Multiple type parameters on a struct** — `Map($K, $V)` — deferred exactly as multiple procedure
  variables were staged after one (ADR-0083 was its own sub-wave).
- **Recursive parameterised types** — `List($T) { next: *List(T); }` — which needs the instance's identity
  before its fields are resolved, the same fixpoint ADR-0015 §1 solved for a non-parameterised recursive
  struct, now per instance.

## Consequences

- **The pool's most load-bearing key changes**, which is why this is a design-of-record ADR rather than an
  in-line edit: every site that matches `Item::StructType { decl }` becomes `{ decl, args }`, and the
  exhaustive-match discipline turns each into a compile error to be handled — the mechanism that has caught
  every such change's missed sites (ADR-0068, ADR-0074).
- **The field side-table re-keys from `DeclId` to instance `PoolId`.** This is the delicate part: a
  non-parameterised struct's fields must still be found, so the empty-args instance keys the same fields
  the `DeclId` used to. Done right, an ordinary struct is unchanged and a parameterised one is a
  generalisation.
- **`type_info` of a parameterised struct** reports its instantiated fields once ADR-0078's per-kind field
  list exists, tying this to the deferred RTTI piece — a parameterised `Type_Info` is where the two meet.
- **This is deferred as an implementation, accepted as a design.** The sub-wave that builds it starts from
  §1–§4 rather than rediscovering that a struct's identity must grow its arguments — the decision that took
  the most weighing here.

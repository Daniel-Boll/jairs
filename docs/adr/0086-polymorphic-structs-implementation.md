# ADR-0086: Polymorphic structs, as built — staged, two-map, refuse-what-is-deferred

- **Status:** Accepted
- **Date:** 2026-08-04
- **Deciders:** dboll
- **W5 sub-wave 5.** ADR-0085 fixed the *design* of a parameterised struct and declared itself deferred as
  an implementation. This ADR records the decisions the build itself required, none of which ADR-0085
  settled: how to stage a change to the pool's most load-bearing invariant, how the field side table is
  actually re-keyed, and how the deferred pieces of ADR-0085 §5 are held so none becomes a silent gap. It
  **amends nothing** in ADR-0085 — §1–§4 were followed as written — it fills in the "how" beneath them.

## Context

ADR-0085 §1 puts a struct's type arguments in its `Item` key, and §2 keys the field list on the instance
`PoolId` rather than the `DeclId`. Both are changes to the invariant ADR-0015 §1 established: *a struct's
identity is its declaration site*. A change there touches 44 match sites and ~40 field-reading call sites
across the pool, both engines, sema, MIR, codegen, the VM and the LSP — and a half-finished version is
exactly this project's named catastrophic failure mode, a well-typed placeholder that miscompiles. So the
open questions were about *sequencing and safety*, not design.

## Decision

### 1. Two commits: a zero-behaviour-change representation refactor, then the behaviour

`Item::StructType`/`UnionType`/`VariantType` gained `args: Vec<PoolId>` (empty for an ordinary struct, so
no existing key moves) as commit **5a**, together with moving every field *read* onto a new dispatcher
`Pool::fields_of(ty)`. That commit is proven behaviour-preserving: the workspace test count and every
snapshot are byte-identical to the prior `main` (969 tests, no `.snap.new`). The parameterised behaviour —
grammar, resolution, instantiation — is commit **5b** on top.

**Why staged.** A single commit would tangle "the representation changed" with "`Box(s64)` now works", so a
moved snapshot could not be attributed to one or the other. Splitting them means 5a's proof is unambiguous:
if any observable output had changed there, the refactor was wrong, and it is caught *before* any new
grammar exists to hide it.

### 2. Two field maps, not one re-keyed

ADR-0085 §2 says the field table keys on the instance. Rather than re-key the single `struct_fields:
DeclId → fields` map — which would touch every writer (sema, sigs, ctx) and change what they pass, mixing
behaviour into 5a — an ordinary struct keeps its `DeclId`-keyed map untouched and a *parameterised instance*
lands in a new `instance_fields: PoolId → fields` map. `Pool::fields_of(ty)` dispatches on whether the
`Item` carries arguments. This reaches ADR-0085's stated consequence verbatim — "an ordinary struct is
unchanged, a parameterised one is a generalisation" — while letting 5a add a map with **no writer yet**, a
dormant generalisation rather than a speculative half-change, because 5b (its writer) is the same wave.

### 3. Fields resolved per reference, keyed on the instance, with a recursion guard

A `Box(s64)` reference resolves in sema's `resolve_apply`: look the constructor name up to a `struct($T)`
declared in this file, resolve the arguments, bind the variables (the same `type_bindings` procedure
instantiation uses), intern the instance via `Pool::struct_instance`, and resolve the declaration's field
list *under those bindings* into `instance_fields`. Before resolving the body it reserves the field slot
(`set_instance_fields(instance, vec![])`), so a future recursive `List($T) { next: *List(T); }` sees the
identity already present and does not loop — ADR-0015 §1's identity-before-fields fixpoint, applied per
instance. The `struct($T)` *template* itself resolves its field `T` to a quiet `PoolId::ERROR` (variables
bound to `ERROR`, no diagnostic), and that template entry's fields are never read, because every reference
reads the instance map.

### 4. Everything ADR-0085 §5 defers is a compile-time refusal, not a gap

- Inferring a struct's argument through a `$T` procedure parameter — `(b: Box($T))` — binds nothing:
  `infer_var_in` and `collect_poly_in_type` leave `TypeRef::Apply` unmatched, so a `Box(s64)` parameter is
  an ordinary concrete type and a `Box($T)` parameter simply does not bind `T`.
- `using` on a parameterised struct promotes nothing: `type_ref_name_in` returns `None` for `Apply`.
- A cross-file or non-parameterised constructor is **E0269**; a wrong argument count is **E0270**. Each
  names the limit rather than half-supporting it.

Multiple struct type parameters (`Map($K, $V)`) *do* resolve — the path zips variables against arguments —
though the differential corpus file exercises one, matching how ADR-0083 staged the procedure case.

## Consequences

- The pool's identity invariant now reads "a struct's identity is its declaration site **and its type
  arguments**", and `fields_of` is the one lookup a consumer holding a type should use — extracting `decl`
  and calling `struct_fields` directly gets the unsubstituted template, correct only for an ordinary struct.
- Both engines needed **no new node**: an instantiated struct is an ordinary aggregate whose fields came
  from a substitution, so `layout_of`/`field_offset` keying on the instance is the whole back-end change
  (ADR-0085 §4 predicted this, and it held).
- The teeth-check is recorded: forcing `resolve_apply` to forget the substitution makes `Box(s64).value` an
  error type and the program is *refused*, not miscompiled — the poison gate holds because the substituted
  field type is the thing every downstream layout and access reads.
- A `Type_Info` of a parameterised struct will report its instantiated fields once ADR-0078's variable-length
  field list exists; that tie to the deferred RTTI piece (ADR-0085's consequences) is unchanged.

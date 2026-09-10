# ADR-0229: Polymorphic calls infer through parameterised nominal types

- **Status:** Accepted
- **Date:** 2026-09-10
- **Decider:** dboll

## Context

The requested generic containers need operations shaped like:

```text
Table :: struct($K, $V) {
  // ...
}

put :: (table: *Table($K, $V), key: K, value: V) {
  // ...
}
```

Jairs already resolves `Table(string, string)` as a nominal struct instance, including when `Table`
is imported. It also infers `$T` directly and through pointers, views, vectors and dynamic arrays.
Those two capabilities did not compose: signature collection deliberately stopped at
`TypeRef::Apply`, so `put` was not recognised as a template unless `K` or `V` appeared somewhere
else, and call inference likewise learned nothing from an actual `*Table(string, string)`.

The broader requested wave also includes instantiating a procedure declared in another module.
That is a separate ownership problem. Current instantiation appends a clone to the file being
checked, while an imported template must be cloned into its declaration file so its private names,
imports and source identity remain correct. Removing E0268 without that program-scoped expansion
would recreate the old `no routine for file N proc M` failure. The two changes therefore land as
consecutive green waves: this ADR closes the inference substrate; the next owns cross-file
materialisation.

## Decision

### 1. `TypeRef::Apply` participates in template recognition

The polymorphic-variable collector descends into every type argument of `Name(args)`.
`*Table($K, $V)` therefore introduces `K` and `V` in the same first-seen order as direct `$K` and
`$V` parameters.

This is still one source type tree. No parallel generic-signature grammar is introduced.

### 2. Inference matches nominal declaration identity, then recurses through arguments

For a parameter `Table($K, $V)` and an argument of resolved type `Table(string, s64)`, inference:

1. resolves the parameter's constructor to its declaration identity;
2. requires the argument to be a `StructType` with that exact `DeclId`;
3. requires the source and resolved argument lists to have the same length; and
4. recursively matches each source type argument against the corresponding concrete `PoolId`.

The rule applies under the existing pointer, view, vector and dynamic-array peeling, so the common
container shape `*Table($K, $V)` works without a special case.

Matching by declaration identity rather than by spelling is required: two modules may each export a
`Table`, and equal names do not make their values interchangeable. Matching by field layout is also
wrong because Jairs structs are nominal.

### 3. Existing binding and checking rules remain authoritative

The first occurrence binds a variable. A later occurrence is checked against that binding when the
concrete signature is resolved, exactly as direct and pointer inference work today. This remains
one-directional inference from argument types, not general unification: there is no occurs-check,
backtracking or inference from the return context.

A constructor mismatch contributes no binding. If no other parameter pins the variable, the
existing E0268 reports that not every `$T` could be inferred; if another parameter pins it, ordinary
argument checking reports the nominal type mismatch.

### 4. The constructor may be local or imported

The existing parameterised-struct lookup already returns the declaring file and struct id for both
local and imported constructors. Inference reuses that lookup, so a local polymorphic procedure may
infer through an imported `Box($T)` without inventing a second module lookup.

This does **not** instantiate an imported polymorphic procedure. E0268 remains for that call until
the next root-scoped specialization wave materialises the clone in its owner file and redirects the
caller to its full procedure reference.

## Rejected alternatives

- **Infer from the field list.** Structs are nominal; equal fields are not equal types, and resolving
  fields merely to infer arguments repeats layout work unnecessarily.
- **Match the constructor's text.** A flat import can contain same-spelled declarations from distinct
  modules. `DeclId` is the identity the pool and layout already use.
- **Add explicit call-site type arguments instead.** That would add syntax and still leave the common
  `put(&table, key, value)` shape unable to infer what its argument already states.
- **Remove the imported-template refusal in this wave.** The current clone belongs to the caller's
  HIR. An imported body resolved there would read the caller's item indices and imports, which is a
  silent wrong-program risk rather than a partial implementation.

## Consequences

- Same-file generic `List` and `Hash_Table` operations can use their native parameterised container
  type in signatures.
- The corpus pins two variables inferred through one nominal instance and the pointer wrapped around
  it.
- An imported parameterised struct can be an inference pattern for a local procedure.
- Cross-file procedure specialization remains the next wave, now with the inference half already
  complete and independently committed.

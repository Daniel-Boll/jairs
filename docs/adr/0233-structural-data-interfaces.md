# ADR-0233: `$T/interface Shape` is a compile-time field constraint over concrete specialisation

- **Status:** Accepted
- **Date:** 2026-09-10
- **Decider:** dboll

## Context

The requested data interface is structural at the call boundary and concrete everywhere after it:

```text
Vector3 :: struct {
  x: float32;
  y: float32;
  z: float32;
}

process_position :: (position: $T/interface Vector3) {
  // may use Vector3's required fields
}
```

A caller's type may declare the fields in another order, may contain additional fields, and may
provide a required field through `using`:

```text
Player :: struct {
  name: string;
  hp: u16;
  y, z, x: float32;
}

Positioned :: struct {
  using position: Vector3;
  name: string;
}
```

The existing `$T` path already infers a concrete type, clones the template and checks and lowers the
clone under that binding. The interface therefore does not need a run-time representation. The
remaining decisions are what counts as a field match, when ambiguity is rejected, and how sema and
MIR agree on a promoted field's concrete projection.

Three independent designs converged on a compile-time constraint and on centralising field lookup.
The repository currently has two different algorithms: sema searches promoted fields
breadth-first, while MIR recursively accepts the first depth-first match. That disagreement can
silently select a different offset, so this wave cannot safely add a third lookup for interfaces.

## Decision

### 1. `$T/interface Shape` constrains an existing polymorphic variable

`interface` is contextual: it is recognised only after `/` inside a `POLY_TYPE`, so an ordinary
declaration may still use `interface` as an identifier.

The HIR keeps the source shape on the variable:

```text
Poly {
  name: T,
  interface: Shape,
}
```

The resolved procedure signature keeps each variable and its optional resolved interface together.
The constraint affects whether an inferred type may instantiate the template; it does not enter
procedure type identity or the existing structural instantiation key.

Only a struct may be an interface shape. The shape's directly declared fields are its requirements.
Its field order, offsets, `#align` and `#place` are irrelevant. A future wave may decide whether
promotion inside the shape itself expands its requirements; this one does not infer an interface
from hidden paths.

### 2. Matching is by visible name and exact resolved type

For every field directly declared by `Shape`, the inferred concrete type must expose one field with
the same name and exact resolved `PoolId`.

- Requirement order does not matter.
- Additional concrete fields are allowed.
- A direct concrete field wins over promoted fields.
- Otherwise the shallowest `using` promotion depth wins.
- Exactly one match must exist at that depth.
- Two matches at the winning depth are ambiguous and reject the call.
- A pointer-valued `using` field is followed with the same auto-dereference rule ordinary field
  access uses.
- Only real struct fields participate. Pseudo-fields such as `string.count` do not satisfy an
  interface.

The constraint applies to the variable itself. Both value and pointer parameter forms therefore
compose with existing inference:

```text
read  :: (value:  $T/interface Shape) { ... }
write :: (value: *$T/interface Shape) { ... }
```

The pointer form peels the pointer, binds `T` to its pointee and checks that pointee.

### 3. One pool-owned lookup returns concrete projection evidence

`jr-pool`, where every resolved field list already meets, owns one pure visible-field lookup. Its
result is missing, ambiguous, or a unique field with:

- the final resolved field type; and
- a path of `Deref` and concrete `Field(index)` steps.

The search auto-dereferences the root, prefers a direct field, then walks `using` bases
breadth-first. It detects cycles per path and reports all matches at the shallowest winning depth.
It reads `Pool::fields_of`, so parameterised struct instances use their substituted field types.

Sema uses this result for ordinary field access and interface conformance. MIR consumes the same
path rather than searching by name again. Target-specific byte offsets remain derived from the
concrete type by `jr-pool`; an interface shape's layout never enters generated code.

### 4. Constraint failure is diagnosed before a clone is recorded

After ordinary `$T` inference has bound every variable, `check_polymorphic_call` validates each
resolved interface against its bound type. A failure reports E0299 at the call and names:

- a non-struct interface shape or candidate;
- a missing field;
- a field whose resolved type differs; or
- an ambiguous field reached through multiple `using` paths.

Malformed `/interface` syntax is E0135.

A successful call records the same `(TemplateRef, [PoolId])` key as an unconstrained `$T` call.
The concrete clone is checked normally and its field expressions receive the candidate's real
projection paths. No vtable, witness object, interface value, coercion, MIR operation or backend
ABI is added.

### 5. The uninstantiated body may see only the declared shape

The template's own body uses the interface shape as its checking surface, while the template still
remains unbound for specialisation and compile-time evaluation. This allows `value.x` when `x` is a
requirement and rejects dependence on a caller-specific extra field.

Concrete clones are rechecked under the real binding. Layout-sensitive operations and nested
specialisation continue to derive their executable facts from the clone, never from the shape
witness.

If preserving that distinction requires separate body-facing parameter types rather than binding
`T` globally to `Shape`, use the separate representation. A shape witness must not masquerade as a
real instantiation binding.

## Rejected alternatives

- **A runtime interface value or vtable.** Every call already specialises to one concrete type, so
  this adds representation, ABI and dispatch cost with no current use.
- **Treat the interface struct's layout as the concrete layout.** Reordered fields would compile to
  wrong offsets, contradicting the requested feature.
- **Check only direct concrete fields.** The requested `using position: Vector3` alternative would
  fail despite ordinary field access accepting it.
- **Take the first promoted match.** It makes declaration order decide meaning and preserves the
  current sema/MIR disagreement.
- **Keep separate sema and MIR searches.** A disagreement is a silent wrong address; projection
  evidence must be decided once.
- **Match assignability or recursively structural field types.** The language's nominal types stay
  nominal. A required `Point` field means that resolved `Point`, not any same-shaped declaration.
- **Reserve `interface` globally.** The spelling needs one contextual marker and does not justify
  breaking existing identifiers.
- **General method/operator requirements in this wave.** The source request is a data-field
  interface. A future requirement enum can extend the resolved constraint without inventing
  witness plumbing before a caller needs it.

## Consequences

- Reordered structs with extra data satisfy a shape without changing their nominal identity.
- A `using`-embedded `Vector3` can satisfy the same procedure as direct `x`, `y` and `z` fields.
- Ambiguous promotion is rejected at constraint checking rather than deferred into a body or left
  for MIR to choose.
- Value and pointer parameters share the existing inference and specialisation path.
- MIR and sema gain one field-resolution contract, closing an existing wrong-offset risk beyond
  interfaces themselves.
- Because MIR field projection changes, this wave requires gate 7 as well as the six ordinary
  gates.

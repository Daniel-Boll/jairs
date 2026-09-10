# ADR-0239: Typed and inferred struct literals construct record-like values

- **Status:** Accepted
- **Date:** 2026-09-10
- **Deciders:** dboll
- **Scope:** `T.{...}` and context-inferred `.{...}` for nominal structs and `string`.
  Inferred arrays, unions, variants, views, dynamic arrays, contexts and multi-results remain
  separate decisions.

## Context

Jairs can declare, copy, return and reflect structs, but cannot construct one as an expression.
The only source spelling is a declaration followed by field assignments. Both Jai-shaped forms
currently fail in the parser:

```jr
point := Point.{x = 1, y = 2};
return .{data = start, count = finish - start};
```

The second form blocks the counted-string scanners in the `ora_to_atlas_test` probe. Its return
type is `string`, so the intended aggregate type is already known. Jairs passes expected types into
returns, typed initialisers, assignments and concrete call arguments; the missing work is a literal
that consumes that context.

There is no typed struct literal implementation for the inferred form to reuse. Fixed array
literals (`T.[...]`) provide the nearest syntax and MIR precedent, while ordinary aggregate locals
provide the runtime representation: allocate a slot, zero it, store projected members, then load
the completed value.

## Decision

### 1. One syntax supports explicit and contextual types

`T.{...}` names its type explicitly. The expression before `.{` is resolved in type position, as
the element expression of `T.[...]` is.

`.{...}` requires a concrete expected type. Returns, typed local initialisers, assignments,
concrete call arguments and nested literal fields may supply it. An unconstrained binding such as
`value := .{x = 1};` is E0300 rather than inventing a structural type. A polymorphic `$T` parameter
does not infer itself from an untyped literal; write the explicit `Point.{...}` form.

The parser uses a dedicated `STRUCT_LITERAL` with dedicated entry nodes. `.{` is distinct from the
bare member `.RED`, and `T.{` is distinct from `T.field`, by one token of lookahead.

### 2. Entries are named or positional, never mixed

Named entries use `field = expression`, may appear in any order, and resolve only direct public
fields of the constructed value. Duplicate and unknown names are diagnostics. A `using`-promoted
field is not an initializer name in this slice because one spelling could otherwise select storage
through several embedding paths.

Positional entries initialize direct fields in declaration order. A literal may contain fewer
entries than fields; supplying more is E0300. Mixing named and positional entries is E0300 because
the meaning of a positional entry after a reordered named one is not self-evident.

`.{}` and `T.{}` are valid zero/default values. The whole aggregate is zero-initialized before
supplied entries overwrite fields, so every omitted field has the same value as a default-declared
local of that type. Struct declarations currently carry no per-field default expressions, so there
is no second defaulting mechanism.

Initializer expressions execute once in source order. Named-field reordering changes destinations,
not evaluation order.

### 3. The first constructible record kinds are structs and `string`

Nominal structs, including parameterised instances, use their substituted direct field list.

`string` participates through its existing public pseudo-fields:

- `data: *u8`
- `count: s64`

MIR uses `StringData` and `StringCount`, not invented nominal field indices. Positional string
initialisation follows Jairs' public order above; named initialisation is order-independent.

Other aggregate-shaped types stay excluded:

- a union needs an active-field rule;
- a variant needs a tag/case rule;
- a view or dynamic array raises borrowing and ownership questions;
- `Context` is compiler-owned state;
- multi-results are deliberately not storable values.

### 4. Sema owns the construction plan

Sema checks each value against its resolved destination field type and records the destination
projection for every entry. MIR consumes that plan rather than resolving names or field order
again.

Lowering creates a slot of the literal's type, emits `Zero`, evaluates entry expressions in source
order, stores each into its recorded projection, then loads the whole aggregate. The VM, Cranelift
and LLVM already implement zeroing, projected stores, aggregate loads and copies; no engine
primitive is added.

The literal is a value, not a place. Its materialisation slot is an implementation detail, as with
an array literal.

### 5. Direct compile-time literals remain deferred

A direct file-scope struct literal is refused through the existing E0230 compile-time-expression
boundary, matching fixed array literals. A `#run` may call an ordinary procedure that constructs
and returns one; the existing aggregate-value interning then carries the result.

This keeps runtime construction separate from the unresolved question of which aggregate source
expressions are admissible directly in constant evaluation.

### 6. Multiline literals use a configurable trailing comma

`jr_fmt::Config::struct_literal_trailing_comma` and
`[fmt] struct_literal_trailing_comma` are booleans, defaulting to `true`.

For a non-empty multiline struct literal, `true` ensures the last entry has a comma, making field
addition and reordering one-line diffs. `false` removes only that final comma. Commas between
entries remain required, and compact one-line literals plus empty `.{}`
are unaffected.

The setting is specific rather than a formatter-wide `trailing_comma` switch because procedure
parameters and wrapped call arguments already have their own unconditional grammar-preserving
policy. One broad boolean would make unrelated constructs change when the caller asked about
record literals.

## Rejected alternatives

### Implement only inferred named literals

Rejected because the parser, CST, HIR and construction plan are the expensive part. Supporting
`T.{...}`, positional entries and `.{}` over that same representation is small, closes the typed
struct-literal gap recorded since ADR-0039, and avoids an immediate second syntax wave.

### Infer a structural type from the field names

Rejected because Jairs structs are nominal. Two declarations with the same fields are different
types, and a literal with no context cannot choose between them.

### Permit every aggregate-shaped type

Rejected because equal-looking storage does not imply equal construction semantics. Unions,
variants, dynamic arrays and contexts each need decisions this counted-string use does not force.

### Resolve fields again in MIR

Rejected because sema and lowering could then disagree about a name, a parameterised field type or
an entry's destination. One recorded construction plan makes the checked field the stored field.

## Consequences

- The motivating `return .{data = start, count = ...};` expressions construct `string` values.
- Jai-shaped typed, inferred, named, positional and empty struct literals share one syntax node and
  one lowering path.
- Omitted fields are predictably zero-filled; named initializers remain source-order expressions.
- A direct file-scope literal remains E0230, pinned separately from a `#run` procedure that
  constructs and returns the same aggregate.
- Formatter and both editor grammars must learn the new expression and entry shape; multiline
  literals gain a default-on, manifest-configurable final comma.
- Gate 7 is required because aggregate MIR and three-engine differential coverage are involved.

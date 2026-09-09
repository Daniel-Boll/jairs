# ADR-0227: Parenthesised results may carry declaration-only labels

- **Status:** Accepted
- **Date:** 2026-09-09
- **Deciders:** dboll
- **Amends:** ADR-0052 §1 and §4.

## Context

Jairs already returns one value as `-> T` and several as `-> (T, U)`. The result positions are
structural and positional: callers destructure them in order, and a one-element result list normalises
to the element type.

The missing source shape is a label on a result:

```text
expect_char :: (s: *string, c: u8) -> (exists: bool) {
  todo;
}
```

The request is for a label "just for the sake of it": information in the declaration, not an implicit
local variable or a second calling convention. The pinned Way to Jai reference supports that reading.
Its named-result examples still declare body locals and return them explicitly; defaulted results and
bare or partial returns are a separate feature with different semantics.

## Decision

### 1. Each entry in a parenthesised result list may have a label

These are legal:

```text
-> (exists: bool)
-> (value: s64, found: bool)
-> (value: s64, bool)
```

The label is optional per position, so named and unnamed entries may be mixed. The parenthesised form is
the only named-result spelling in this wave. Jairs keeps ADR-0052's existing canonical shape rather than
also admitting Jai's unparenthesised `-> value: T, found: bool` form.

Two positions in one declaration may not carry the same label. E0298 reports the duplicate at its second
spelling. A duplicate communicates that two distinct positional results have the same role and therefore
defeats the label's only purpose.

Rejected alternatives:

- **Require every position to be named once one is.** Mixed declarations are useful during migration,
  and names do not affect calling or typing.
- **Allow duplicate labels.** They are legal metadata but misleading metadata.
- **Add the unparenthesised form now.** It widens the return grammar beyond the source shape requested
  and reopens ADR-0052's deliberate parenthesised result-list syntax for no semantic gain.

### 2. A result label is metadata, never a binding

Labels do not enter the procedure body's scope. A parameter or local may reuse the same spelling, and an
explicit `return value;` or `return a, b;` remains required on every reached returning path.

This wave does not add:

- default result values;
- bare `return;` from a non-void procedure;
- partial positional returns;
- named assignments in a `return`;
- call-site selection by result name.

Those forms choose values, while this feature only describes positions. Mixing the two would turn a
frontend-only declaration feature into new control-flow and MIR semantics.

### 3. Labels never participate in type identity

`-> (exists: bool)`, `-> (bool)` and `-> bool` are the same procedure return type. A multi-result
procedure's interned type remains the ordered list of element types only. Procedure-pointer assignment,
overload identity and call checking therefore cannot change when a label is renamed.

HIR carries one optional `(name, span)` per written result position on the procedure declaration. The
metadata lives there rather than on the interned results type, because the pool deliberately normalises a
one-element list to its scalar and because two declarations with identical types may use different labels.
`ProcSig` mirrors the names so an importing file's tooling can render them without the declaring file's HIR.

MIR and all three engines remain unchanged.

### 4. Tooling treats labels as signature declarations

The formatter preserves labels and prints one space after `:`. Tree-sitter gives each position its own
`result` node with optional `name` and required `type` fields; the generated parser is committed.

LSP hover, completion detail, document symbols and signature help render the labels. Semantic tokens
classify a result label as a parameter declaration because it names one position in a procedure's public
signature, while definition and references deliberately offer nothing: the label is not a body binding and
has no use site.

The executable Way-to-Jai baseline gains a second chapter 17 probe, taking it from 35 to 36 probes
across the same 35 example-bearing groups. The owned port uses Jairs' parenthesised labels and records
the remaining difference explicitly: the pinned guide also demonstrates an unparenthesised spelling,
default values and implicit result bindings, none of which this decision admits.

## Consequences

- The requested `-> (exists: bool)` source parses, formats, checks and appears faithfully in editor
  signatures.
- Existing unnamed result source and every ABI remain unchanged.
- Renaming a result label is non-semantic.
- The guide comparison tests the supported subset and preserves the broader forms as named gaps.
- Defaulted/named return assignment remains an explicit compatibility gap rather than a half-implemented
  consequence of accepting a colon.
- Gate 7 is unnecessary: no MIR, layout or back-end code changes.

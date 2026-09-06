# ADR-0212: the formatter preserves `#program_export`

- **Status:** Accepted
- **Date:** 2026-09-06
- **Deciders:** dboll
- **Amends:** ADR-0197 §2's exported-symbol surface and the formatter inventory.

## Context

The formatter optimisation audit probed every procedure attribute against the emitter that walks
them. `#program_export` was absent:

```jairs
exported :: (a: s64) -> s64 #c_call #program_export { return a; }
```

formatted to:

```jairs
exported :: (a: s64) -> s64 #c_call {
    return a;
}
```

The result still parses and type-checks. Its meaning changes later: ADR-0197 makes an exported
procedure use its source name so C can link it, while an unmarked procedure gets a private mangled
symbol. Formatting a library therefore removed part of its ABI without a diagnostic.

This escaped every existing formatter test. The attribute appeared in Rust-generated integration
fixtures, not in the non-recursive `tests/corpus/valid` directory the formatter's corpus tests walk.
The procedure-attribute emitter ends in `_ => {}`, so a new attribute compiles and disappears.

## Decision

### 1. `PROGRAM_EXPORT_ATTR` is emitted exactly where it appears

The procedure-attribute loop emits ` #program_export`, in source order beside `#c_call`,
`#no_abc`, `#must`, `#c_variadic`, `#expand`, `#modify` and notes.

The regression writes both legal orders with `#c_call`, asserts the exact spellings survive, then
asserts idempotence and reparsing. A parse-only assertion is insufficient because the broken output
was valid Jairs.

**Rejected: rely on the surrounding node's raw-text fallback.** The procedure itself is already a
known formatted node. Its attribute children are consumed by this loop, so the fallback is never
reached.

**Rejected: treat this as only an integration-link test.** The existing static/dynamic-library
tests already prove `#program_export` reaches codegen. They build source directly and never format
it first, so they cannot catch the formatter deleting the marker.

### 2. The exhaustiveness redesign remains a separate decision

The audit also found duplicated node-role allowlists and wildcard fallbacks throughout `jr-fmt`.
Replacing them with one typed, exhaustive registry is the recommended follow-up, but it is a
structural change across `jr-syntax` and the whole formatter. This emergency correction does not
pretend one added arm fixes that architecture.

## Consequences

- Formatting no longer silently removes a library export.
- One formatter unit test is added; no corpus file, module or engine changes.
- The broader formatter safety/configuration/diff wave remains explicit rather than hidden by this
  patch.

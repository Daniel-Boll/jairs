# ADR-0005: Polymorph instantiation identity is structural

- **Status:** Accepted
- **Date:** 2026-07-25
- **Deciders:** dboll

## Context

When a polymorphic procedure such as `sort($T)` is instantiated with a concrete
argument, the compiler must decide when two instantiations are *the same*
instantiation. If `sort(Entity)` is reached independently from two different
files, do we generate one function or two? The answer determines the key by
which instantiations are cached, and that key design lives in the InternPool —
which is built in the Jairs-0 slice, long before polymorphs (wave W5) exist.

Two identity models:

1. **Nominal identity** — keyed on the syntactic site or a per-call token, so
   the "same" call from two files could be two instantiations unless extra work
   dedupes them.
2. **Structural identity** — keyed on the tuple of *resolved, interned
   comptime-argument IDs*. Two calls with the same resolved arguments produce the
   same key and therefore the same instantiation, regardless of where they were
   written.

Once `Type` becomes a first-class comptime value (wave W4), structural identity
is forced anyway: a type argument *is* an interned value, and equal values must
key equally. Choosing structural now aligns the InternPool key design with where
the language is going.

## Decision

Polymorph instantiation identity is **structural**: an instantiation is keyed by
the tuple of resolved comptime-argument IDs in the InternPool. `sort(Entity)`
reached from two files interns to the same argument tuple and therefore dedupes
to one generated function.

Errors, however, **display nominally**: a diagnostic shows `sort($T = Entity)`,
the user's intent, rather than the internal InternPool key. Identity is
structural; presentation is nominal.

## Consequences

### Positive

- Automatic cross-file de-duplication of instantiations with no extra pass.
- The key model is already correct for wave W4's first-class `Type` values, so no
  rework when RTTI lands.
- Diagnostics stay legible because they render intent, not keys.

### Negative

- The InternPool must canonicalise comptime argument values well enough that
  "equal arguments" reliably produce "equal keys"; a weak canonicaliser would
  silently split or merge instantiations.

### Follow-on work this forces

- **Into the slice:** this decision fixes the InternPool key design in `jr-pool`
  now, even though no polymorphs exist yet — the key must be a tuple of interned
  comptime-value IDs.
- **Into wave W4:** first-class `Type` values must intern as ordinary comptime
  values so that type arguments key structurally like any other.
- **Into wave W5:** instantiation caching and the nominal error *display* layer
  are built on top of the structural key.

## Alternatives considered

- **Nominal identity.** Rejected: it would generate redundant instantiations for
  identical cross-file calls and, more importantly, it is incompatible with
  first-class `Type` values in W4 — where equal type-values must key equally —
  forcing a switch to structural later anyway. Better to pay for structural
  identity once, now.

# ADR-0092: `type_info(T)` describes a bound type variable — reflection over what polymorphism binds

- **Status:** Accepted
- **Date:** 2026-08-04
- **Deciders:** dboll
- **W5 sub-wave 7c.** Written while designing `#modify`, which needs it: a compile-time predicate over an
  instantiation must be able to *ask something* about the bound type, and it could not.

## Context

`#modify` is the next macro piece — a block that runs at compile time when an instantiation is being decided
and can reject it. Designing it surfaced a blocker that is worth more than `#modify` itself:

```jr
size_of :: (x: $T) -> s64 {
    return type_info(T).size;      // error[E0261]: `type_info` needs a type
}
```

**Reflection over a bound type variable did not work.** `described_type` asked resolution what `T` names, got
`Res::Error` (a type variable is no declaration), then asked `builtin_type_named("T")`, which found no
builtin — so the described type was `None` and E0261 fired. A `#modify` predicate would have had nothing to
predicate on, and more importantly a `$T` procedure could not reflect on its own parameter at all, which is
the most obvious thing polymorphism plus RTTI should allow.

Found by writing the feature, per AGENTS.md — not by reading the plan, which listed `#modify` as ready.

## Decision

### 1. `described_type` consults the type bindings first, and the bindings are seeded per body

Three pieces, each the same shape as one built for `$N`:

- **`described_type` checks `Ctx::type_bindings` before resolution**, the way `resolve_type_name` already
  does (ADR-0081 §1): inside an instantiation, `T` *is* the bound type, and a same-named declaration must not
  shadow it.
- **`check_file`'s body loop seeds those bindings** from `FileHir::proc_bindings` for the body's procedure —
  they had only ever been set transiently inside `check_polymorphic_call`, so during a body check the map was
  empty. Seeded **per body** and cleared after, for the reason ADR-0089 §1 gives for `$N`'s values: two
  instantiations of one template share the variable name `T` with *different* bindings, and leaving one set
  would describe the wrong type in the other's body — a silently wrong `size`, not an error.
- **A template's own `type_info(T)` is withheld, not refused.** A template has no binding; the program is
  correct and each instantiation resolves `T` for real. `Ctx::poly_var_names` (seeded per body from the
  signature's `poly_vars`) distinguishes "names a type variable, so wait for the instantiation" from "names
  nothing, so refuse" — the same withholding `[N]T` gets for an unknown length (ADR-0089 §2), and the same
  shape as `jr-hir`'s withheld E0201 inside a pending `#insert` (ADR-0073 §1).

### 2. An instantiation's `Type_Info` constants are folded against the instantiation's own check

`file_consts` folds `type_info` calls from the **base** check — where a template's call was withheld and
recorded nothing. So an instantiation's `type_info(T)` had no folded value, `scan` refused the body, and it
surfaced as `internal compiler error: no routine for file 0 proc 2` — the sixth time internals have leaked
for a reasonable program.

`file_mir` now folds the instantiation's `type_info_calls` too, against `inst.check` (where `T` *is* bound),
using the **same** `type_info_value` `file_consts` uses. Sharing that function rather than repeating it is
the point: two builders of a `Type_Info` would be two chances to disagree about its shape, which is exactly
what ADR-0075 §2's validated field table exists to prevent.

## Consequences

- **A `$T` procedure can reflect on its own bound type**: `type_info(T).size`, `.count`, and an `.id`
  comparison against a concrete type's all work, at each instantiation's own type. `valid/076` exits 42 with
  two instantiations reflecting different types (`s64` → 8, `u8` → 1), and the MIR snapshot shows each
  instantiation storing its **own** folded `Type_Info` — `{#id, 2_enum, "s64", 8_s64, …}` in one.
- **`#modify` is unblocked**: a predicate can now ask `type_info(T).id == type_info(s64).id`, which is the
  sound identity comparison ADR-0077 established. That was the point of doing this first.
- **No new diagnostic code.** E0261 is *withheld* in one new case; nothing new is refused.
- This is the pattern's third instance — a binding consulted first, seeded per body, withheld in the
  template (`$T` types here, `$N` values in ADR-0089, `$T` in signatures in ADR-0081). The next feature that
  needs a per-instantiation fact should follow it rather than inventing a fourth channel.

# ADR-0089: An array length may name a `$N` comptime parameter, read from the instantiation's baked value

- **Status:** Accepted
- **Date:** 2026-08-04
- **Deciders:** dboll
- **W5 sub-wave 6c.** `buf: [N]s64` inside a `make :: ($N: s64)` — the case comptime-value parameters exist
  for, and the last piece of the `$N` feature after ADR-0087 (surface) and ADR-0088 (instantiation).

## Context

ADR-0088 made a `$N` call run by baking each argument's value into a cloned procedure. What it did *not*
unlock is the useful case: an array whose **length** is that parameter. `buf: [N]s64` needs `N`'s value at
the point the array *type* resolves, and `constant_array_length` (ADR-0070 §1) looked only in
`hir.scope` — file-level constants. So `[N]s64` inside a `$N` procedure was E0233, verified by running
before this ADR was written.

The constraint that shaped this is ADR-0039 §3a: **sema has no constant evaluator**, because const-eval
lives in `jr-db` over the VM, downstream of type resolution (ADR-0018 §3). ADR-0070 §1 already showed the
way around it for a file-level constant — sema *reads* a literal that is already in the HIR rather than
evaluating anything — and the same move is available here, because ADR-0088's pre-pass already interned the
value.

## Decision

### 1. The baked value reaches sema through the HIR, on `FileHir::param_values`

`expand_instantiations` records `(ProcId, parameter name, value PoolId)` for each `$N` parameter it bakes —
the value-side counterpart of `FileHir::proc_bindings`, which ADR-0082 §2 added for a bound `$T`. The
signature phase seeds a `Ctx::value_bindings` map from it around the procedure it is resolving, and
`check_file`'s body loop re-seeds it per body; `constant_array_length` consults that map **first**, for the
same reason `resolve_type_name` consults `type_bindings` first — inside an instantiation, `N` *is* that
parameter, and a same-named file constant must not shadow it.

**Why a side table rather than rewriting the `TypeRef`.** A parameter's or return type's `TypeRef` lives in
the shared `FileHir::type_refs` arena (which `copy_type_ref` already clones per instantiation), but a
*local*'s annotation lives in the **body's** arena — and `buf: [N]s64` on a local is the common case. A
rewrite would therefore need two paths with two arena rules; the side table has one.

**Why sema still runs no evaluator.** The value arrives already interned, produced by ADR-0088's pre-pass,
carried in on the HIR. `jr-sema`'s `Cargo.toml` still names neither `jr-db` nor `jr-vm`, so ADR-0039 §3a's
constraint is honoured rather than inverted — exactly as ADR-0070 §1 honoured it.

**Why re-seeded per body rather than left set.** Two instantiations of one template share the parameter
name `N` with *different* values. Leaving the last-written binding in place would give the second
instantiation's length to the first's body — a silently wrong array size, which is the failure mode this
project is organised around. So both the signature phase and the body loop clear what they set.

### 2. A **template**'s own `[N]T` resolves to a placeholder, and its length-dependent checks are withheld

A template has no value for `N` — only its instantiations do — so `[N]s64` there cannot resolve. Reporting
E0233 would be a false error about a correct program, so it is **withheld**: the type resolves to `[0]s64`
and the array is recorded in `Ctx::placeholder_arrays`. Every check that reads a *length* consults that set
and withholds; concretely, the literal-index range check (E0236) does, because `buf[0]` against `[0]s64`
would otherwise report "index 0 is out of range" for code that is fine.

**Why a placeholder rather than skipping the template's body.** ADR-0087 §2's whole point is that a `$N`
template's body **is** type-checked — its parameter types are known, only the value varies — and that
catches body errors a sub-wave early. Skipping the body to avoid the placeholder would give that up. The
placeholder is safe because the template is never lowered: `ProcSig::is_template` skips its MIR and its
native declaration (ADR-0087 §2), so no code is generated against `[0]s64`.

**Why this is not a well-typed placeholder of the dangerous kind.** PLAN §5's named failure mode is a
placeholder that reaches *code generation* as a legitimate value. This one reaches only the template's own
type, and the template produces no code at all. Each instantiation resolves a real length and is checked
normally — which is where a genuinely out-of-range index is still caught.

## Consequences

- **`$N` is complete**: a comptime-value parameter can size an array, which is what the feature is for and
  what the standard library's fixed-capacity structures (W7) will use.
- **Two instantiations get two array types**, verified in the MIR snapshot: `$inst0` has a `[4]s64` slot and
  `$inst1` a `[3]s64` slot from one declaration. The corpus file sums 1..N in each and exits 16, asserted in
  both engines — a shared or leaked length would change the total.
- **No new diagnostic code.** E0233 is *withheld* in one new case rather than joined by a sibling; E0236 is
  withheld in the same case. Nothing new is refused, so the code count is unchanged.
- Teeth-checked: clearing the baked bindings makes the instantiation report E0233 rather than compiling with
  a wrong length — a refusal, not a miscompile.
- Still deferred, unchanged: `[2 + 2]u8` and any length needing *arithmetic* (ADR-0070's own deferral), and
  a length naming a constant from another file.

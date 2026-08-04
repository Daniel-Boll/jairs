# ADR-0097: A `#bake_arguments` declaration produces a specialised procedure — and W5 closes

- **Status:** Accepted
- **Date:** 2026-08-04
- **Deciders:** dboll
- **W5 sub-wave 7h**, the last. ADR-0096 delivered the surface and refused the specialisation (E0276). This
  produces it, lifting the refusal for a bake whose values are literals. **W5 — Polymorphism is complete.**

## Decision

### 1. The specialised procedure is a clone with the baked parameters dropped

`add_five :: #bake_arguments add(a = 5);` lowers to a **real `ConstValue::Proc`**: a clone of `add` with the
parameter `a` removed from its list and `5` substituted for every use of it in the body. So `add_five` is an
ordinary one-argument procedure — callable, lowerable, inlinable — and nothing downstream is taught about it.

Three steps, which are ADR-0088 §3's exactly:

1. **Drop** the baked parameters from the clone's list.
2. **Substitute** each baked parameter's `Res::Param` use with its literal.
3. **Remap** the kept parameters' indices, since earlier ones may have been dropped.

Applied during *lowering* rather than in `instantiate.rs`, because a baked procedure is a **declaration**, not
an instantiation. The reuse is the point: W5's last piece is a reuse of the polymorphism machinery rather than
a new mechanism, and step 3 is exactly what would silently read the wrong parameter if skipped — which is why
the corpus bakes both the *first* and the *second* parameter of `sub` and checks that both reach the same
answer.

The operand's arguments are read from the arg list's **children**, not `ArgList::args()`: a named argument is a
`NAMED_ARG` node and not an `Expr`, so that accessor skips every one — the trap ADR-0053 §1 records, met again
here.

### 2. A baked value must be a **literal**, and the reason is a phase order

ADR-0096 §2 planned to evaluate a baked argument through ADR-0088 §2's const-eval pre-pass. **Building it
showed that pre-pass runs after lowering**, and the value is needed *here*, where the clone is built. So the
rule is narrower: a baked value must be a literal lowering can read, and anything else is E0276 with a message
saying why.

This is the same narrowing ADR-0039 §3a took for an array length, and the same widening route is available
later: ADR-0070 §1 widened that one by reading a literal already in the HIR from a named constant, which would
work identically here.

### 3. What stays refused, each named

- A baked value that is not a literal (§2).
- An operand that does not name a procedure **declared in this file** — the clone copies a *body*, and another
  file's body is not in this HIR. A cross-file bake is deferred with the cross-file splice (ADR-0091 §3's
  boundary).
- A named argument naming no parameter, or a positional one past the end.

## Consequences

- **E0276 is lifted for the working case** and keeps the two refusals above — the fifth by-design refusal
  raised and then narrowed rather than retired, since it still has honest work.
- **W5 — Polymorphism is complete**: `$T` procedures with multiple variables and nested inference,
  polymorphic structs, `$N` comptime-value parameters including `[N]T`, `type_info(T)` on a bound variable,
  `#expand` macros that splice, `#modify` predicates that run and can reject, and `#bake_arguments`
  specialisation. Fourteen sub-waves, ADR-0081 through ADR-0097.
- `valid/078` exits **131** in both engines, and the MIR snapshot shows each baked procedure with **one**
  parameter and its literal inlined — `50_s64 - v1` for the positional bake against `v1 - 8_s64` for the
  second-parameter one, which is the remap made visible.
- Next is **W6 — Metaprogram** (workspaces, the compiler message loop, `#run build()` build scripts), then
  **W7 — Stdlib**.

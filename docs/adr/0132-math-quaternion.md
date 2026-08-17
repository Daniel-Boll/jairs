# ADR-0132: `Math` `Quaternion` — the last of `Math`'s three unkept pieces

- **Status:** Accepted
- **Date:** 2026-08-17
- **Deciders:** dboll
- **Wave 3 of eight, sub-wave 3c.** Matches ADR-0128–0131. This is the **fifth** of ADR-0127 §3's
  six unkept promises — the quaternion half of `Math`'s `vec/mat/quat`, delivered without a design
  fork owed to the decider because ADR-0130 and ADR-0131 forced every convention this wave would
  otherwise have to choose. Only three small decisions remain and are recorded at their point of
  decision: layout order, whether multiplication auto-normalises, and how the zero cases answer.

## Context

### What ADR-0130 and ADR-0131 already settled

- **The element type is `float64`** (ADR-0130 §1), because `sqrt` is `#foreign libc "sqrt"` which
  is a double. `quat_length` and `quat_normalize` reach the FFI boundary; a `float32` quaternion
  would cast at every call that matters.
- **The rotation convention is right-handed** (ADR-0131 §3), because `cross` (ADR-0130) is. A
  left-handed quaternion beside a right-handed matrix would make `quat_to_matrix4(q) * v` disagree
  with `quat_rotate(q, v)`, and that is the cross-module lie the vector-matrix pair already refused.
- **`Matrix4` exists**, and is column-major (ADR-0131 §1). So `quat_to_matrix4` has a target that
  matches the module's other rotation matrices bit-for-bit and stores through the same field.

The wave that would have needed all three of those decisions taken has instead inherited them.

### Three small decisions that still need to be made

Each is called out because the *other* answer is defensible in isolation and would be surprising
without an explanation.

## Decision

### 1. Layout is `{x, y, z, w}`, matching `Vector4`

The mathematical convention is `{w, x, y, z}` — scalar first, so multiplication reads
`(a.w, a.xyz) * (b.w, b.xyz)` in the order the derivations use. The graphics convention (and
Jairs's own `Vector4` in ADR-0130) is scalar-last. **Consistency inside `Math`** wins here: a
caller assigning `v4.x = q.x` should not need to remember which type reorders. The Hamilton
product's four component formulas do not become clearer with either order.

**Rejected: `{w, x, y, z}` (scalar-first).** The strongest argument is textbook alignment, and it
would make a component-keyed transfer between `Vector4` and `Quaternion` a silent bug (`v.x = q.x`
writing the scalar into the x slot). A convention that makes a same-name assignment mean two
different things is exactly the shape ADR-0124 §1 spent a whole wave closing for
`type_bindings`.

### 2. Multiplication does **not** auto-normalise

A unit quaternion times a unit quaternion is nearly-unit but drifts by rounding. Some libraries
divide out the length on every multiplication so the invariant holds by construction; this module
does not, so `q * q` costs six adds and sixteen multiplies with no `sqrt`.

**Rejected: normalise on every product.** The `sqrt` is a real cost, and a caller stacking a
hundred rotations paid it a hundred times for a drift that is a hundredth of what they get. Any
caller who wants the invariant can call `quat_normalize` explicitly; the module can't take that
choice out of the caller's hands cheaply. This decision is one file's convention and lives in the
`operator *` docstring.

### 3. Zero cases answer the identity

`quat_normalize(zero_quaternion)` returns `quat_identity()` rather than a NaN or a trap;
`quat_inverse(zero_quaternion)` does the same. This follows [`normalize3`]'s discipline
(ADR-0130 §3): there is no correct answer for a degenerate input, and every implementation has to
choose. **Identity** is the choice that keeps the result usable in the arithmetic that follows,
where a NaN would silently poison every subsequent component.

**Rejected: NaN.** Poisons every downstream comparison silently, which is exactly the failure mode
the vector `normalize` refused for the same reason. **Rejected: trap.** Makes a degenerate input
unsurvivable in code that is otherwise correct, and a caller who wants to distinguish tests
`quat_length_squared(q) == 0.0` first.

### 4. `quat_slerp` falls back to normalised linear interpolation at the pole

Slerp's formula is

    q(t) = (sin((1-t)·θ) · a + sin(t·θ) · b) / sin(θ)          θ = acos(a · b)

which is ill-conditioned as `θ → 0` because `sin(θ)` denominator collapses. The threshold **0.9995**
on `dot(a, b)` switches to `quat_normalize(a + (b - a) * t)`. Above that threshold the difference
between slerp and nlerp is smaller than the `sqrt` renormalisation that follows anyway, so the
fallback pays for what the primary path can't deliver at that scale.

**`acos` joins the module's libm wraps** for this — `sin`, `cos`, `exp`, `ln`, `powf`, `sqrt` and
now `acos`, whose only current caller is `quat_slerp`. Recorded at its declaration because a `#foreign`
with one caller reads like leftover scaffolding otherwise.

### 5. `quat_rotate` is an **eight-op formula**, not `q * v * q_conjugate`

The naive rotation via a full quaternion sandwich is 32 multiplications (16 per `Quaternion *
Quaternion`); the Rodrigues form used here is 18. This is the same reason `length_squared3` exists
in ADR-0130 — a routine everyone reaches for gets the optimised form, and the identity holds
across engines because both compile the same expression.

**Rejected: use `q * v * q_conjugate` for readability.** The formula is one line, the reader is
graphics-fluent by the time they meet `quat_rotate`, and the extra 14 multiplies per rotation
matter in an animation loop.

### 6. The corpus file, again, asserts on **equality**, with the two familiar exceptions

`valid/105` uses angles from `{0, π/2, π}` where possible, so `sin` and `cos` are in `{-1, 0, 1}`
and the resulting quaternions have exact components. `quat_identity` composed with itself, with a
rotation, and with its own inverse all pin exact-in-doubles. For non-trivial angles the file uses
`sin(θ/2)^2 + cos(θ/2)^2 == 1.0` as an identity that holds exactly for any libm — the sum of the
squares of any `sin`/`cos` pair, since libm agrees on both, — and pins the differential harness's
premise rather than any specific angle's rounding.

`quat_to_matrix4(q) * (v as vec4(x, y, z, 0))` is expected to agree with `quat_rotate(q, v) as
vec4` to within one `sqrt` rounding. The corpus file pins them at the exact-arithmetic cases and
lets the differential harness handle the rest.

## Consequences

- `Math` gains one type, 5 operator overloads and 13 procedures. `acos` also arrives, its first
  and (today) only caller being `quat_slerp`.
- **`Math` is now complete in the sense ADR-0115 tried to claim**: `Vector2/3/4`, `Matrix4`,
  `Quaternion` all ship with the closed-form functions and libm transcendentals, all right-handed,
  all `float64`, all cross-module-operator. The last three of ADR-0127 §3's `Math` items are met.
- **The eight-wave programme is 4 of 8 complete.** 3c closes wave 3; waves 4–7 remain, each with a
  fork already decided in PLAN §7.
- **Test count**: 1010, unchanged, since this is a library wave whose coverage rides on the
  differential and snapshot harnesses. **Corpus count**: 218 → 219 with `valid/105-math-quaternion.jr`.
- **Deferred, not declined**: `quat_from_matrix4` (multiple valid quaternions per matrix; the
  numerically stable branch selection is subtle and has no current caller). `quat_from_euler` for
  the same reason a general axis rotation was deferred from ADR-0131 §4 — the axis-angle form
  covers every current caller.

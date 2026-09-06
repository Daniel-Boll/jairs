---
title: Math
description: Exact closed-form integer and float functions computed in Jairs, libm transcendentals reached by FFI, and vector, matrix and quaternion types with operator overloads.
sidebar:
  order: 75
---

`Math` has two halves that reach `float` correctness by different routes, and that split is the
interesting thing about the module. The exact, closed-form functions are computed **in Jairs**, so both
engines produce identical bits. The transcendentals — `sqrt`, `sin`, `cos`, `exp`, `ln`, `powf` — are
**`#foreign` wraps of libm**, not Jairs approximations. It also carries `Vector2/3/4`, `Matrix4` and
`Quaternion`.

## The exact half, computed in Jairs

```jr
abs :: (x: s64) -> s64            // traps on the most negative s64 — no positive counterpart
fabs :: (x: float64) -> float64
min :: (a: s64, b: s64) -> s64
max :: (a: s64, b: s64) -> s64
clamp :: (value: s64, low: s64, high: s64) -> s64   // low is applied last, so low wins if low > high
sign :: (x: s64) -> s64           // -1, 0 or 1
pow :: (base: s64, exponent: s64) -> s64            // integer, exact; negative exponent returns 0
gcd :: (a: s64, b: s64) -> s64    // Euclid; result is non-negative
floor :: (x: float64) -> float64
ceil :: (x: float64) -> float64
round :: (x: float64) -> float64  // halves away from zero
```

```jr
#import "Basic";
#import "Math";

main :: () {
    n := 0;

    if abs(-5) == 5 && abs(5) == 5 && abs(0) == 0 {
        n = n + 1;
    }
    if min(3, 7) == 3 && max(3, 7) == 7 && min(-2, -9) == -9 {
        n = n + 2;
    }
    if sign(-3) == -1 && sign(0) == 0 && sign(8) == 1 {
        n = n + 4;
    }

    // `clamp`, including the crossed-bounds case where `low` wins.
    if clamp(10, 0, 5) == 5 && clamp(-1, 0, 5) == 0 && clamp(3, 0, 5) == 3 {
        if clamp(3, 8, 2) == 8 {
            n = n + 8;
        }
    }

    if pow(2, 10) == 1024 && pow(5, 0) == 1 && pow(3, 3) == 27 {
        n = n + 16;
    }
    if gcd(12, 18) == 6 && gcd(7, 0) == 7 && gcd(0, 0) == 0 {
        n = n + 32;
    }

    // `floor`/`ceil`/`round` on positives — exact whole-number results.
    if floor(3.7) == 3.0 && ceil(3.2) == 4.0 && round(3.5) == 4.0 {
        n = n + 64;
    }

    // The negative case, where truncation and flooring diverge.
    if floor(-1.5) == -2.0 && ceil(-1.5) == -1.0 && round(-2.5) == -3.0 {
        n = n + 128;
    }

    exit(n);
}
```

`floor` is on the *exact* side even though it is a "float function": the line is **exactness, not
difficulty**. It is closed-form — cast toward zero (`cast(s64, x)` truncates), then step down by one when
a negative `x` was cut upward — so `floor(-1.5)` is `-2` while `cast(s64, -1.5)` is `-1`. `sqrt` needs a
loop whose rounding the two engines need not share, so it is on the other side. `abs` and `pow` are
honest about overflow: both use the trapping arithmetic, so `abs` of the most negative `s64` and an
overflowing `pow` **trap** rather than returning a wrong value. The exit code is **255**.

## The transcendentals, as libm wraps

```jr
sqrt :: (x: float64) -> float64 #foreign libm "sqrt";
sin :: (x: float64) -> float64 #foreign libm "sin";
cos :: (x: float64) -> float64 #foreign libm "cos";
acos :: (x: float64) -> float64 #foreign libm "acos";     // used by quat_slerp, below
exp :: (x: float64) -> float64 #foreign libm "exp";
ln :: (x: float64) -> float64 #foreign libm "log";     // named ln, not log: it is the natural log
powf :: (base: float64, exponent: float64) -> float64 #foreign libm "pow";
```

`libm`, not `libc`: every routine here used to be `#foreign libc`, which links `-lc` and happens to work
on macOS, where the math functions live in `libSystem` and `libm.tbd` is a symlink to it. On glibc `libm`
is a **separate** library, so `-lc` alone leaves `sin`, `cos`, `sqrt` and `acos` undefined references and
the link fails — a portability defect this project's macOS-only test machine hid for the module's whole
life, until Linux CI actually ran it (ADR-0205/0206).

```jr
#import "Basic";
#import "Math";

main :: () {
    n := 0;

    if sqrt(16.0) == 4.0 {
        n = n + 1;
    }

    // powf(2.0, 0.5) and sqrt(2.0) are the same libm result.
    if powf(2.0, 0.5) == sqrt(2.0) {
        n = n + 2;
    }

    if powf(2.0, 10.0) == 1024.0 {
        n = n + 4;
    }

    if exp(0.0) == 1.0 && ln(1.0) == 0.0 {
        n = n + 8;
    }

    if sin(0.0) == 0.0 && cos(0.0) == 1.0 {
        n = n + 16;
    }

    // exp(ln(1.0)) == exp(0.0) == 1.0, which is exact.
    if exp(ln(1.0)) == 1.0 {
        n = n + 32;
    }

    // The exact half still works beside the wraps.
    if floor(3.7) == 3.0 {
        n = n + 64;
    }
    if abs(-5) == 5 {
        n = n + 128;
    }

    exit(n);
}
```

The transcendentals were **deliberately absent** in the first `Math`, and the module said so plainly. A
float could not cross the FFI boundary then, so libm was unreachable — and an approximation written in
Jairs would have been wrong in a way this project cannot tolerate: its last bits depend on evaluation
order, and the two engines could round a fused multiply-add differently, so they would disagree on the
last ulp, the one thing the differential harness treats as a failure. Once a float could cross the FFI
boundary (see the *Typed allocation & FFI floats* page), the transcendentals arrived the *right* way:
libm is correctly rounded, and **both engines call the same libm**, so `sqrt(2.0)` is bit-for-bit
identical in the comptime VM and in native code. That is why the comparisons above use exact `==` rather
than a tolerance — the values checked are ones every correctly-rounded libm returns precisely, and both
engines share the library. The exit code is **255**.

## Vectors

`Vector2`, `Vector3` and `Vector4` are plain `float64` structs with `x, y, z, w` components. Arithmetic
comes through **operator overloads** (`+ - * / ==`) and the rest through named procedures.

```jr
Vector3 :: struct { x: float64; y: float64; z: float64; }

vec2 / vec3 / vec4            // constructors
operator + - * / ==          // per width; `*` in both scalar orders (v * s and s * v)
negate2 / negate3 / negate4  // a procedure, not unary operator -, which is out of scope
dot2 / dot3 / dot4
length_squared2/3/4          // exact — no square root
length2/3/4                  // calls libm sqrt, so both engines agree to the last bit
distance2/3/4
normalize2/3/4               // a zero vector normalises to the zero vector, not a NaN or a trap
cross :: (a: Vector3, b: Vector3) -> Vector3   // Vector3 only; right-handed
lerp2/3/4                    // t outside [0,1] extrapolates deliberately
```

```jr
#import "Basic";
#import "Math";

main :: () {
    n := 0;

    a := vec3(1.0, 2.0, 3.0);
    if a.x == 1.0 && a.y == 2.0 && a.z == 3.0 {
        n = n + 1;
    }

    // The imported operator: addition and subtraction on Vector3.
    b := vec3(4.0, 5.0, 6.0);
    if a + b == vec3(5.0, 7.0, 9.0) {
        n = n + 2;
    }
    if b - a == vec3(3.0, 3.0, 3.0) {
        n = n + 4;
    }

    // Scalar multiply in both orders, and division.
    if a * 2.0 == vec3(2.0, 4.0, 6.0) && 2.0 * a == vec3(2.0, 4.0, 6.0) {
        n = n + 8;
    }

    // The right-hand rule: x cross y is z, and y cross x is -z.
    if cross(vec3(1.0, 0.0, 0.0), vec3(0.0, 1.0, 0.0)) == vec3(0.0, 0.0, 1.0) {
        n = n + 128;
    }

    // A 3-4-5 triangle, so the root is exact and `==` is legitimate.
    if length3(vec3(3.0, 4.0, 0.0)) == 5.0 {
        n = n + 512;
    }

    // The degenerate case: no direction, so no unit vector — zero rather than NaN.
    if normalize3(vec3(0.0, 0.0, 0.0)) == vec3(0.0, 0.0, 0.0) {
        n = n + 4096;
    }

    // ... (the full file checks all three widths and every operator; total 262143)
    exit(1);
}
```

(The excerpt above is trimmed; the full corpus file asserts every operator on all three widths and exits
0 only when its running total reaches 262143.)

### Why three concrete types rather than one generic vector

Because a template body cannot use the operators it would need. Resolving `a + b` inside a `$T` body
against the *instantiated* type is operator-bounded polymorphism, which the language does not have (the
same gap `Sort` names). A generic vector would have to spell out `a.x + b.x` per component anyway and
could not offer `+` to its callers, so it would be strictly worse than three concrete types.

Likewise the names carry a dimension — `dot3` rather than `dot` — because Jairs has **no procedure
overloading**: a second `dot` would be a duplicate declaration. Only *operators* resolve by parameter
type, which is why `+` needs no suffix and `dot` does. The element type is `float64` (not Jai's
`float32`), because `sqrt` is a libm double and a `float32` vector would cast at every `length` call.

Design choices worth noting: `normalize` of a **zero** vector returns the zero vector rather than a NaN
or a trap — there is no unit vector in "no direction", and zero keeps the result usable in later
arithmetic where a NaN would silently poison everything. `lerp` **extrapolates** outside `[0, 1]`
deliberately — clamping would hide a caller's mistake. `cross` exists only for `Vector3`, because the
cross product is only a binary operation in three dimensions.

### Matrix4

```jr
Matrix4 :: struct { values: [16]float64; }   // column-major: values[col*4 + row]

mat4_get / mat4_set
mat4_identity / mat4_zero
mat4_translation / mat4_scale / mat4_scale_uniform
mat4_rotation_x / mat4_rotation_y / mat4_rotation_z   // right-handed
mat4_transpose
operator + - == (Matrix4, Matrix4)
operator * (Matrix4, Matrix4)     // composition, non-commutative
operator * (Matrix4, Vector4)     // the workhorse: applies the matrix to a homogeneous vector
operator * (Matrix4, float64) and (float64, Matrix4)
mat4_perspective / mat4_orthographic / mat4_look_at   // right-handed, OpenGL-style z ∈ [-1, 1]
```

```jr
#import "Basic";
#import "Math";

main :: () {
    n := 0;

    // The identity leaves a point unchanged.
    p := vec4(3.0, 4.0, 5.0, 1.0);
    if mat4_identity() * p == p {
        n = n + 1;
    }

    // A translation shifts a point but leaves a direction (w = 0) unchanged.
    t := mat4_translation(10.0, 0.0, 0.0);
    moved := t * p;
    if moved.x == 13.0 && moved.y == 4.0 && moved.z == 5.0 {
        n = n + 2;
    }
    direction := vec4(1.0, 0.0, 0.0, 0.0);
    if t * direction == direction {
        n = n + 4;
    }

    // Composition: scale then translate, applied as one matrix, matches applying them in sequence.
    s := mat4_scale_uniform(2.0);
    combined := t * s;
    stepwise := t * (s * p);
    if combined * p == stepwise {
        n = n + 8;
    }

    // An orthographic projection centred on the origin sends the origin to the centre of clip space.
    ortho := mat4_orthographic(-10.0, 10.0, -10.0, 10.0, 1.0, 100.0);
    origin := vec4(0.0, 0.0, -1.0, 1.0);
    center := ortho * origin;
    if cast(s64, center.x) == 0 && cast(s64, center.y) == 0 {
        n = n + 16;
    }

    // A view from a point on the z axis toward the origin, with y up, puts the eye at the view
    // space's own origin — the defining property of a look-at matrix.
    view := mat4_look_at(vec3(0.0, 0.0, 5.0), vec3(0.0, 0.0, 0.0), vec3(0.0, 1.0, 0.0));
    eye_in_view := view * vec4(0.0, 0.0, 5.0, 1.0);
    if cast(s64, eye_in_view.x) == 0 && cast(s64, eye_in_view.y) == 0 && cast(s64, eye_in_view.z) == 0 {
        n = n + 32;
    }

    exit(n);
}
```

The exit code is **63**. Storage is **column-major** (`values[col*4 + row]`), the layout GLSL and OpenGL
use, so `values[12..15]` is the translation column. The rotations and projection helpers are
**right-handed** to match `Math`'s cross product — choosing left-handed here would make the sign of
`cross` a lie one file over. The `w` component of `Vector4` is what makes a translation reachable:
applied to a point `(x, y, z, 1)` a translation shifts it, and applied to a direction `(x, y, z, 0)` it
leaves the direction unchanged, which the direction check above pins directly.

## Quaternion

`Quaternion` is a 4-tuple encoding a rotation in 3-D: storage is `{x, y, z, w}`, **scalar-last**, matching
`Vector4`'s layout, with `w` the scalar part. Rotation composition is **right-handed**, to match
everything else in the module.

```jr
Quaternion :: struct { x: float64; y: float64; z: float64; w: float64; }

quat :: (x: float64, y: float64, z: float64, w: float64) -> Quaternion
quat_identity :: () -> Quaternion
quat_from_axis_angle :: (axis: Vector3, angle_radians: float64) -> Quaternion   // axis is NOT normalised
operator + - * / == (Quaternion, ...)   // * is the Hamilton product; / and * also take a float64 scalar
quat_conjugate :: (q: Quaternion) -> Quaternion       // the inverse, for a UNIT quaternion
quat_length_squared / quat_length :: (q: Quaternion) -> float64
quat_dot :: (a: Quaternion, b: Quaternion) -> float64
quat_normalize :: (q: Quaternion) -> Quaternion       // a zero quaternion normalises to IDENTITY
quat_inverse :: (q: Quaternion) -> Quaternion         // the identity for a zero quaternion
quat_rotate :: (q: Quaternion, v: Vector3) -> Vector3 // Rodrigues' formula, eight ops not sixteen
quat_to_matrix4 :: (q: Quaternion) -> Matrix4         // assumes q is unit
quat_slerp :: (a: Quaternion, b: Quaternion, t: float64) -> Quaternion
```

`Quaternion` carries **no unit-length invariant**, and multiplication (the Hamilton product, `operator
*`) deliberately does **not** auto-normalise: it would hide a `sqrt` in every composition, and a caller
stacking a hundred rotations does not need to pay for it a hundred times. `quat_slerp` interpolates along
the **shortest** great-circle arc and falls back to normalised *linear* interpolation once the two
quaternions' dot product exceeds `0.9995` — close enough that the great-circle path and the straight-line
path are indistinguishable in a `float64`, and the fallback avoids a division that would otherwise blow
up as the angle between them goes to zero. It answers both endpoints exactly at `t == 0` and `t == 1`.

```jr
#import "Basic";
#import "Math";

main :: () {
    n := 0;

    id := quat_identity();
    if id.x == 0.0 && id.y == 0.0 && id.z == 0.0 && id.w == 1.0 {
        n = n + 1;
    }

    // No PI constant (see below), so libm's own acos gives it: acos(-1.0) is exactly pi.
    pi := acos(-1.0);
    half_turn := quat_from_axis_angle(vec3(0.0, 1.0, 0.0), pi);

    // A 180-degree turn about y sends (1, 0, 0) to (-1, 0, 0).
    rotated := quat_rotate(half_turn, vec3(1.0, 0.0, 0.0));
    if cast(s64, rotated.x * 1000.0) == -1000 {
        n = n + 2;
    }

    // Multiplication does not auto-normalise: scaling a quaternion by 2 quadruples its squared length.
    doubled := half_turn * 2.0;
    if cast(s64, quat_length_squared(doubled) * 100.0) == cast(s64, quat_length_squared(half_turn) * 400.0) {
        n = n + 4;
    }

    // Slerp at the endpoints answers both quaternions exactly.
    a := quat_identity();
    b := quat_from_axis_angle(vec3(0.0, 0.0, 1.0), pi / 2.0);
    if quat_slerp(a, b, 0.0) == a && quat_slerp(a, b, 1.0) == b {
        n = n + 8;
    }

    exit(n);
}
```

The exit code is **15**.

## What Math still doesn't have

Every one of the following is <span class="jairs-status absent">absent</span> today, verified by grep
rather than inferred:

- **No constants.** `Math` declares zero of them — no `PI`, no `TAU`, no `E`, no `EPSILON`. Every angle
  literal must be hand-written or, as above, recovered from `acos(-1.0)`.
- **No `float64` `min`, `max`, `clamp` or `sign`.** All four exist for `s64` only; `fabs` is the *only*
  float counterpart the exact half provides.
- **No scalar `lerp`.** Only `lerp2`/`lerp3`/`lerp4` on vectors — interpolating a single float (a fade, a
  cooldown, a camera zoom) needs open code.
- **No `atan2`, `tan`, `asin`, `atan` or `fmod`.** Only `sin`, `cos` and `acos` are wrapped, so there is no
  "angle of a vector" primitive and no float modulo to wrap an angle into `[0, 2·pi)`.
- **No 2D rotation of a `Vector2`.** No `rotate2`, no `angle2`, no `from_angle` — arguably *the* most
  common 2D operation has no procedure at all; the only route is `mat4_rotation_z` plus a round trip
  through a `Vector4`.
- **No `mat4_inverse`** (and no `mat4_determinant`), so there is no screen-to-world unprojection —
  `mat4_transpose` exists but is an inverse only for a pure rotation.
- **No `operator !=`** on any of the five types — only `==` is overloaded, so `a != b` does not compile
  for `Vector2`/`3`/`4`, `Matrix4` or `Quaternion`.

The *Math for games* page in Book IV has the fuller, games-facing version of this list — component-wise
vector multiply, `Rect`/AABB and collision primitives, easing functions, and the rest of what a 2D game
reaches for and does not find here. See [Math for games](/games/math-for-games/).

See also [Book I — The Jairs Language](/language/introduction/).

---
title: Math
description: Exact closed-form integer and float functions computed in Jairs, libm transcendentals reached by FFI, and vector and matrix types with operator overloads.
sidebar:
  order: 75
---

`Math` has two halves that reach `float` correctness by different routes, and that split is the
interesting thing about the module. The exact, closed-form functions are computed **in Jairs**, so both
engines produce identical bits. The transcendentals — `sqrt`, `sin`, `cos`, `exp`, `ln`, `powf` — are
**`#foreign` wraps of libm**, not Jairs approximations. It also carries `Vector2/3/4` and `Matrix4`.

There are **no quaternions**.

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
sqrt :: (x: float64) -> float64 #foreign libc "sqrt";
sin :: (x: float64) -> float64 #foreign libc "sin";
cos :: (x: float64) -> float64 #foreign libc "cos";
exp :: (x: float64) -> float64 #foreign libc "exp";
ln :: (x: float64) -> float64 #foreign libc "log";     // named ln, not log: it is the natural log
powf :: (base: float64, exponent: float64) -> float64 #foreign libc "pow";
```

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

## Vectors and Matrix4

`Vector2`, `Vector3` and `Vector4` are plain `float64` structs with `x, y, z, w` components. Arithmetic
comes through **operator overloads** (`+ - * / ==`) and the rest through named procedures. `Matrix4` is a
4×4 `float64` matrix. Quaternions do **not** exist.

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

Storage is **column-major** (`values[col*4 + row]`), the layout GLSL and OpenGL use, so `values[12..15]`
is the translation column. The rotations and projection helpers are **right-handed** to match `Math`'s
cross product — choosing left-handed here would make the sign of `cross` a lie one file over. The `w`
component of `Vector4` is what makes a translation reachable: applied to a point `(x, y, z, 1)` a
translation shifts it, and applied to a direction `(x, y, z, 0)` it leaves the direction unchanged.

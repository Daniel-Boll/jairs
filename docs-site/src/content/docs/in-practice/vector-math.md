---
title: 3D vector math
description: Compute a triangle's normal and how strongly it faces a light, using Math's vectors.
sidebar:
  order: 5
---

A small piece of 3D geometry: given a triangle, find the unit vector perpendicular to its
face (its *normal*), and measure how directly it faces a light. It uses
[`Math`](/language/the-standard-library/#math)'s `Vector3` type, and it shows operator
overloads crossing a module boundary.

```jr
#import "Basic";
#import "Math";

// Compute the unit normal of a triangle and how strongly it faces a light.
main :: () {
    a := vec3(0.0, 0.0, 0.0);
    b := vec3(1.0, 0.0, 0.0);
    c := vec3(0.0, 1.0, 0.0);

    // Two edges of the triangle; their cross product is perpendicular to the face.
    edge1 := b - a;            // operator - crosses the module boundary
    edge2 := c - a;
    normal := normalize3(cross(edge1, edge2));

    // A light shining straight down the +z axis.
    light := vec3(0.0, 0.0, 1.0);
    facing := dot3(normal, light);

    // There is no float printing yet, so scale into an integer to show the value.
    print("normal.z x1000 = ");
    print_int(cast(s64, normal.z * 1000.0));
    print("\nfacing   x1000 = ");
    print_int(cast(s64, facing * 1000.0));
    print("\n");

    if facing > 0.0 {
        print("the face points toward the light\n");
    }
}
```

Output:

```
normal.z x1000 = 1000
facing   x1000 = 1000
the face points toward the light
```

The triangle lies in the *xy*-plane, so its normal is exactly `(0, 0, 1)` — hence `normal.z`
is 1.0 — and it faces a *+z* light head-on, so the facing factor is 1.0.

## How it works

**Constructing vectors.** `vec3(x, y, z)` builds a `Vector3`, a struct of three `float64`
components. The three corners `a`, `b`, `c` define the triangle.

**Operators that cross the module boundary.** `b - a` uses `Math`'s `operator -` on
`Vector3`, even though the overload is defined in the `Math` module and used here in ours.
That is the point worth noticing: [operator overloads cross an
import](/language/operators-and-overloading/#overloading-an-operator-for-your-own-type), so a
vector type from a library behaves like a built-in in your code. `cross` and `dot3` are
ordinary procedures from the same module.

**The geometry.** The cross product of two edge vectors is perpendicular to the face;
`normalize3` scales it to unit length. The dot product of that unit normal with a unit light
direction is the cosine of the angle between them — 1.0 when they align, as they do here.

**Working around no float printing.** Jairs has no float-printing routine yet (see
[What's absent](/language/whats-absent/)), so to *show* a float we multiply by 1000 and
`cast(s64, …)` — a float-to-int cast that
[saturates](/language/the-type-system/#cast), giving a clean integer to `print_int`. The
`if facing > 0.0` branch shows a float comparison driving control flow directly.

## A note on exactness

The numbers here are exact — `0.0`, `1.0` and their products are all exactly representable in
binary floating point, and `normalize3` of `(0, 0, 1)` is `(0, 0, 1)` with no rounding. When a
computation reaches `Math`'s `sqrt` (as `length3` does), the result is still bit-identical
between the engines, because both call the same correctly-rounded libm through the
[FFI](/language/modules/#the-foreign-function-interface). That is why `Math` wraps libm rather
than approximating transcendentals in Jairs — an in-language approximation's last bit could
differ between the two engines.

## What it demonstrates

- `Math`'s `Vector3` with `-`, `cross`, `dot3`, `normalize3`.
- Operator overloads resolving across a module import.
- A saturating float-to-int `cast` as a stand-in for float printing.

Next: [a generated task runner](/in-practice/note-serialiser/).

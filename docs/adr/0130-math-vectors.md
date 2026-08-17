# ADR-0130: `Math` vectors — `Vector2`, `Vector3`, `Vector4` with cross-module operators

- **Status:** Accepted
- **Date:** 2026-08-12
- **Deciders:** dboll
- **Wave 3 of eight, sub-wave 3a.** Waves 1 and 2 were ADR-0128 and ADR-0129. This is the third of
  ADR-0127 §3's **six unkept promises** — `Math` vec/mat/quat — delivered in the part that needs no
  language change. **Matrices (3b) and quaternions (3c) are still owed**, and §5 says why they are
  separate rather than late.
- No design fork was put to the decider. §1 and §2 record two choices that *look* like forks and are
  not: each has one answer the existing code forces, and both are stated because a reader will otherwise
  assume a preference was exercised.

## Context

### The promise, and why it was worth probing before writing

ADR-0115 declared `Math` **complete**. It shipped `sqrt`, `sin`, `cos`, `exp`, `ln`, `powf` as libm wraps
and the exact half in Jairs, and `PLAN.md`'s W7 row lists `Math (vec/mat/quat)`. The vectors were absent.
ADR-0127 §3 recorded the contradiction, noting that the wave "declared `Math` *complete* without them".

The whole design rests on one fact that was **not** safe to assume: **does an operator overload cross the
module boundary?** A `Vector3` whose `+` worked only inside `modules/Math/module.jr` would be worthless —
every caller would write `add3(a, b)` and the overloads would be decoration. So it was probed first, with a
two-file throwaway module, before a line of the real thing was written. It works: `a + b` in a file that
`#import "Math"` resolves to `Math`'s overload, for `+ - * /` and `==`, and with `*` in both operand
orders.

That is the same discipline ADR-0070 §0 and ADR-0067 §0 record: a plan's stated dependency is checkable,
and checking it is cheap. Here it was the *undocumented* dependency that needed the check.

### Three language limits shape the API, and none of them is negotiable here

- **No procedure overloading.** A second `dot` is E0200, duplicate declaration. Only operators resolve by
  parameter type, because they intern as the synthetic symbol `operator+` with type-based lookup
  (ADR-0048).
- **No unary operator overload.** `operator -` with one parameter collides with the binary form on arity
  and is out of scope (`type-errors/048`).
- **Operators do not resolve inside a `$T` template body.** `modules/Sort` §18 already names this:
  resolving `a + b` against the *instantiated* type is operator-bounded polymorphism, a real feature
  belonging to a later wave.

## Decision

### 1. The element type is `float64`, because `sqrt` is

Jai's `Vector3` is `float32`, for SIMD packing and GPU interchange. **Neither exists in Jairs** — SIMD is
W8, graphics W10 — and `sqrt` is `#foreign libc "sqrt"`, which takes a **double**. `length` therefore needs
a `float64` at the FFI boundary, so a `float32` vector would cast in and back out at every call that
matters. Every other function in the module is already `float64`.

The element type follows **the module it lives in**, not the language it is modelled on. A packed
`float32` vector belongs with the wave that has a reason for the packing.

**Rejected: both widths.** Without procedure overloading that doubles every name — `dot3` and `dot3f` —
for a benefit no current consumer can measure.

### 2. Names carry a dimension, and that is a language gap showing through

`dot3`, `length3`, `normalize3`, `lerp3`, `negate3`. This is **not** a naming preference: a second `dot` is
a duplicate declaration. Only `cross` escapes, because the cross product is binary in three dimensions
only, so there is nothing to disambiguate it from.

Stated plainly in the module's own docs, because a reader who does not know Jairs lacks overloading will
read the suffixes as clumsiness. When overloading arrives they collapse into one `dot`, and **the operators
do not change at all** — which is the argument for putting `+ - * / ==` on operators and everything else on
named procedures, rather than a uniform `add3`/`sub3` API that would have aged worse.

**Rejected: a generic `Vector($T, $N)`.** It cannot use the operators it needs (limit 3 above), so its body
would spell out `a.x + b.x` per component exactly as three concrete types do — while also being unable to
*offer* `+` to callers. Strictly worse, and the reason is a language limit rather than a size judgement.

### 3. `normalize` of a zero vector answers the zero vector

There is no unit vector in "no direction", so every implementation chooses. The alternatives are a NaN,
which **silently poisons** every later component and every comparison, or a trap, which makes a degenerate
input unsurvivable in code that is otherwise correct. Zero keeps the result usable, and a caller who needs
to distinguish tests `length_squared3` first — one call, no root.

Pinned by `valid/103`, because an unpinned choice in a degenerate case is exactly what drifts. The
teeth-check confirms it: removing the guard fails the corpus file.

### 4. The corpus file asserts on **equality**, and says where that is subtle

`valid/103` uses 3-4-5 triangles, halves and small integers, so `==` is legitimate rather than lucky. A
tolerance would weaken the differential harness, whose premise is that the two engines agree bit for bit.

**One assertion needed its reasoning corrected before it could be trusted.** The file first claimed every
value was "exactly representable in binary floating point", which is **false** of `normalize3`'s expected
`0.6` and `0.8` — neither has a finite binary expansion. The equality holds for a different reason:
IEEE-754 division is *correctly rounded*, so `3.0 / 5.0` yields the double nearest 0.6, which is precisely
what the literal `0.6` parses to. Both sides land on the same double; neither is exact. The comment now
says that, because a test whose stated justification is wrong is a test nobody can safely change.

The file also pins the **right-hand rule** — `x cross y = z` and `y cross x = -z`. A sign error in any of
`cross`'s six terms passes every length and dot assertion, which is why the orientation is checked directly
rather than inferred; the teeth-check flips one term and the file fails.

### 5. Matrices and quaternions are sub-waves 3b and 3c

Not descoped — **sequenced**. W4 shipped in ten sub-waves and W5 in fifteen, for the reason ADR-0069 §0
gives: a unit five times the size of the others cannot be verified the way the others were. `Matrix4` needs
its own decisions that vectors do not force — row-major or column-major storage, whether `operator *`
carries matrix-times-vector as well as matrix-times-matrix, and whether the projection helpers assume a
handedness. Those are real forks, and they belong in front of the decider rather than inside a wave whose
mandate was "the part that needs no language change".

`PLAN.md` §2.1 therefore still marks `Math` vec/mat/quat as **partly** delivered, rather than closing the
row. Closing it now would repeat exactly what ADR-0115 did.

## Consequences

- `Math` gains three types, 21 operator overloads and 25 procedures, all written in Jairs. No compiler
  change, which is what made this the right wave to take after two that were compiler work.
- **A library's operators are now known to cross the module boundary**, and a corpus file pins it. Every
  future library type that wants arithmetic — a `Matrix4`, a `Quaternion`, a `Complex`, a big integer —
  can rely on it rather than re-probing.
- **1010 tests, unchanged; 216 → 217 corpus files.** `valid/103` is iterated by the differential and
  snapshot harnesses rather than adding a test case, the third wave running for which the corpus count is
  the only number that moves.
- The dimension suffixes are a **standing, visible reminder** that procedure overloading is absent. That is
  a feature of the naming: the gap is documented in the place a user meets it.

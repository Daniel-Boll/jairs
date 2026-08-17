# ADR-0131: `Math` `Matrix4` — column-major storage, right-handed conventions

- **Status:** Accepted
- **Date:** 2026-08-17
- **Deciders:** dboll
- **Wave 3 of eight, sub-wave 3b.** ADR-0128 was wave 1, ADR-0129 wave 2, ADR-0130 sub-wave 3a
  (vectors). This is the fourth of ADR-0127 §3's **six unkept promises** — the matrix half of `Math`'s
  `vec/mat/quat` — delivered in the part that still needs no language change. **Quaternions (3c) are
  still owed**, and want a matrix to convert to and from, which is why the sub-waves are ordered
  this way rather than in parallel.
- **Three forks were owed to the decider** (per PLAN §7's Wave 3b row): storage order, whether
  `operator *` carries matrix×vector, and the handedness of the projection helpers. §1–§3 record the
  choice, the rejected alternative and why the rejection was the wrong answer here — none was ever
  put to the decider by the wave, on the goal's own terms ("take the recommended approach; save the
  alternatives and why").

## Context

### The premise, and what the previous wave settled

ADR-0130 shipped `Vector2`/`3`/`4` with cross-module operator overloads and asserted the fact the
whole library-typing effort rests on: **an operator overload defined in a module resolves in a file
that imports it**. It was probed before a line was written. That probe covered `+ - * /` and `==`
over three types.

Matrix4 gains nothing new from the check — a `Matrix4 * Vector4` is a resolution against types
declared in the same module — so this wave writes the code directly rather than re-probing what one
sub-wave over already pinned.

### Three language limits still shape the API, unchanged from ADR-0130

- **No procedure overloading**, so a helper's name carries an arity or a purpose (`mat4_translation`,
  `mat4_rotation_x`) rather than resolving from the argument list.
- **No unary operator overload**, so a "negate" that logically wants to be `-m` is out of scope
  (`type-errors/048`). No `negate` for Matrix4 ships, because the callers who need it — a shear
  reflected, a light direction flipped — do not exist yet, and a procedure they will not use is a
  price paid in reader attention for nothing (ADR-0058 §3's argument, one type wider).
- **Operators do not resolve inside a `$T` template body**, so a generic `Matrix($T, $N)` would
  have the same problem `Vector($T, $N)` did (ADR-0130 §2 rejected), and for the same reason: it
  could not use the operators it needs.

## Decision

### 1. Storage is **column-major** — `values[col*4 + row]`

Every 4×4 matrix in the module stores its 16 elements column-first: the four `float64` at
`values[c*4 ..= c*4+3]` are column `c`. The translation column is therefore `values[12..15]`, the
layout GLSL and OpenGL use, and the layout the mathematical convention

    (M · v)ᵣ = Σₖ Mᵣₖ · vₖ

reads directly: `v` is a column of coordinates, `M` sits on the left, composition `A · B` applied
to `v` is `A · (B · v)` — right-to-left, the way `mat4_translation(x, y, z) * mat4_scale_uniform(2)`
reads *when you speak it out loud* ("translate, then scale, applied to v" — scale first, then
translate).

**Rejected: row-major.** The C-array-in-memory reading is more familiar and the row order
`{a, b, c, tx}` puts the translation at the *end* of each row, which some people find easier to
read in a debugger. Two reasons that lose here. First, when a graphics wave arrives (`W10`) it will
target GLSL/SPIR-V, both column-major, and a per-frame transpose on every uniform upload is a real
cost for a decision that could have been made once. Second, `A · B` in row-major means the vector
is a **row** on the left — `v · M` — and no other convention in Jairs points at that direction: the
Vector types are conceptually columns already (their component names read as coordinates, not as a
one-row layout).

**Rejected: expose the storage as a raw `[16]float64` typedef rather than a `Matrix4` struct.**
Every access would then bake `col*4 + row` into every caller's code, and changing the layout later
would be a workspace rewrite. `mat4_get`/`mat4_set` exist for exactly that: the layout is one
module's fact, not every caller's.

### 2. `operator *` carries **all four** meaningful multiplications

`Matrix4 * Matrix4` (composition), `Matrix4 * Vector4` (transformation), `Matrix4 * float64`
(scalar scaling), and `float64 * Matrix4` (its mirror, since the operators do not commute their own
operand order — one wired without the other is invisible, as ADR-0130 §b noted). Composition and
transformation are the workhorses; the scalar forms exist for symmetry with the vector operators
and because a per-frame `M * dt` is an ordinary thing to write.

**Rejected: only `Matrix4 * Matrix4`**, with a named `apply(m, v)` for transformation. It reads
worse (`apply(mat4_translation(1, 0, 0), apply(mat4_rotation_z(a), v))` versus
`mat4_translation(1, 0, 0) * mat4_rotation_z(a) * v`), and the whole reason the module has operator
overloads is that `apply(add3(a, mul3(b, 2.0)), …)` was already refused for vectors.

**Rejected: `Vector4 * Matrix4` too.** In column-major storage that would be a *row-vector times
matrix* product, which the design just rejected in §1. Providing it would give two syntaxes for the
same operation whose meaning depends on which one you wrote, and the whole point of a convention is
that the meaning is a property of the type, not the spelling.

### 3. Projections and `mat4_look_at` are **right-handed**

`mat4_perspective` and `mat4_orthographic` produce matrices for a right-handed camera looking down
its own **−z** axis, with clip-space depth mapped to `[-1, 1]` — the OpenGL convention.
`mat4_look_at` uses the same convention: the view frame's `+z` is *out of the screen*, so
`eye - center` is the z basis (not `center - eye`, which is the left-handed choice).

**Rejected: left-handed.** DirectX/Unity is left-handed, and Unreal is left-handed with a
different-axis-up on top of that, so "graphics uses left-handed" is not a fact either. What is a
fact is that `cross` in ADR-0130 §4 was pinned right-handed — `x cross y = z`, with `valid/103`
teeth-checked by flipping a term. A left-handed projection would then live next door to a
right-handed cross product, and the sign of a physics gradient across the module boundary would be
a lie waiting for someone to hit. **The invariant this module already exposes forces the
choice**, so §3 is a fork with one answer rather than a real preference.

**Rejected: reverse-z / infinite-far depth mapping.** These are useful (deeper precision near the
camera, no explicit `far`) and there is real support for taking one of them as the default. They
are also each a separate rejected convention for `[-1, 1]` depth, and the graphics wave that has a
target platform will make the choice with information this wave does not have. Shipping the
OpenGL-classic mapping keeps a caller who reads a textbook and types the formula in able to check
the result; shipping reverse-z here would make the module's projection *look* like the textbook
formula and produce different numbers.

### 4. Rotation is **three axis-aligned procedures**, not a general axis-angle constructor

`mat4_rotation_x`, `mat4_rotation_y`, `mat4_rotation_z`. A general `mat4_rotation_axis(axis, angle)`
is *more* useful but the code that writes it well is a quaternion converted to a matrix — that is
literally sub-wave 3c, and doing it here would either duplicate the derivation or ship an
inefficient Rodrigues-formula version that the quaternion wave would then replace. The three
axis-aligned rotations cover every case where the axis is a coordinate axis, which is most of them,
and compose with `*` for the rest until 3c arrives.

**Rejected: a Rodrigues-formula `mat4_rotation_axis`**, for the reason above. Owed to sub-wave 3c
via the quaternion.

### 5. **No `mat4_inverse` and no `mat4_determinant`**, deliberately

An `inverse` of a general 4×4 matrix is a 90-line cofactor expansion; a `determinant` is 40 lines.
Both are mechanical to write and were held out anyway, and the reason is not code volume: the
matrices this module *constructs* (identity, translation, scale, rotation, look_at, orthographic)
all have **closed-form** inverses that are cheaper and more numerically stable than the general
cofactor formula. A caller who wants "the inverse of my view matrix" is better served by writing

    inv_view := mat4_translation(-eye.x, -eye.y, -eye.z) * mat4_look_at_rotation_transpose(...)

or, more usually, by keeping the inverse as they build up the forward. Shipping a general `inverse`
would make the wrong idiom convenient — a caller inverting a translation with an 90-line cofactor
sum when a sign flip on three floats does it — and refusing wants an ADR.

**Deferred, not declined.** A general `mat4_inverse` shows up the day a caller writes a matrix
whose construction Jairs does not provide (a shear, a projection composed from primitives the
library did not offer). It has one design decision — whether it returns a `Matrix4` for a singular
input or reports a failure — that this wave has no reason to take.

### 6. The corpus file, once again, asserts on **equality**, with two exceptions stated

`valid/104` sets up matrices whose entries are exact `float64` — 0, 1, 2, half-integer rotations by
0, π/2, π/π (all with `sin`/`cos` values in `{-1, 0, 1}`, exactly represented) — and asserts
equality against the expected `Vector4`. That reuses ADR-0130 §4's premise that the differential
harness's whole point is bit-for-bit agreement, and a tolerance would weaken it.

Two exceptions carry a *different* justification than the vector file's, and one caught a wrong
expectation before it shipped. `mat4_perspective(π/2, 1, 1, 100)` was expected to yield `f = 1.0`
(since `cot(π/4) = 1` mathematically), but the corpus file **fails** if it asserts that: libm's
`cos(π/4)` rounds to `0.7071067811865476` and `sin(π/4)` to `0.7071067811865475` — one ulp apart —
and their ratio is `1.0000000000000002`, not `1.0`. Both engines agree on *that* value, which is
what the differential harness pins; the mathematical identity does not survive being computed in
finite precision, and expecting it to would be a corpus file lying about what the code does. The
file instead asserts `p.values[0] == p.values[5]` — the aspect symmetry, which is a structural
property of the formula rather than a bet against rounding.

Second, `mat4_rotation_z(π)` is expected to yield `-1` on the diagonal and 0 off, and `sin(π)` is
not zero in `float64` — it is `1.2246e-16`. The file therefore checks
`mat4_rotation_x/y/z(0.0) == mat4_identity()`, since `sin(0) = 0` and `cos(0) = 1` are both exact
in doubles — the identity case is the one arithmetic can defend.

`mat4_look_at` is checked by sending the origin through the returned matrix and asserting the eye
lands at (0, 0, 0) — the property a view matrix has by definition. That check catches an axis-flip
in any of the three basis rows, which no scalar check would.

## Consequences

- `Math` gains one type, 6 operator overloads and 12 procedures. No compiler change, so all six
  gates run against the pre-Matrix4 machinery.
- **Column-major storage is now a documented module convention**, and the graphics wave (W10) can
  build on it rather than choosing again. `mat4_get`/`mat4_set` hide the fact from every caller.
- The **right-handed convention** is now pinned by two operators (`cross` in ADR-0130, projections
  and `look_at` here). A future wave choosing left-handed graphics ships a *different module*, not
  a switched convention here.
- **Test count and corpus count**: the corpus grows by one file (`valid/104-math-matrix4.jr`),
  iterated by the differential and snapshot harnesses rather than adding a Rust test, so the
  workspace test count is unchanged. The pattern from ADR-0130 recurs — an all-library wave moves
  only the corpus count.
- Two things are **deferred rather than declined**: `mat4_inverse`, when there is a caller for a
  matrix the library did not construct; and `mat4_rotation_axis`, which sub-wave 3c will derive
  from the quaternion rather than duplicate here.

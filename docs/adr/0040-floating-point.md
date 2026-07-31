# ADR-0040: `float32` and `float64` are plain IEEE-754, and `Convert` learns a `NumKind`

- **Status:** Accepted
- **Date:** 2026-07-28
- **Deciders:** dboll
- **Amends:** nothing. ADR-0002 is *scoped* by §1 rather than overturned — it was always
  about integer overflow, and this ADR states the boundary it never had to state.

## Context

`PLAN.md` §2.1 puts `float32`/`float64` in W1, and ADR-0037 §1 singled them out as "the one
part of the tower `IntKind` cannot absorb": a new value representation in the pool, the VM
and Cranelift, plus literal parsing the parser refuses with E0120.

Four facts were established by reading the code before this ADR was written.

- **The lexer already produces `FLOAT_LITERAL`, and does it carefully.** `1.5`, `1e9`,
  `1.5e-3` all lex. A `.` begins a fractional part *only* when a digit follows, so `1..2`
  lexes as `1 .. 2` and `x.*` is unaffected — the two ways float lexing usually breaks a
  language that has ranges and a postfix deref. Nothing here needs changing.
- **The parser refuses a float literal with E0120**, whose message says "arrives in wave W1"
  — which stays true right up to the moment this wave lands and then becomes a lie. It is
  removed, not reworded.
- **`Value::Scalar(u64)` can carry float bits.** The VM's value model needs **no new
  variant**: a `float64` is its eight bytes and a `float32` is its four, exactly as an
  integer is. What the VM needs is a way to know *which* interpretation applies, and that is
  a property of the type, which it already has.
- **`IntKind::of` returning `None` is the current "not an integer" signal**, and the VM's
  `binary` uses it to fall back to a raw bit compare for `bool` and pointer equality. A
  float reaching that fallback would compare bits — which is wrong for `-0.0 == 0.0` (true
  in IEEE-754, different bits) and for `NaN == NaN` (false in IEEE-754, identical bits).
  This is the one place where doing nothing produces a *plausible wrong answer* rather than
  an error, so it is where the care goes.

## Decision

### 1. Plain IEEE-754. No traps, and ADR-0002 does not apply

```jr
1.0 / 0.0     // inf
0.0 / 0.0     // NaN
big * big     // inf — saturates
NaN == NaN    // false
```

ADR-0002 makes integer `+`, `-`, `*` trap on overflow. That decision does **not** extend
here, and the reason is the one ADR-0002 is actually about: an overflowing integer addition
produces a result the program did not ask for — the true sum is not representable and the
wrapped value is a different number. IEEE-754 *defines* `inf` as the answer to an overflowing
float multiply and `NaN` as the answer to `0.0/0.0`. They are values, not failures.

So there is no trap, no check, and no branch on any float operation. This is what Jai, C,
Zig and Rust all do, and it is what the hardware does with no extra instructions.

**Rejected: trap on float division by zero.** Superficially consistent with integer `/`,
which does trap on a zero divisor. Rejected because the integer case traps for a different
reason — there is no integer `inf`, so `1/0` has no representable answer at all — while
`1.0/0.0` has one and numerical code sometimes wants it. It would also cost a
compare-and-branch on every float division.

**Rejected: trap on NaN production.** The most debuggable option, and the one no systems
language takes: it needs a check after *every* arithmetic operation, and NaN is a legitimate
sentinel in real numerical code.

**The consequence, stated because it surprises people:** `==` is no longer reflexive.
`x == x` is `false` when `x` is `NaN`. Jairs has no `is_nan` yet — it arrives with W7's
`Math` — so a program that needs the check today writes `x != x`.

### 2. `FloatKind`, beside `IntKind`, in `jr-pool`

```rust
pub struct FloatKind { pub bits: u16 }   // 32 or 64
```

`Item::FloatType { bits }`, interned structurally like `IntType`. `float32` and `float64`
resolve through `FloatKind::from_name`, which is `IntKind::from_name`'s counterpart, so
`jr-sema`'s type resolution and `jr-lsp`'s completion list read one list of names as
ADR-0037 §1 established.

Layout is `bits/8`, aligned to itself, from `layout_of` — the one place layout is computed
(ADR-0018 §2).

**No pre-interned `PoolId` constant**, for exactly ADR-0037 §1's reason: the well-known
prefix is for types reached *before* user code, and no float is. Interning is structural, so
`Item::FloatType { bits: 64 }` becomes a `PoolId` the first time anything asks.

**Float arithmetic lives in `jr-pool` beside the integer arithmetic**, as `float_binary`,
`float_compare` and `float_negate`. This is ADR-0022 §2's rule and it matters more here, not
less: a fold happens at compile time and bakes its answer into a `PoolId` that **both**
engines then consume, so a disagreement with the interpreter shows up as two engines
agreeing on the wrong constant — which `differential.rs` cannot see.

### 3. `Rvalue::Convert` carries a `NumKind`, not an `IntKind`

```rust
pub enum NumKind {
    Int(IntKind),
    Float(FloatKind),
}
```

`cast` now has four directions: int→int, int→float, float→int, float→float. The
*destination* still comes from the `ValueId` the rvalue defines, so one field determines the
direction and every existing construction site keeps its shape.

This preserves the check ADR-0037 added and which is the reason `Convert` records its source
at all: the verifier asserts the recorded `from` matches the operand's actual type. That
check is what catches a sign-extend where a zero-extend belonged, and it now also catches an
`fcvt` where an `sextend` belonged — a wrong number with no diagnostic anywhere.

**Rejected: a separate `Rvalue::FloatConvert`.** No existing site changes. Rejected because
it splits one concept across two variants every pass must handle identically, and a cast
from an int to a float belongs to neither cleanly — it would need a third.

**Rejected: record both `from` and `to`.** Self-describing in a dump, and `to` would
duplicate the `ValueId`'s type. Two facts that must agree can disagree, which is precisely
what the verifier exists to catch for `from` alone.

### 4. `float32`→`float64` widens exactly; `float64`→`float32` rounds; float→int truncates

- **Widening** `float32`→`float64` is exact, always.
- **Narrowing** `float64`→`float32` rounds to nearest, and saturates to `inf` when the value
  is too large. IEEE-754's own rule.
- **float→int truncates toward zero**, and a value outside the destination's range is
  **saturating**, not wrapping: `cast(s8, 1000.0)` is 127.

That last one is a real decision. C makes an out-of-range float→int conversion *undefined
behaviour*, and Cranelift has both `fcvt_to_sint` (which traps) and `fcvt_to_sint_sat`
(which saturates). Saturation is chosen because it is total: every float has an answer in
every integer type, so there is no third behaviour to define and no trap to add to a path
§1 just made trap-free. Rust made the same change for the same reason. `NaN` converts to 0,
which is what `fcvt_to_sint_sat` produces and what Rust specifies.

**int→float rounds to nearest** where the integer is not exactly representable, which for
`float64` means integers above 2^53. Unavoidable and standard.

### 5. A float literal takes its type from context, and defaults to `float64`

ADR-0016 §1 makes an integer literal take its type from its context. A float literal does
the same: `x: float32 = 1.5;` gives `1.5` the type `float32`. With no context — `y := 1.5;`
— it defaults to **`float64`**, matching every language that has both and matching the
integer literal's default to `s64`.

`Literal::Float { value: f64, .. }` holds the parsed value as an `f64` regardless of the
eventual type, for the same reason ADR-0038 §2 made the integer literal an `i128`: the widest
representation is the one that cannot lose information before the type is known. A
`float32` context narrows it at interning time.

**A float literal that does not fit `float32` is not an error.** `x: float32 = 1e300;` gives
`inf`, because §1 says overflow saturates and a literal is not a special case. This differs
from the integer rule — `x: u8 = 300;` *is* E0204 — and the difference is §1's: there is no
integer `inf` to saturate to, so an integer literal that does not fit has no answer, while a
float literal always has one.

### 6. No implicit conversion between an integer and a float, in either direction

`1 + 1.5` is a type error, and so is `some_s64 + some_float64`. `cast` is the only way
across, exactly as it is the only way between integer widths (ADR-0037 §2).

This is stricter than C and it is the same strictness Jairs already has: ADR-0016's typing
rules have no implicit numeric conversions at all, and adding one for floats would make the
float the *only* type that silently changes another's meaning.

The exception is the one that already exists and is not a conversion: an untyped *literal*
takes its context's type. `1.5 + x` where `x` is a `float32` makes the literal a `float32`;
`1 + x` where `x` is a `float64` is an error, because `1` is an integer literal and §5's
context typing gives it the *integer* interpretation. That asymmetry is deliberate — `1` and
`1.0` are different literals — and `1.0 + x` is what the programmer meant.

### 7. `%` is not defined on floats in this wave

`a % b` on floats is refused with a type error. C's `fmod` truncates toward zero, Python's
`%` follows the sign of the divisor, and the two disagree on `-1.0 % 3.0`. That is a language
decision with no forcing constraint yet, and nothing in the corpus wants it. The refusal names
the reason rather than saying "not supported".

Likewise **no math intrinsics**: `sqrt`, `sin`, `cos` belong to W7's `Math` module written in
Jairs over `#foreign` to libm, which is where `PLAN.md` §2.1 puts them.

## Consequences

- **The VM gains no `Value` variant**, and that is load-bearing rather than lucky: a float is
  its bits, and which interpretation applies comes from the type the VM already has. What it
  gains is a dispatch on `FloatKind::of` beside the existing `IntKind::of`.
- **The `IntKind::of` fallback in the VM's `binary` becomes a real hazard and is closed.**
  It currently answers a raw bit compare for `==` on anything non-integer. A float reaching
  it would get `-0.0 == 0.0` wrong (true in IEEE-754, different bits) and `NaN == NaN` wrong
  (false in IEEE-754, identical bits). Both are *plausible wrong answers* rather than errors,
  which is this project's named failure mode, so the float case is dispatched **before** that
  fallback and a test pins both values.
- **E0120 is deleted, not reworded.** Its message says floats "arrive in wave W1"; leaving
  it would be the plan contradicting the code, which is the other named failure mode.
- **`differential.rs` gains float cases**, and they earn their place: float arithmetic is
  the one thing in this wave where the VM's software evaluation and Cranelift's hardware
  instructions are genuinely different implementations of the same specification. An integer
  add is exact in both by construction; a float multiply is only equal because IEEE-754 says
  so, and the harness is what checks that both actually obey it.
- **`f64` cannot be a pool key by derived `Hash`/`Eq`.** `Item` derives both, and `f64` has
  neither — `NaN != NaN` breaks `Eq`, and `0.0 == -0.0` with different bits breaks the
  `Hash`/`Eq` contract. `Item::FloatValue` therefore stores the **bits** as a `u64`, so two
  float values intern to one `PoolId` exactly when their bit patterns match. The consequence
  is that `0.0` and `-0.0` are distinct pool entries, which is correct: they are
  distinguishable values, and `1.0/0.0` versus `1.0/-0.0` proves it.

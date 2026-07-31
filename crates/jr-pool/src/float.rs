//! IEEE-754 arithmetic, shared by both evaluators.
//!
//! [ADR-0040](../../../docs/adr/0040-floating-point.md) is this module's specification, and
//! it exists here for [ADR-0022](../../../docs/adr/0022-dce-constprop-shared-arithmetic.md)
//! §2's reason rather than for tidiness: a constant fold happens at *compile* time and
//! bakes its answer into a [`PoolId`](crate::PoolId) that **both** engines then consume, so
//! a disagreement between the folder and the interpreter does not show up as two engines
//! disagreeing — it shows up as two engines agreeing on the wrong number, which
//! `differential.rs` cannot see.
//!
//! # Why there is no `FloatTrap`
//!
//! Because there is nothing to trap on. ADR-0002 makes integer `+`, `-`, `*` trap on
//! overflow, and ADR-0040 §1 scopes that decision to integers: an overflowing integer
//! addition produces a result the program did not ask for, while IEEE-754 *defines* `inf`
//! as the answer to an overflowing float multiply and `NaN` as the answer to `0.0/0.0`.
//! Those are values, not failures.
//!
//! So every function here is total. [`float_binary`] returns a `u64` rather than a
//! `Result`, which is the visible difference from [`int_binary`](crate::int_binary) and the
//! one place this module's whole argument is legible in a signature.
//!
//! # Why bits rather than `f64` at the boundary
//!
//! `Item` derives `Hash` and `Eq`, and `f64` has neither: `NaN != NaN` breaks `Eq`, and
//! `0.0 == -0.0` with different bit patterns breaks the `Hash`/`Eq` contract. Every value
//! crossing this module's boundary is therefore a bit pattern, decoded on the way in and
//! re-encoded on the way out — the same discipline `IntKind` already uses, for a different
//! reason.

use crate::item::{Item, PoolId};
use crate::pool::Pool;

// ---------------------------------------------------------------------------
// FloatKind
// ---------------------------------------------------------------------------

/// The width of a floating-point type.
///
/// One field rather than two, unlike [`IntKind`](crate::IntKind): there is no signedness to
/// record, because IEEE-754 has one signed representation and no unsigned counterpart.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FloatKind {
    /// The width in bits: 32 or 64.
    pub bits: u16,
}

impl FloatKind {
    /// `float64`, the type an untyped float literal defaults to (ADR-0040 §5).
    pub const F64: Self = Self { bits: 64 };
    /// `float32`.
    pub const F32: Self = Self { bits: 32 };

    /// The float kind a builtin type *name* denotes, if it denotes one.
    ///
    /// **The one list of float type names in the project**, for the same reason
    /// [`IntKind::from_name`](crate::IntKind::from_name) is the one list of integer names
    /// (ADR-0037 §1): `jr-sema`'s type resolution, its diagnostic notes and `jr-lsp`'s
    /// completion list all need the same answer, and string matches that must agree are the
    /// drift ADR-0022 §2 refuses for arithmetic.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "float32" => Some(Self::F32),
            "float64" => Some(Self::F64),
            _ => None,
        }
    }

    /// Every float type name Jairs has, in widening order.
    ///
    /// Narrower first, matching [`IntKind::NAMES`](crate::IntKind::NAMES)'s widening order,
    /// so a completion list reads the same way round for both families.
    pub const NAMES: &'static [&'static str] = &["float32", "float64"];

    /// The name this kind is spelled with: `float32`, `float64`.
    ///
    /// The inverse of [`FloatKind::from_name`], kept beside it so the two cannot disagree —
    /// a round-trip test asserts they do not.
    #[must_use]
    pub fn name(self) -> String {
        format!("float{}", self.bits)
    }

    /// The float kind of `ty`, if it is a float type.
    ///
    /// Shaped like [`IntKind::of`](crate::IntKind::of), so a consumer that must handle both
    /// families asks the same question twice rather than matching on `Item` itself.
    #[must_use]
    pub fn of(pool: &Pool, ty: PoolId) -> Option<Self> {
        match pool.item(ty) {
            Item::FloatType { bits } => Some(Self { bits: *bits }),
            _ => None,
        }
    }

    /// The width in bytes.
    #[must_use]
    pub const fn size(self) -> u64 {
        (self.bits / 8) as u64
    }

    /// Decodes a stored bit pattern into a mathematical value.
    ///
    /// A `float32`'s bits are its low 32, so this is where the two widths stop being
    /// interchangeable. Returning `f64` for both means every caller does its arithmetic in
    /// one type and re-encodes at the end, which is what [`FloatKind::encode`] is for.
    #[must_use]
    pub fn decode(self, bits: u64) -> f64 {
        if self.bits == 32 {
            f64::from(f32::from_bits(bits as u32))
        } else {
            f64::from_bits(bits)
        }
    }

    /// Encodes a mathematical value as a stored bit pattern of this width.
    ///
    /// Narrowing to `float32` **rounds to nearest and saturates to `inf`** when the value is
    /// too large, which is IEEE-754's own rule and is why this needs no error path
    /// (ADR-0040 §4). `as f32` is exactly that conversion in Rust.
    #[must_use]
    pub fn encode(self, value: f64) -> u64 {
        if self.bits == 32 {
            u64::from((value as f32).to_bits())
        } else {
            value.to_bits()
        }
    }
}

// ---------------------------------------------------------------------------
// The operations
// ---------------------------------------------------------------------------

/// A floating-point arithmetic operation.
///
/// Deliberately *not* `jr-mir`'s `BinOp`: this crate knows nothing about MIR, and the
/// separation is what lets one arithmetic implementation serve the interpreter, the
/// constant folder and any future consumer (ADR-0022 §2).
///
/// There is no `Rem`: ADR-0040 §7 leaves `%` on floats undefined, because C's `fmod`
/// truncates toward zero while Python's `%` follows the sign of the divisor, and the two
/// disagree on `-1.0 % 3.0`. A variant here would be a decision taken by accident.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FloatOp {
    /// `+`
    Add,
    /// `-`
    Sub,
    /// `*`
    Mul,
    /// `/` — `x / 0.0` is `inf`, and `0.0 / 0.0` is `NaN` (ADR-0040 §1).
    Div,
}

/// A floating-point comparison.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FloatCmp {
    /// `==` — **false** for `NaN` against itself, and **true** for `0.0` against `-0.0`.
    Eq,
    /// `!=` — the negation of [`FloatCmp::Eq`], so **true** for `NaN` against itself.
    Ne,
    /// `<`
    Lt,
    /// `<=`
    Le,
    /// `>`
    Gt,
    /// `>=`
    Ge,
}

/// Applies `op` to two mathematical values, producing bits of kind `out`.
///
/// Total: there is no error path, because ADR-0040 §1 gives every input an answer. That is
/// the whole difference from [`int_binary`](crate::int_binary), whose `Result` exists for
/// ADR-0002's traps.
///
/// The arithmetic is done in `f64` and re-encoded, which for `out.bits == 32` means a
/// `float32` operation is computed at `float64` precision and then rounded once. That is
/// **not** the same as computing it at `float32` precision throughout — the difference is a
/// double rounding, visible in the last bit of some results. It is accepted knowingly here
/// because the alternative is a second code path per width, and because `jr-codegen-clif`
/// emits native `f32` instructions either way, so the two engines are held equal by
/// `differential.rs` rather than by construction. A corpus case that disagreed would be a
/// real finding rather than a surprise.
#[must_use]
pub fn float_binary(op: FloatOp, out: FloatKind, a: f64, b: f64) -> u64 {
    let value = match op {
        FloatOp::Add => a + b,
        FloatOp::Sub => a - b,
        FloatOp::Mul => a * b,
        FloatOp::Div => a / b,
    };
    out.encode(value)
}

/// Compares two mathematical values under IEEE-754's rules.
///
/// `NaN` compares **false** to everything including itself, and `Ne` is the negation of
/// `Eq` rather than its own predicate — so `NaN != NaN` is `true`. Rust's `f64` operators
/// already implement exactly this, which is why the bodies are one line each: the subtlety
/// is in *not* reimplementing it.
#[must_use]
pub fn float_compare(op: FloatCmp, a: f64, b: f64) -> bool {
    match op {
        FloatCmp::Eq => a == b,
        FloatCmp::Ne => a != b,
        FloatCmp::Lt => a < b,
        FloatCmp::Le => a <= b,
        FloatCmp::Gt => a > b,
        FloatCmp::Ge => a >= b,
    }
}

/// Negates a mathematical value, producing bits of kind `out`.
///
/// Total, and exactly where [`int_negate`](crate::int_negate) is not: negating a float
/// flips its sign bit and always succeeds, while negating the most negative integer is one
/// past the maximum and traps (ADR-0002). `-0.0` is a real value and is what this returns
/// for `0.0`.
#[must_use]
pub fn float_negate(out: FloatKind, a: f64) -> u64 {
    out.encode(-a)
}

/// Converts a mathematical float value into integer *bits* of kind `out`.
///
/// **Truncates toward zero, and saturates** rather than wrapping or trapping (ADR-0040 §4):
/// `cast(s8, 1000.0)` is 127, and `NaN` is 0. Total, so every float has an answer in every
/// integer type and there is no third behaviour to define.
///
/// Rust's `as` on a float-to-integer cast is specified to do precisely this — it was
/// changed to saturate for the same reason — so the conversion is delegated rather than
/// hand-rolled, and `IntKind::wrap` then normalises the bits.
#[must_use]
pub fn float_to_int(out: crate::IntKind, a: f64) -> u64 {
    // Via `i128` because `out` may be any width up to 64 and `i128` holds every one of
    // them, signed or unsigned, exactly as `IntKind`'s own arithmetic does.
    let clamped = if a.is_nan() {
        0
    } else {
        let min = out.min();
        let max = out.max();
        // `as i128` already saturates at `i128`'s bounds, so this narrows to the
        // destination's range afterwards.
        (a as i128).clamp(min, max)
    };
    out.wrap(clamped)
}

/// Converts a mathematical integer value into float bits of kind `out`.
///
/// Rounds to nearest where the integer is not exactly representable, which for `float64`
/// means integers above 2^53. Unavoidable and standard (ADR-0040 §4).
#[must_use]
pub fn int_to_float(out: FloatKind, a: i128) -> u64 {
    out.encode(a as f64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_round_trip() {
        // The pair that must not drift: `from_name` and `name` are inverses, and `NAMES` is
        // exactly the set `from_name` accepts.
        for name in FloatKind::NAMES {
            let kind = FloatKind::from_name(name).expect("a listed name must parse");
            assert_eq!(&kind.name(), name);
        }
        assert_eq!(FloatKind::NAMES.len(), 2);
        assert!(FloatKind::from_name("float16").is_none());
        assert!(FloatKind::from_name("float").is_none());
        assert!(FloatKind::from_name("s64").is_none());
    }

    #[test]
    fn division_by_zero_is_infinity_rather_than_a_trap() {
        // ADR-0040 §1, and the signature is half the point: there is no `Result` to unwrap.
        let inf = float_binary(FloatOp::Div, FloatKind::F64, 1.0, 0.0);
        assert!(FloatKind::F64.decode(inf).is_infinite());
        assert!(FloatKind::F64.decode(inf) > 0.0);

        let neg = float_binary(FloatOp::Div, FloatKind::F64, -1.0, 0.0);
        assert!(FloatKind::F64.decode(neg).is_infinite());
        assert!(FloatKind::F64.decode(neg) < 0.0);
    }

    #[test]
    fn zero_over_zero_is_nan() {
        let nan = float_binary(FloatOp::Div, FloatKind::F64, 0.0, 0.0);
        assert!(FloatKind::F64.decode(nan).is_nan());
    }

    #[test]
    fn nan_is_not_equal_to_itself_and_negative_zero_equals_zero() {
        // The two comparisons a raw *bit* compare gets wrong, in opposite directions. This is
        // the hazard ADR-0040's Consequences names: the VM's `binary` falls back to a bit
        // compare for any non-integer scalar, and a float reaching that fallback would answer
        // both of these backwards — a plausible wrong answer rather than an error.
        let nan = f64::NAN;
        assert!(
            !float_compare(FloatCmp::Eq, nan, nan),
            "NaN == NaN is false"
        );
        assert!(float_compare(FloatCmp::Ne, nan, nan), "NaN != NaN is true");
        assert!(
            float_compare(FloatCmp::Eq, 0.0, -0.0),
            "0.0 == -0.0 is true despite different bits"
        );
        assert_ne!(
            FloatKind::F64.encode(0.0),
            FloatKind::F64.encode(-0.0),
            "and their bit patterns really do differ"
        );
    }

    #[test]
    fn an_overflowing_multiply_saturates_to_infinity() {
        let big = f64::MAX;
        let out = float_binary(FloatOp::Mul, FloatKind::F64, big, big);
        assert!(FloatKind::F64.decode(out).is_infinite());
    }

    #[test]
    fn narrowing_to_float32_rounds_and_saturates() {
        // Exactly representable in both, so this isolates the *rounding* from the saturation.
        assert_eq!(FloatKind::F32.decode(FloatKind::F32.encode(1.5)), 1.5);
        // Too large for `float32`: IEEE-754 says `inf`, not an error (ADR-0040 §4).
        assert!(
            FloatKind::F32
                .decode(FloatKind::F32.encode(1e300))
                .is_infinite()
        );
    }

    #[test]
    fn float_to_int_truncates_toward_zero_and_saturates() {
        use crate::IntKind;
        let s8 = IntKind {
            signed: true,
            bits: 8,
        };
        // Truncation, not rounding: 1.9 is 1 and -1.9 is -1.
        assert_eq!(s8.decode(float_to_int(s8, 1.9)), 1);
        assert_eq!(s8.decode(float_to_int(s8, -1.9)), -1);
        // Saturation rather than wrapping: 1000 in an `s8` is 127, not -24.
        assert_eq!(s8.decode(float_to_int(s8, 1000.0)), 127);
        assert_eq!(s8.decode(float_to_int(s8, -1000.0)), -128);
        // `NaN` is 0, matching `fcvt_to_sint_sat` and Rust.
        assert_eq!(s8.decode(float_to_int(s8, f64::NAN)), 0);
        // Infinity saturates like any out-of-range value.
        assert_eq!(s8.decode(float_to_int(s8, f64::INFINITY)), 127);
    }

    #[test]
    fn int_to_float_is_exact_below_the_mantissa_and_rounds_above_it() {
        assert_eq!(
            FloatKind::F64.decode(int_to_float(FloatKind::F64, 42)),
            42.0
        );
        // 2^53 + 1 is the first integer a `float64` cannot represent.
        let unrepresentable = (1i128 << 53) + 1;
        let rounded = FloatKind::F64.decode(int_to_float(FloatKind::F64, unrepresentable));
        assert_eq!(rounded, 9_007_199_254_740_992.0, "rounds to 2^53");
    }

    #[test]
    fn negation_flips_the_sign_of_zero() {
        // `-0.0` is a real value and negation is total, which is exactly where this differs
        // from `int_negate`'s trap on the most negative integer.
        let neg_zero = float_negate(FloatKind::F64, 0.0);
        assert_eq!(neg_zero, FloatKind::F64.encode(-0.0));
        assert!(FloatKind::F64.decode(neg_zero).is_sign_negative());
    }
}

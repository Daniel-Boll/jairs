//! ADR-0002's integer arithmetic, in the one place both evaluators can reach.
//!
//! # Why this is in `jr-pool` and not where it was
//!
//! It lived in `jr-vm`'s `value.rs`, which is where the only evaluator was. Once
//! `jr-mir` acquired a constant-folding pass there were two, and `jr-mir` cannot
//! depend on `jr-vm` — `jr-vm` depends on `jr-mir` to consume MIR, so that closes a
//! Cargo cycle. [ADR-0022](../../../docs/adr/0022-dce-constprop-shared-arithmetic.md)
//! §2 moves it here rather than duplicating it.
//!
//! `jr-pool` is the right floor and not merely a convenient one: [`IntKind::of`]
//! reads [`Item::IntType`]`{ signed, bits }`, so signedness and width are already
//! this crate's knowledge. What moved is the *arithmetic over* that knowledge.
//!
//! # What this does not fix
//!
//! `jr-codegen-clif` still has its own implementation, in Cranelift's
//! `sadd_overflow` family plus `trap_if`, and it cannot use this one because it
//! *emits* code rather than evaluating. So ADR-0002's arithmetic now has **two**
//! implementations rather than three, and the remaining pair is held equal by
//! `crates/jr-cli/tests/differential.rs` and nothing else. That is weaker than
//! ADR-0018 §2 achieved for layout, and ADR-0022 §2 says so instead of implying
//! otherwise.
//!
//! # Why the operator enums are this crate's own
//!
//! `jr_mir::BinOp` is not reused, and the reason is ADR-0017's: MIR *owning* its
//! operator set is what makes `&&` unrepresentable as an `Rvalue::Binary`, and the
//! exhaustive translation is what makes a new HIR operator a compile error. Moving
//! that type down here would turn a claim about a type MIR owns into a claim about
//! one it does not. `jr-mir` supplies the translation, so there is one mapping and
//! both callers use it.
//!
//! The split between [`IntOp`] and [`IntCmp`] mirrors the interpreter's own shape,
//! which was not arbitrary: a comparison's result is a `bool`, so it must not be
//! normalised through the destination's integer kind, and keeping them apart in the
//! type system means that mistake cannot be made twice.
//!
//! # Why the arithmetic goes through `i128`
//!
//! ADR-0002 makes `+`, `-`, `*`, `/`, `%` and unary `-` **trap** on overflow, with
//! `+%`, `-%`, `*%` as the explicit opt-out. Detecting that for an arbitrary
//! `(signed, bits)` pair is fiddly in the target width and trivial one width up, so
//! every operation widens to `i128`, computes exactly, and then asks whether the
//! result fits. `i128` holds every value of every Jairs integer type — including
//! `u64::MAX`, and the product of two `s64` magnitudes for the range check — so the
//! check is exact rather than approximate. The alternative, `checked_add` per
//! concrete Rust type, needs one arm per `(signed, bits)` combination and gets
//! narrow widths wrong.

use crate::item::{Item, PoolId};
use crate::pool::Pool;

// ---------------------------------------------------------------------------
// Integer kinds
// ---------------------------------------------------------------------------

/// The signedness and width of an integer type.
///
/// Read from the pool rather than inferred: ADR-0015 spells the width in
/// [`Item::IntType`], so there is nothing here to decide.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct IntKind {
    /// `true` for `sN`, `false` for `uN`.
    pub signed: bool,
    /// The width in bits, 1..=64.
    pub bits: u16,
}

impl IntKind {
    /// `s64`, the type an untyped integer literal defaults to (ADR-0016 §1).
    pub const S64: Self = Self {
        signed: true,
        bits: 64,
    };

    /// The integer kind of `ty`, if it is an integer type.
    ///
    /// A pointer is deliberately *not* an integer kind: pointer arithmetic is not
    /// expressible in Jairs-0, and treating an address as an `s64` would make the
    /// first attempt silently succeed.
    #[must_use]
    pub fn of(pool: &Pool, ty: PoolId) -> Option<Self> {
        match pool.item(ty) {
            Item::IntType { signed, bits } => Some(Self {
                signed: *signed,
                bits: *bits,
            }),
            _ => None,
        }
    }

    /// The mask of the bits this type occupies.
    #[must_use]
    pub const fn mask(self) -> u64 {
        if self.bits >= 64 {
            u64::MAX
        } else {
            (1u64 << self.bits) - 1
        }
    }

    /// The lowest value this type can hold.
    #[must_use]
    pub const fn min(self) -> i128 {
        if self.signed {
            -(1i128 << (self.bits - 1))
        } else {
            0
        }
    }

    /// The highest value this type can hold.
    #[must_use]
    pub const fn max(self) -> i128 {
        if self.signed {
            (1i128 << (self.bits - 1)) - 1
        } else {
            (1i128 << self.bits) - 1
        }
    }

    /// Recovers the mathematical value from normalised bits.
    #[must_use]
    pub const fn decode(self, bits: u64) -> i128 {
        let raw = bits & self.mask();
        if self.signed && self.bits < 128 && (raw >> (self.bits - 1)) & 1 == 1 {
            // Sign-extend: the value is `raw - 2^bits`.
            raw as i128 - (1i128 << self.bits)
        } else {
            raw as i128
        }
    }

    /// Normalises a mathematical value into this type's bits, wrapping.
    ///
    /// Used by the `+%`, `-%`, `*%` family, and to store the result of a checked
    /// operation once its range has been verified.
    #[must_use]
    pub const fn wrap(self, value: i128) -> u64 {
        (value as u64) & self.mask()
    }

    /// Normalises a value, or reports overflow.
    ///
    /// # Errors
    /// [`IntTrap::Overflow`] when `value` is outside the type's range.
    pub const fn check(self, value: i128, what: &'static str) -> Result<u64, IntTrap> {
        if value < self.min() || value > self.max() {
            return Err(IntTrap::Overflow { what });
        }
        Ok(self.wrap(value))
    }
}

// ---------------------------------------------------------------------------
// Operators
// ---------------------------------------------------------------------------

/// An arithmetic operation on two integers of the same kind.
///
/// Comparisons are [`IntCmp`] and not variants here, because their result is a
/// `bool` rather than a value of the operand type — mixing them is how a comparison
/// ends up normalised through an integer width.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IntOp {
    /// Addition, trapping on overflow (ADR-0002).
    Add,
    /// Subtraction, trapping on overflow.
    Sub,
    /// Multiplication, trapping on overflow.
    Mul,
    /// Division. Traps on a zero divisor and on `MIN / -1`.
    Div,
    /// Remainder. Traps on a zero divisor and on `MIN % -1`.
    Rem,
    /// Wrapping addition (`+%`).
    WrapAdd,
    /// Wrapping subtraction (`-%`).
    WrapSub,
    /// Wrapping multiplication (`*%`).
    WrapMul,
}

/// A comparison of two integers of the same kind. The result is a `bool`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IntCmp {
    /// Equality.
    Eq,
    /// Inequality.
    Ne,
    /// Less than.
    Lt,
    /// Less than or equal.
    Le,
    /// Greater than.
    Gt,
    /// Greater than or equal.
    Ge,
}

/// Why an integer operation could not produce a value.
///
/// Deliberately not `jr-vm`'s `Trap` or `jr-codegen-clif`'s `TrapKind`: this crate
/// knows nothing about interpreters or trap helpers, and each consumer maps this
/// onto its own vocabulary. [`IntTrap::Overflow`] carries the same `&'static str`
/// `jr-vm`'s message is built from, so the mapping cannot introduce wording drift —
/// which matters because `differential.rs` compares the finished sentence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IntTrap {
    /// The result did not fit the destination type (ADR-0002).
    Overflow {
        /// A short description of the operation, for the message.
        what: &'static str,
    },
    /// A divisor was zero.
    DivideByZero,
}

// ---------------------------------------------------------------------------
// The operations
// ---------------------------------------------------------------------------

/// Applies `op` to two mathematical values, producing bits of kind `out`.
///
/// `a` and `b` are already-decoded mathematical values (see [`IntKind::decode`]),
/// and the result is normalised bits of `out`. `out` is the *destination's* kind
/// rather than the operands', because that is what the result must fit.
///
/// # Errors
/// [`IntTrap`] for an overflow or a zero divisor, per ADR-0002.
pub const fn int_binary(op: IntOp, out: IntKind, a: i128, b: i128) -> Result<u64, IntTrap> {
    Ok(match op {
        IntOp::Add => match out.check(a + b, "addition") {
            Ok(bits) => bits,
            Err(trap) => return Err(trap),
        },
        IntOp::Sub => match out.check(a - b, "subtraction") {
            Ok(bits) => bits,
            Err(trap) => return Err(trap),
        },
        IntOp::Mul => match out.check(a * b, "multiplication") {
            Ok(bits) => bits,
            Err(trap) => return Err(trap),
        },
        IntOp::Div => {
            if b == 0 {
                return Err(IntTrap::DivideByZero);
            }
            // `MIN / -1` overflows rather than dividing: its true quotient is one
            // past the type's maximum. The range check catches it, which is why the
            // division happens in `i128` and is checked like everything else.
            match out.check(a / b, "division") {
                Ok(bits) => bits,
                Err(trap) => return Err(trap),
            }
        }
        IntOp::Rem => {
            if b == 0 {
                return Err(IntTrap::DivideByZero);
            }
            match out.check(a % b, "remainder") {
                Ok(bits) => bits,
                Err(trap) => return Err(trap),
            }
        }
        IntOp::WrapAdd => out.wrap(a + b),
        IntOp::WrapSub => out.wrap(a - b),
        IntOp::WrapMul => out.wrap(a * b),
    })
}

/// Compares two mathematical values.
#[must_use]
pub const fn int_compare(op: IntCmp, a: i128, b: i128) -> bool {
    match op {
        IntCmp::Eq => a == b,
        IntCmp::Ne => a != b,
        IntCmp::Lt => a < b,
        IntCmp::Le => a <= b,
        IntCmp::Gt => a > b,
        IntCmp::Ge => a >= b,
    }
}

/// Negates a mathematical value, producing bits of kind `out`.
///
/// # Errors
/// [`IntTrap::Overflow`] for the most negative value, whose negation is one past
/// the maximum (ADR-0002). The ordinary range check covers it, so there is no
/// special case.
pub const fn int_negate(out: IntKind, a: i128) -> Result<u64, IntTrap> {
    out.check(-a, "negation")
}

#[cfg(test)]
mod tests {
    use super::*;

    const U8: IntKind = IntKind {
        signed: false,
        bits: 8,
    };
    const S8: IntKind = IntKind {
        signed: true,
        bits: 8,
    };
    const U64: IntKind = IntKind {
        signed: false,
        bits: 64,
    };

    #[test]
    fn ranges_match_the_width_and_signedness() {
        assert_eq!((U8.min(), U8.max()), (0, 255));
        assert_eq!((S8.min(), S8.max()), (-128, 127));
        assert_eq!((U64.min(), U64.max()), (0, u64::MAX as i128));
        assert_eq!(
            (IntKind::S64.min(), IntKind::S64.max()),
            (i64::MIN as i128, i64::MAX as i128)
        );
    }

    #[test]
    fn decode_sign_extends_only_for_signed_types() {
        assert_eq!(S8.decode(0xff), -1);
        assert_eq!(U8.decode(0xff), 255);
        assert_eq!(IntKind::S64.decode(u64::MAX), -1);
        assert_eq!(U64.decode(u64::MAX), u64::MAX as i128);
    }

    #[test]
    fn wrap_and_decode_round_trip() {
        for value in [-128i128, -1, 0, 1, 127] {
            assert_eq!(S8.decode(S8.wrap(value)), value);
        }
        for value in [0i128, 1, 255] {
            assert_eq!(U8.decode(U8.wrap(value)), value);
        }
    }

    #[test]
    fn trapping_arithmetic_traps_at_the_types_own_width() {
        // Not at 64 bits: a narrow type must overflow where *it* overflows, which is
        // the property `jr-codegen-clif`'s per-width `Repr` also has to get right.
        assert_eq!(
            int_binary(IntOp::Add, S8, 127, 1),
            Err(IntTrap::Overflow { what: "addition" })
        );
        assert_eq!(int_binary(IntOp::Add, IntKind::S64, 127, 1), Ok(128));
        assert_eq!(
            int_binary(IntOp::Add, U8, 255, 1),
            Err(IntTrap::Overflow { what: "addition" })
        );
    }

    #[test]
    fn wrapping_arithmetic_does_not_trap() {
        assert_eq!(int_binary(IntOp::WrapAdd, S8, 127, 1), Ok(S8.wrap(-128)));
        assert_eq!(int_binary(IntOp::WrapAdd, U8, 255, 1), Ok(0));
    }

    #[test]
    fn division_reports_a_zero_divisor_and_the_one_overflow_it_has() {
        assert_eq!(int_binary(IntOp::Div, S8, 1, 0), Err(IntTrap::DivideByZero));
        assert_eq!(int_binary(IntOp::Rem, S8, 1, 0), Err(IntTrap::DivideByZero));
        assert_eq!(
            int_binary(IntOp::Div, S8, -128, -1),
            Err(IntTrap::Overflow { what: "division" }),
            "`MIN / -1` is one past the maximum, not a division"
        );
    }

    #[test]
    fn negation_traps_only_on_the_most_negative_value() {
        assert_eq!(int_negate(S8, -127), Ok(S8.wrap(127)));
        assert_eq!(
            int_negate(S8, -128),
            Err(IntTrap::Overflow { what: "negation" })
        );
    }

    #[test]
    fn a_comparison_is_mathematical_and_not_bitwise() {
        // The reason `IntCmp` is separate: these operate on decoded values, so a
        // signed comparison of `-1 < 1` cannot accidentally become `0xff < 0x01`.
        assert!(int_compare(IntCmp::Lt, -1, 1));
        assert!(!int_compare(IntCmp::Lt, 1, -1));
        assert!(int_compare(IntCmp::Ge, 5, 5));
    }
}

//! The runtime value a register holds, and the integer arithmetic ADR-0002 specifies.
//!
//! # Why a register is one of three things
//!
//! [`Value`] has a scalar case and an aggregate case, and nothing else. That
//! follows from `jr-mir`: `escape.rs`'s `is_register_representable` already decided
//! that `bool`, the integer types and pointers are the only types that fit a
//! register, and everything else — `string` above all, which ADR-0004 makes a
//! two-word `{data, count}` pair — lives in memory.
//!
//! An aggregate can nonetheless *reach* a register, which is why the case exists: a
//! `string` parameter is a block parameter, a `string` literal is an
//! `Operand::Constant`, and `Rvalue::Use` copies either. `jr-mir` spills an
//! aggregate parameter to a slot the moment a field of it is read, so an aggregate
//! in a register is only ever moved wholesale — never indexed.
//!
//! # Why no type is stored alongside
//!
//! A [`Value`] is bits, and its type comes from `MirBody::value(id).ty` or
//! `Pool::type_of` for a constant. Carrying a copy here would be a second answer to
//! a question MIR and the pool already answer, and two answers are two chances to
//! disagree — the same argument `jr_mir::Operand::Constant` makes for not storing a
//! type beside a `PoolId`.
//!
//! # Why the arithmetic goes through `i128`
//!
//! ADR-0002 makes `+`, `-`, `*`, `/`, `%` and unary `-` **trap** on overflow, with
//! `+%`, `-%`, `*%` as the explicit opt-out. Detecting that for an arbitrary
//! `(signed, bits)` pair is fiddly in the target width and trivial one width up, so
//! every operation widens to `i128`, computes exactly, and then asks whether the
//! result fits. `i128` holds every value of every Jairs integer type — including
//! `u64::MAX` and the product of two `s64`s' worth of magnitude for the range check
//! — so the check is exact rather than approximate. The alternative,
//! `checked_add` per concrete Rust type, needs one match arm per `(signed, bits)`
//! combination and gets `u1`-style widths wrong.

use jr_pool::{Item, Pool, PoolId};

use crate::error::{Trap, VmError};

// ---------------------------------------------------------------------------
// Value
// ---------------------------------------------------------------------------

/// A byte address inside the VM's [`crate::Memory`].
///
/// Zero is never a valid allocation, so it is available as the null pointer — the
/// gap `jr-mir`'s `zero_value` records as the reason a default-initialised pointer
/// local is treated as uninitialised.
pub type Address = u64;

/// What one register holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    /// The single value of type `void`.
    Void,
    /// A `bool` (0 or 1), an integer's raw bits, or a pointer's [`Address`].
    ///
    /// Integer bits are always **normalised to the type's width**: the bits above
    /// `bits` are zero, for signed and unsigned alike. That invariant is what makes
    /// [`Value::as_int`] able to recover the mathematical value from the type alone,
    /// and it is restored after every arithmetic operation by [`IntKind::wrap`].
    Scalar(u64),
    /// An aggregate held by value, as its bytes in target layout.
    Aggregate(Vec<u8>),
    /// A value that was never assigned.
    ///
    /// Distinct from any bit pattern, because `Rvalue::Undef` is a well-typed value
    /// with no bits rather than a zero. Reading one traps
    /// ([`Trap::UninitialisedRead`]) instead of yielding a plausible number, so the
    /// bug E0227 reports statically does not become a wrong answer when the check
    /// is skipped.
    Undefined,
}

impl Value {
    /// A boolean.
    #[must_use]
    pub const fn bool(value: bool) -> Self {
        Self::Scalar(if value { 1 } else { 0 })
    }

    /// The scalar bits, or an error naming what was found instead.
    ///
    /// # Errors
    /// [`VmError::Trap`] for an undefined value, [`VmError::Internal`] otherwise —
    /// a non-scalar here means MIR's types and the bytecode disagree.
    pub fn scalar(&self) -> Result<u64, VmError> {
        match self {
            Self::Scalar(bits) => Ok(*bits),
            Self::Undefined => Err(VmError::Trap(Trap::UninitialisedRead)),
            Self::Void => Err(VmError::internal("expected a scalar, found void")),
            Self::Aggregate(_) => Err(VmError::internal("expected a scalar, found an aggregate")),
        }
    }

    /// The scalar interpreted as a `bool`.
    ///
    /// # Errors
    /// As [`Self::scalar`].
    pub fn boolean(&self) -> Result<bool, VmError> {
        Ok(self.scalar()? != 0)
    }

    /// The mathematical value of an integer of type `kind`.
    ///
    /// # Errors
    /// As [`Self::scalar`].
    pub fn as_int(&self, kind: IntKind) -> Result<i128, VmError> {
        Ok(kind.decode(self.scalar()?))
    }

    /// The aggregate's bytes.
    ///
    /// # Errors
    /// [`VmError::Trap`] for an undefined value, [`VmError::Internal`] otherwise.
    pub fn aggregate(&self) -> Result<&[u8], VmError> {
        match self {
            Self::Aggregate(bytes) => Ok(bytes),
            Self::Undefined => Err(VmError::Trap(Trap::UninitialisedRead)),
            Self::Void | Self::Scalar(_) => {
                Err(VmError::internal("expected an aggregate, found a scalar"))
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Integer kinds
// ---------------------------------------------------------------------------

/// The signedness and width of an integer type.
///
/// Read from the pool rather than inferred: ADR-0015 spells the width in
/// `Item::IntType { signed, bits }`, so there is nothing here to decide.
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

    /// Normalises a value, or traps if it does not fit.
    ///
    /// # Errors
    /// [`Trap::Overflow`] when `value` is outside the type's range.
    pub const fn check(self, value: i128, what: &'static str) -> Result<u64, VmError> {
        if value < self.min() || value > self.max() {
            return Err(VmError::Trap(Trap::Overflow { what }));
        }
        Ok(self.wrap(value))
    }
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
    fn check_traps_outside_the_range_and_wrap_does_not() {
        assert_eq!(
            U8.check(256, "addition"),
            Err(VmError::Trap(Trap::Overflow { what: "addition" })),
            "ADR-0002: `+` traps rather than wrapping"
        );
        assert_eq!(U8.wrap(256), 0, "`+%` is the documented opt-out");
        assert!(S8.check(-129, "subtraction").is_err());
        assert_eq!(S8.check(127, "addition"), Ok(127));
    }

    #[test]
    fn bits_are_normalised_so_the_high_bits_never_leak() {
        // The invariant `Value::Scalar` documents: everything above `bits` is zero,
        // signed or not, so `decode` is a function of the type alone.
        assert_eq!(S8.wrap(-1), 0xff);
        assert_eq!(S8.wrap(-1) >> 8, 0);
    }

    #[test]
    fn an_undefined_value_traps_rather_than_reading_as_zero() {
        assert_eq!(
            Value::Undefined.scalar(),
            Err(VmError::Trap(Trap::UninitialisedRead)),
            "inventing a zero would hide the bug E0227 exists to report"
        );
    }

    #[test]
    fn a_scalar_is_not_an_aggregate_and_the_mismatch_is_a_compiler_bug() {
        assert!(matches!(
            Value::Scalar(1).aggregate(),
            Err(VmError::Internal(_))
        ));
        assert!(matches!(
            Value::Aggregate(vec![0; 16]).scalar(),
            Err(VmError::Internal(_))
        ));
    }

    #[test]
    fn int_kind_of_reads_the_pool_and_rejects_a_pointer() {
        let pool = Pool::new();
        assert_eq!(IntKind::of(&pool, PoolId::S64), Some(IntKind::S64));
        assert_eq!(
            IntKind::of(&pool, PoolId::U8),
            Some(IntKind {
                signed: false,
                bits: 8
            })
        );
        assert_eq!(
            IntKind::of(&pool, PoolId::PTR_U8),
            None,
            "pointer arithmetic is not expressible in Jairs-0, so a pointer must not look like an integer"
        );
        assert_eq!(IntKind::of(&pool, PoolId::BOOL), None);
    }
}

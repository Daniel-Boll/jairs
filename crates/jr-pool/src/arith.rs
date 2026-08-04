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

    /// The integer kind a builtin type *name* denotes, if it denotes one.
    ///
    /// **This is the one list of integer type names in the project** (ADR-0037 §1). It lives
    /// here rather than in `jr-sema` because three other places need the same answer —
    /// `resolve_type_name`, the "the builtin types are …" note, and the language server's
    /// completion list — and four string matches that must agree is the drift ADR-0022 §2
    /// refuses for arithmetic. A width added here appears everywhere at once.
    ///
    /// Only the widths Jairs has: 8, 16, 32 and 64, signed and unsigned. `s128` is not a
    /// Jairs type, and neither is `u1` — the `bits` field can hold them and the *language*
    /// does not have them, which is exactly why the name mapping is narrower than the
    /// representation.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        let (signed, digits) = match name.as_bytes().first()? {
            b's' => (true, &name[1..]),
            b'u' => (false, &name[1..]),
            _ => return None,
        };
        let bits = match digits {
            "8" => 8,
            "16" => 16,
            "32" => 32,
            "64" => 64,
            _ => return None,
        };
        Some(Self { signed, bits })
    }

    /// Every integer type name Jairs has, in widening order per signedness.
    ///
    /// Ordered rather than a set, so a completion list and a diagnostic's note read the same
    /// way round every time. Signed first because `s64` is what an untyped literal defaults
    /// to (ADR-0016 §1), so it is the name a reader meets first.
    pub const NAMES: &'static [&'static str] =
        &["s8", "s16", "s32", "s64", "u8", "u16", "u32", "u64"];

    /// The name this kind is spelled with: `s64`, `u16`.
    ///
    /// The inverse of [`IntKind::from_name`], and kept beside it so the two cannot disagree —
    /// a round-trip test asserts they do not.
    #[must_use]
    pub fn name(self) -> String {
        format!("{}{}", if self.signed { 's' } else { 'u' }, self.bits)
    }

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
            // An enum *is* its backing integer at run time (ADR-0041 §3), which is `s64` for
            // every enum this project has. Answering here rather than at each consumer is what
            // lets the interpreter, the folder and both back ends treat `Perm.READ | Perm.WRITE`
            // as the integer operation it is — and it is `Item`-level rather than a special
            // case per evaluator, which is ADR-0022 §2's rule.
            //
            // Deliberately *both* forms: a plain enum reaches this only through `cast` and a
            // comparison, both of which want the backing kind too, and refusing it here would
            // make `cast(s64, colour)` need a second path.
            Item::EnumType { .. } => Some(Self::S64),
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
    /// Bitwise and (`&`) — cannot trap (ADR-0042).
    BitAnd,
    /// Bitwise or (`|`) — cannot trap.
    BitOr,
    /// Bitwise xor (`^`) — cannot trap.
    BitXor,
    /// Left shift (`<<`). Traps on a count outside the type's width (ADR-0042 §3).
    Shl,
    /// Right shift (`>>`), arithmetic for a signed type and logical for an unsigned one
    /// (ADR-0042 §2). Traps on an out-of-range count.
    Shr,
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
    /// A shift count was negative or `>=` the type's width (ADR-0042 §3).
    ///
    /// A trap rather than a mask or a saturation, for ADR-0002's reason: a shift by 8 of an
    /// 8-bit value produces a result the program did not ask for, and every alternative is a
    /// *silent* wrong answer.
    ShiftOutOfRange,
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
        // **The wrapping operators compute in the destination width, not in `i128`** (ADR-0116 §2). `a` and
        // `b` are decoded to `i128`, and for two `u64` operands near the top of the range `a * b` is ~2^128,
        // which overflows `i128` itself and panicked in a debug build — *before* `wrap` could take the low
        // bits. The whole point of the wrapping forms is that the high bits are discarded (ADR-0002's opt-out),
        // so the arithmetic is done on the truncated `u64` values with Rust's `wrapping_*`, which is exactly
        // "keep the low `bits`, discarding overflow" for any width the mask then normalises. Found by `Map`'s
        // hash multiply overflowing at comptime while native code (which has no `i128` intermediary) was fine —
        // an engine divergence the corpus differential caught.
        IntOp::WrapAdd => out.wrap((a as u64).wrapping_add(b as u64) as i128),
        IntOp::WrapSub => out.wrap((a as u64).wrapping_sub(b as u64) as i128),
        IntOp::WrapMul => out.wrap((a as u64).wrapping_mul(b as u64) as i128),

        // Bitwise operations are done on the *stored bits*, then re-normalised. Working on
        // the decoded `i128` would sign-extend a negative narrow value into the high bits and
        // then mask them off again — the same answer for `&`, `|` and `^`, but only because
        // the mask undoes it, which is a coincidence worth not relying on.
        IntOp::BitAnd => out.wrap(a & b),
        IntOp::BitOr => out.wrap(a | b),
        IntOp::BitXor => out.wrap(a ^ b),

        // A count is out of range when it is negative or `>=` the width (ADR-0042 §3). Both
        // are the same trap, because reinterpreting a negative count as a shift the other way
        // would make `x << -1` silently mean `x >> 1`.
        IntOp::Shl => {
            // `as` rather than `i128::from`, because this function is `const` and `From`
            // is not yet a const trait. Widening a `u16` to an `i128` is exact either way.
            if b < 0 || b >= out.bits as i128 {
                return Err(IntTrap::ShiftOutOfRange);
            }
            out.wrap(a << b)
        }
        IntOp::Shr => {
            if b < 0 || b >= out.bits as i128 {
                return Err(IntTrap::ShiftOutOfRange);
            }
            // `a` is the *decoded* value, so it is already negative for a negative signed
            // input — and `>>` on a negative `i128` is arithmetic in Rust. That gives sign
            // extension for a signed type for free. For an unsigned type `a` is non-negative,
            // so the same shift is logical. The type decides, exactly as it does for `/`
            // (ADR-0042 §2), without this needing to branch on `out.signed`.
            out.wrap(a >> b)
        }
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

/// Complements a value's bits, producing bits of kind `out` (ADR-0042 §4).
///
/// Normalised to the type's own width, so `~0` in a `u8` is 255 rather than `-1` truncated —
/// which is what makes a narrow type complement within its width instead of at 64 bits and
/// then differing between the two engines.
#[must_use]
pub const fn int_not(out: IntKind, a: i128) -> u64 {
    out.wrap(!a)
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

    #[test]
    fn every_name_round_trips_through_from_name() {
        // The two directions are written separately, so this is what keeps them honest.
        for name in IntKind::NAMES {
            let kind = IntKind::from_name(name).unwrap_or_else(|| panic!("{name} must parse"));
            assert_eq!(kind.name(), *name);
        }
    }

    #[test]
    fn the_tower_is_exactly_eight_names() {
        // Guards against a width being added to the list without a decision: ADR-0037 §1 says
        // 8/16/32/64 in both signednesses, and `float32`/`float64` are a later wave.
        assert_eq!(IntKind::NAMES.len(), 8);
        assert_eq!(IntKind::from_name("s64"), Some(IntKind::S64));
        assert_eq!(
            IntKind::from_name("u16"),
            Some(IntKind {
                signed: false,
                bits: 16
            })
        );
    }

    #[test]
    fn a_name_the_language_does_not_have_is_not_a_kind() {
        // The `bits` field can represent all of these. The *language* does not have them,
        // which is the whole reason the name mapping is narrower than the representation.
        for name in [
            "s128", "u1", "s7", "int", "float32", "s", "u", "s64x", "bool",
        ] {
            assert_eq!(IntKind::from_name(name), None, "{name} must not parse");
        }
    }

    #[test]
    fn bitwise_operations_normalise_to_the_type_width() {
        let u8k = IntKind {
            signed: false,
            bits: 8,
        };
        assert_eq!(
            u8k.decode(int_binary(IntOp::BitAnd, u8k, 0xF0, 0x3C).unwrap()),
            0x30
        );
        assert_eq!(
            u8k.decode(int_binary(IntOp::BitOr, u8k, 0xF0, 0x0F).unwrap()),
            0xFF
        );
        assert_eq!(
            u8k.decode(int_binary(IntOp::BitXor, u8k, 0xFF, 0x0F).unwrap()),
            0xF0
        );
        // `~0` in a `u8` is 255, not `-1` (ADR-0042 §4): the complement is normalised to the
        // type's own width rather than taken at 64 bits and truncated.
        assert_eq!(u8k.decode(int_not(u8k, 0)), 255);
    }

    #[test]
    fn right_shift_is_arithmetic_for_signed_and_logical_for_unsigned() {
        // ADR-0042 §2: the *type* decides, exactly as it does for `/`.
        let s8k = IntKind {
            signed: true,
            bits: 8,
        };
        assert_eq!(s8k.decode(int_binary(IntOp::Shr, s8k, -8, 1).unwrap()), -4);
        let u8k = IntKind {
            signed: false,
            bits: 8,
        };
        assert_eq!(u8k.decode(int_binary(IntOp::Shr, u8k, 240, 4).unwrap()), 15);
    }

    #[test]
    fn a_shift_count_at_or_past_the_width_traps() {
        // Not masked to the width (which x86 does natively and would silently turn `<< 8`
        // into `<< 0`), and not saturated to zero. ADR-0042 §3.
        let s8k = IntKind {
            signed: true,
            bits: 8,
        };
        assert_eq!(
            int_binary(IntOp::Shl, s8k, 1, 8),
            Err(IntTrap::ShiftOutOfRange)
        );
        assert_eq!(
            int_binary(IntOp::Shr, s8k, 1, 8),
            Err(IntTrap::ShiftOutOfRange)
        );
        // A count one below the width is fine, which is what makes the boundary a boundary.
        assert!(int_binary(IntOp::Shl, s8k, 1, 7).is_ok());
    }

    #[test]
    fn a_negative_shift_count_traps_rather_than_reversing_direction() {
        // `x << -1` must not silently mean `x >> 1` (ADR-0042 §3).
        let s64k = IntKind::S64;
        assert_eq!(
            int_binary(IntOp::Shl, s64k, 4, -1),
            Err(IntTrap::ShiftOutOfRange)
        );
    }

    #[test]
    fn a_left_shift_that_overflows_the_type_wraps_rather_than_trapping() {
        // The *count* is checked; the result is not. `1 << 7` in an `s8` is -128, because the
        // bit lands on the sign. That is what the bits do, and ADR-0002's overflow trap is
        // about arithmetic whose true result is unrepresentable — a shift's result is exactly
        // the bits requested.
        let s8k = IntKind {
            signed: true,
            bits: 8,
        };
        assert_eq!(s8k.decode(int_binary(IntOp::Shl, s8k, 1, 7).unwrap()), -128);
    }

    #[test]
    fn each_width_masks_and_bounds_correctly() {
        // The tower is a naming change *because* these were already generic. Asserted rather
        // than assumed, since every new width relies on it.
        let u16_kind = IntKind::from_name("u16").expect("parses");
        assert_eq!(u16_kind.min(), 0);
        assert_eq!(u16_kind.max(), 65_535);
        let s8 = IntKind::from_name("s8").expect("parses");
        assert_eq!(s8.min(), -128);
        assert_eq!(s8.max(), 127);
        // Truncation is the cast's runtime behaviour (ADR-0037 §2): 300 wraps to 44 in `u8`.
        assert_eq!(IntKind::from_name("u8").expect("parses").wrap(300), 44);
        // And sign extension survives narrowing to a signed type.
        assert_eq!(s8.decode(s8.wrap(-1)), -1);
    }
}
